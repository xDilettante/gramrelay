"""Export/verify a bounded committed source ZIP; --consumer executes trusted code."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
import threading
import tomllib
import unicodedata
import zipfile
import zlib

ROOT = Path(__file__).resolve().parents[1]
MAX_ARCHIVE, MAX_EXPANDED, MAX_FILE = 16 * 1024**2, 64 * 1024**2, 16 * 1024**2
MAX_ENTRIES, MAX_MANIFEST = 10_000, 1024**2
DEVICES = {"CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"} | {
    prefix + number for prefix in ("COM", "LPT") for number in "123456789\u00b9\u00b2\u00b3"
}


def json_bytes(value):
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2, allow_nan=False) + "\n").encode("utf-8")


def read_bounded(path, limit):
    with Path(path).open("rb") as stream:
        data = stream.read(limit + 1)
    if len(data) > limit:
        raise ValueError("input exceeds source-bundle limit")
    return data


def parts(name):
    if len(name.encode("utf-8")) > 512 or any(ord(char) < 32 or char in '\\:<>"|?*~' for char in name):
        raise ValueError("unsafe archive path")
    result = (name[:-1] if name.endswith("/") else name).split("/")
    if len(result) > 16 or any(
        not part or part in (".", "..") or part.endswith((" ", "."))
        or len(part.encode("utf-8")) > 255 or part.split(".")[0].rstrip(" ").upper() in DEVICES
        or part.casefold() in (".git", "target") or part.casefold().startswith(".env")
        or part.casefold().endswith((".pem", ".key")) for part in result
    ):
        raise ValueError("unsafe archive path")
    return result


def _extra(data):
    if data and (len(data) != 9 or data[:5] != b"UT\x05\x00\x01"):
        raise ValueError("unsupported ZIP extension")


def _central_preflight(data):
    if len(data) > MAX_ARCHIVE:
        raise ValueError("archive exceeds compressed limit")
    endings = []
    for length in (40, 64):
        position = len(data) - 22 - length
        if position >= 0 and data[position:position + 4] == b"PK\x05\x06":
            fields = struct.unpack_from("<4s4H2IH", data, position)
            if fields[-1] == length and re.fullmatch(rb"[0-9a-f]{40}|[0-9a-f]{64}", data[position + 22:]):
                endings.append((position, fields))
    if len(endings) != 1:
        raise ValueError("missing canonical commit ZIP comment/end record")
    end, (_, disk, directory_disk, disk_count, count, size, offset, _) = endings[0]
    if data[max(0, end - 20):end - 16] == b"PK\x06\x07":
        raise ValueError("ZIP64 locator is forbidden")
    if disk or directory_disk or disk_count != count or not 0 < count <= MAX_ENTRIES or offset + size != end:
        raise ValueError("unsupported ZIP directory/entry count")
    directory_start = offset
    seen, expanded, compressed = 0, 0, 0
    while offset < end:
        if offset + 46 > end or data[offset:offset + 4] != b"PK\x01\x02":
            raise ValueError("invalid ZIP directory framing")
        needed, flags, method = struct.unpack_from("<3H", data, offset + 6)
        packed, unpacked = struct.unpack_from("<2I", data, offset + 20)
        name_len, extra_len, comment_len, member_disk = struct.unpack_from("<4H", data, offset + 28)
        local_offset = struct.unpack_from("<I", data, offset + 42)[0]
        finish = offset + 46 + name_len + extra_len + comment_len
        if (needed > 20 or flags & ~0x80E or method not in (0, 8) or member_disk or comment_len
                or not 0 < name_len <= 512 or extra_len > 256 or finish > end
                or local_offset >= offset or packed > MAX_ARCHIVE or unpacked > MAX_FILE):
            raise ValueError("unsupported ZIP member metadata")
        parts(data[offset + 46:offset + 46 + name_len].decode("utf-8" if flags & 0x800 else "cp437"))
        extra = data[offset + 46 + name_len:finish]
        # Native Git emits only its bounded UT timestamp field. ZIP64 and name
        # override extensions are outside this small-source profile.
        _extra(extra)
        seen, expanded, compressed = seen + 1, expanded + unpacked, compressed + packed
        if seen > MAX_ENTRIES or expanded > MAX_EXPANDED or compressed > MAX_ARCHIVE:
            raise ValueError("archive exceeds cumulative limits")
        offset = finish
    if seen != count:
        raise ValueError("ZIP directory count mismatch")
    return directory_start


def _member(data, info, directory_start):
    """Validate local framing and bounded genuine stream end, not ZipExtFile's truncated view."""
    offset = info.header_offset
    if offset < 0 or offset + 30 > directory_start or data[offset:offset + 4] != b"PK\x03\x04":
        raise ValueError("invalid ZIP local header")
    needed, flags, method = struct.unpack_from("<3H", data, offset + 4)
    local_sizes = struct.unpack_from("<3I", data, offset + 14)
    name_len, extra_len = struct.unpack_from("<2H", data, offset + 26)
    start = offset + 30 + name_len + extra_len
    finish = start + info.compress_size
    sizes = (info.CRC, info.compress_size, info.file_size)
    if (needed != info.extract_version or flags != info.flag_bits or method != info.compress_type
            or not 0 < name_len <= 512 or extra_len > 256 or finish > directory_start
            or local_sizes not in ((0, 0, 0), sizes) or (not flags & 8 and local_sizes != sizes)):
        raise ValueError("ZIP local/directory metadata mismatch")
    name = data[offset + 30:offset + 30 + name_len]
    if name != info.orig_filename.encode("utf-8" if flags & 0x800 else "cp437"):
        raise ValueError("ZIP local name mismatch")
    _extra(data[offset + 30 + name_len:start])
    packed = data[start:finish]
    if method == 8:
        try:
            decoder = zlib.decompressobj(-15)
            content = decoder.decompress(packed, info.file_size + 1)
        except zlib.error as error:
            raise ValueError("invalid raw deflate stream") from error
        if not decoder.eof or decoder.unused_data or decoder.unconsumed_tail:
            raise ValueError("deflate stream exceeds size or has incomplete/trailing data")
    else:
        content = packed
    if len(content) != info.file_size or zlib.crc32(content) != info.CRC:
        raise ValueError("actual member size/CRC mismatch")
    if flags & 8:
        if data[finish:finish + 4] == b"PK\x07\x08":
            finish += 4
        if finish + 12 > directory_start or struct.unpack_from("<3I", data, finish) != sizes:
            raise ValueError("ZIP data descriptor mismatch")
        finish += 12
    return content, finish


