"""Verify downstream grammers patches against pinned, hash-checked release archives."""
import argparse
import difflib
import hashlib
import io
import os
from pathlib import Path, PurePosixPath
import sys
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
VERSION = "0.12.0"
PREFIX = f"grammers-mtsender-{VERSION}"
SHA256 = "9a6339a64b7b70ae4447ae4ae7b1b5a1e2e15649882bf26550df356e7a8bb1a4"
LIMIT = 4 * 1024 * 1024
PATCHED = {"Cargo.toml", "src/sender_pool.rs", "src/sender.rs", "src/configuration.rs"}
LOCAL = {"Cargo.lock", "src/tg_safety_tests.rs", "src/tg_rpc_fixture.rs", "src/tg_sender_receive_tests.rs", "PATCH.md", "tg-safety.patch", "LICENSE-APACHE"}
PACKAGES = {
    "grammers-mtsender": (SHA256, PATCHED, LOCAL, "tg-safety.patch"),
    "grammers-mtproto": (
        "fc95dd02ac37101d8121e5098706a9c9b342e855ca7d7b8b5daa91ec7b119649",
        {"Cargo.toml", "src/manual_tl.rs", "src/mtp/encrypted.rs", "src/mtp/mod.rs"},
        {"Cargo.lock", "PATCH.md", "tg-receive.patch", "LICENSE-APACHE", "src/mtp/tg_receive_tests.rs"},
        "tg-receive.patch",
    ),
    "grammers-tl-types": (
        "eb4da570f8dbcea3e0d0a1746240045555bacc56b4d57d3a81204ee0e84736bd",
        {"Cargo.toml", "src/deserialize.rs"},
        {"Cargo.lock", "PATCH.md", "tg-decode.patch", "LICENSE-APACHE", "src/tg_decode_tests.rs"},
        "tg-decode.patch",
    ),
}


def upstream(data: bytes, *, prefix=None, checksum=None, patched=None) -> dict[str, bytes]:
    prefix = PREFIX if prefix is None else prefix
    checksum = SHA256 if checksum is None else checksum
    patched = PATCHED if patched is None else patched
    if len(data) > LIMIT or hashlib.sha256(data).hexdigest() != checksum:
        raise ValueError("upstream archive checksum mismatch")
    result = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive.getmembers():
            path = PurePosixPath(member.name)
            if not path.parts or path.parts[0] != prefix or ".." in path.parts or "\\" in member.name:
                raise ValueError("unexpected archive path")
            if member.isdir():
                continue
            if not member.isfile() or member.size > LIMIT:
                raise ValueError("unexpected archive entry")
            relative = str(PurePosixPath(*path.parts[1:]))
            if relative != "Cargo.lock":
                result[relative] = archive.extractfile(member).read()
    if not patched <= result.keys():
        raise ValueError("missing upstream files")
    return result


def patch_text(original: dict[str, bytes], directory: Path, *, patched=None, local=None) -> bytes:
    patched = PATCHED if patched is None else patched
    local = LOCAL if local is None else local
    paths = list(directory.rglob("*"))
    if any(path.is_symlink() or getattr(path, "is_junction", lambda: False)() for path in [directory, *paths]):
        raise ValueError("vendored source cannot contain links")
    actual = {path.relative_to(directory).as_posix() for path in paths if path.is_file()}
    if actual != set(original) | local:
        raise ValueError("unexpected or missing vendored files")
    diff = []
    for name, expected in sorted(original.items()):
        path = directory / name
        if path.is_symlink():
            raise ValueError("vendored source cannot be a symlink")
        content = path.read_bytes()
        if name not in patched:
            if content != expected:
                raise ValueError(f"unreviewed upstream change: {name}")
        else:
            # Normalize only text newlines: Git may materialize CRLF on Windows.
            before = expected.decode("utf-8").splitlines(keepends=False)
            after = content.decode("utf-8").splitlines(keepends=False)
            diff.extend(difflib.unified_diff([line + "\n" for line in before], [line + "\n" for line in after], fromfile=f"upstream/{name}", tofile=f"patched/{name}"))
    return "".join(diff).encode("utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("check", "generate"))
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--download", action="store_true")
    parser.add_argument("--package", choices=("all", *PACKAGES), default="all")
    args = parser.parse_args()
    try:
        if args.archive and args.package == "all":
            raise ValueError("--archive requires one explicit --package")
        for name in PACKAGES if args.package == "all" else (args.package,):
            checksum, patched, local, patch_name = PACKAGES[name]
            prefix = f"{name}-{VERSION}"
            path = args.archive
            if path is None:
                cache = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")) / "registry" / "cache"
                path = next(cache.glob(f"*/{prefix}.crate"), None)
                downloaded = ROOT / "target" / "upstream" / f"{prefix}.crate"
                if path is None and downloaded.is_file():
                    path = downloaded
            if path is None and args.download:
                path = ROOT / "target" / "upstream" / f"{prefix}.crate"
                url = f"https://static.crates.io/crates/{name}/{prefix}.crate"
                with urllib.request.urlopen(url, timeout=30) as response:
                    data = response.read(LIMIT + 1)
                upstream(data, prefix=prefix, checksum=checksum, patched=patched)
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
            if path is None:
                raise ValueError("pinned archive unavailable; supply --archive or --download")
            with path.open("rb") as archive:
                original = upstream(archive.read(LIMIT + 1), prefix=prefix, checksum=checksum, patched=patched)
            directory = ROOT / "vendor" / name
            expected = patch_text(original, directory, patched=patched, local=local)
            destination = directory / patch_name
            if args.command == "generate":
                destination.write_bytes(expected)
            elif destination.read_bytes().replace(b"\r\n", b"\n") != expected:
                raise ValueError("recorded patch differs from actual source")
            print(f"{args.command}: {name}: pinned archive, {len(original) - len(patched)} unchanged files, {len(patched)} recorded source/manifest changes")
        return 0
    except (OSError, ValueError, tarfile.TarError):
        print("grammers patch verification failed", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