def inspect_bundle(data):
    directory_start = _central_preflight(data)
    files, spellings, explicit, regular, parents = {}, {}, set(), set(), set()
    extents = []
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        commit = archive.comment.decode("ascii")
        prefix = None
        for info in archive.infolist():
            components = parts(info.filename)
            if prefix is None:
                prefix = components[0]
            if components[0] != prefix:
                raise ValueError("multiple source roots")
            for depth in range(1, len(components) + 1):
                spelling = "/".join(components[:depth])
                key = unicodedata.normalize("NFC", unicodedata.normalize("NFC", spelling).casefold())
                if key in spellings and spellings[key] != spelling:
                    raise ValueError("ambiguous archive path/parent alias")
                spellings[key] = spelling
            if len(spellings) > MAX_ENTRIES:
                raise ValueError("too many archive path components")
            name = "/".join(components)
            if name in explicit or any("/".join(components[:depth]) in regular for depth in range(1, len(components))):
                raise ValueError("duplicate archive entry or file parent")
            explicit.add(name)
            parents.update("/".join(components[:depth]) for depth in range(1, len(components)))
            directory = info.is_dir()
            mode = info.external_attr >> 16
            allowed = (0, stat.S_IFDIR | 0o755) if directory else (0, stat.S_IFREG | 0o644, stat.S_IFREG | 0o755)
            if mode not in allowed or (info.external_attr & 0x10 and not directory):
                raise ValueError("links/special/unsupported modes are forbidden")
            content, finish = _member(data, info, directory_start)
            extents.append((info.header_offset, finish))
            if directory:
                if info.file_size or info.compress_size:
                    raise ValueError("directory contains payload")
                continue
            if len(components) == 1 or name in parents:
                raise ValueError("file conflicts with source root/parent")
            regular.add(name)
            files["/".join(components[1:])] = (content, 0o755 if mode & 0o111 else 0o644)
    position = 0
    for start, finish in sorted(extents):
        if start != position:
            raise ValueError("overlapping/gapped ZIP local records")
        position = finish
    if position != directory_start:
        raise ValueError("unexpected data before ZIP directory")
    config = tomllib.loads(files["Cargo.toml"][0].decode("utf-8"))
    package = config.get("package")
    version = package.get("version") if isinstance(package, dict) else None
    if not isinstance(version, str) or not re.fullmatch(r"[0-9][0-9A-Za-z.+-]{0,79}", version) or prefix != f"telegram-rust-{version}":
        raise ValueError("source version/prefix mismatch")
    document = {"format_version": 1, "commit": commit, "version": version, "prefix": prefix,
                "archive_sha256": hashlib.sha256(data).hexdigest(),
                "files": {name: {"size": len(content), "sha256": hashlib.sha256(content).hexdigest(), "mode": mode}
                          for name, (content, mode) in sorted(files.items())}}
    return document, files


def verify_bundle(data, manifest):
    def object_pairs(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate manifest key")
            result[key] = value
        return result
    if len(manifest) > MAX_MANIFEST:
        raise ValueError("manifest exceeds limit")
    supplied = json.loads(manifest, object_pairs_hook=object_pairs)
    document, files = inspect_bundle(data)
    if json_bytes(supplied) != json_bytes(document):
        raise ValueError("source manifest/inventory/digest mismatch")
    return document, files


def materialize(files, root):
    """Write a verified immutable snapshot into a new directory in a trusted parent."""
    root.mkdir()
    for name, (content, mode) in files.items():
        destination = root / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        with destination.open("xb") as stream:
            stream.write(content)
        destination.chmod(mode)


def _git(source, *arguments, limit=MAX_ARCHIVE):
    process = subprocess.Popen(["git", "--no-replace-objects", "-c", "core.autocrlf=false", "-c", "core.eol=lf", "-c", "core.attributesFile=", "-c", "core.fsmonitor=false", "-c", "gc.auto=0", *arguments], cwd=source,
                               stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=os.environ | {"TZ": "UTC", "GIT_TERMINAL_PROMPT": "0"})
    expired = threading.Event()
    def expire():
        expired.set()
        try: process.kill()
        except OSError: pass
    timer = threading.Timer(60, expire)
    timer.start()
    try:
        data = process.stdout.read(limit + 1)
        if len(data) > limit:
            process.kill()
        code = process.wait()
    finally:
        timer.cancel()
        timer.join()
        if process.poll() is None:
            process.kill()
        process.wait()
        process.stdout.close()
    if expired.is_set() or code or len(data) > limit:
        raise ValueError("Git command failed or exceeded output/time limit")
    return data


def build(source, output):
    source, output = source.resolve(), output.resolve()
    commit = _git(source, "rev-parse", "HEAD", limit=1024).decode().strip()
    if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", commit) or _git(source, "status", "--porcelain", "--ignore-submodules=all", limit=MAX_MANIFEST):
        raise ValueError("source export requires a clean committed snapshot")
    version = tomllib.loads(_git(source, "show", f"{commit}:Cargo.toml", limit=MAX_FILE).decode())["package"]["version"]
    if not isinstance(version, str) or not re.fullmatch(r"[0-9][0-9A-Za-z.+-]{0,79}", version):
        raise ValueError("unsupported source version")
    expected, total = {}, 0
    for record in _git(source, "ls-tree", "-r", "-l", "-z", "--full-tree", commit).split(b"\0"):
        if not record: continue
        header, raw_name = record.split(b"\t", 1)
        mode, kind, oid, size = header.split()
        name = raw_name.decode("utf-8")
        parts(f"telegram-rust-{version}/" + name)
        if kind != b"blob" or mode not in (b"100644", b"100755") or int(size) > MAX_FILE:
            raise ValueError("unsupported committed source entry")
        expected[name] = (oid.decode(), int(mode, 8) & 0o777)
        total += int(size)
        if len(expected) > MAX_ENTRIES or total > MAX_EXPANDED:
            raise ValueError("committed source exceeds bundle limits")
    data = _git(source, "archive", "--format=zip", "-6", f"--prefix=telegram-rust-{version}/", commit)
    document, files = inspect_bundle(data)
    if document["commit"] != commit or files.keys() != expected.keys():
        raise ValueError("archive inventory differs from captured Git tree")
    for name, (content, mode) in files.items():
        oid, expected_mode = expected[name]
        digest = hashlib.new("sha1" if len(oid) == 40 else "sha256", f"blob {len(content)}\0".encode() + content).hexdigest()
        if digest != oid or mode != expected_mode:
            raise ValueError("archive bytes/modes differ from committed Git blob")
    metadata = json_bytes(document)
    if len(metadata) > MAX_MANIFEST:
        raise ValueError("generated manifest exceeds profile limit")
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"telegram-rust-{version}-{commit[:12]}.zip"
    manifest = archive.with_suffix(".manifest.json")
    for path, content in [(archive, data), (manifest, metadata)]:
        if path.is_symlink() or getattr(path, "is_junction", lambda: False)():
            raise ValueError("artifact output cannot be a link")
        if path.exists():
            if read_bounded(path, max(MAX_ARCHIVE, MAX_MANIFEST)) != content:
                raise FileExistsError("different source artifact already exists")
        else:
            with path.open("xb") as stream: stream.write(content)
    return archive, manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    export = commands.add_parser("build")
    export.add_argument("--source", type=Path, default=ROOT)
    export.add_argument("--output-dir", type=Path, default=ROOT / "target/dist")
    check = commands.add_parser("check")
    check.add_argument("archive", type=Path)
    check.add_argument("--manifest", type=Path, required=True)
    check.add_argument("--consumer", action="store_true", help="execute trusted archived project verifiers/Rust fixture")
    check.add_argument("--target-dir", type=Path, default=ROOT / "target/source-consumer")
    args = parser.parse_args()
    try:
        if args.command == "build":
            archive, manifest = build(args.source, args.output_dir)
            print(f"archive: {archive}\nmanifest: {manifest}")
        else:
            data = read_bounded(args.archive, MAX_ARCHIVE)
            manifest = read_bounded(args.manifest, MAX_MANIFEST)
            document, files = verify_bundle(data, manifest)
            print(f"verified: {document['commit']} / {len(files)} files / {document['archive_sha256']}", flush=True)
            if args.consumer:
                if shutil.disk_usage(ROOT).free < 20 * 1024**3:
                    raise ValueError("less than 20 GiB free for relocated consumer")
                with tempfile.TemporaryDirectory(prefix="telegram source ") as temporary:
                    source = Path(temporary).resolve() / "source"
                    materialize(files, source)
                    for script in ("verify_grammers.py", "verify_rmcp.py"):
                        subprocess.run([sys.executable, str(source / "tools" / script), "check"], cwd=source, check=True)
                    subprocess.run([sys.executable, str(source / "tools/check_external.py"), "--source", str(source), "--target-dir", str(args.target_dir.resolve()), "--run"], cwd=source, check=True)
        return 0
    except (OSError, ValueError, KeyError, zipfile.BadZipFile, subprocess.SubprocessError) as error:
        print(f"source bundle failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
