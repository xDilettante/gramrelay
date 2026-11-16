"""Verify the narrow mtsender patch against the pinned, hash-checked release archive."""
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
VENDOR = ROOT / "vendor" / "grammers-mtsender"
VERSION = "0.12.0"
PREFIX = f"grammers-mtsender-{VERSION}"
SHA256 = "9a6339a64b7b70ae4447ae4ae7b1b5a1e2e15649882bf26550df356e7a8bb1a4"
URL = f"https://static.crates.io/crates/grammers-mtsender/{PREFIX}.crate"
LIMIT = 4 * 1024 * 1024
COPIED = {"Cargo.toml", "Cargo.toml.orig", "DEPS.md", "README.md", ".cargo_vcs_info.json"}
PATCHED = {"Cargo.toml", "src/sender_pool.rs"}
LOCAL = {"Cargo.lock", "src/tg_safety_tests.rs", "PATCH.md", "tg-safety.patch", "LICENSE-APACHE"}


def upstream(data: bytes) -> dict[str, bytes]:
    if len(data) > LIMIT or hashlib.sha256(data).hexdigest() != SHA256:
        raise ValueError("upstream archive checksum mismatch")
    result = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive.getmembers():
            path = PurePosixPath(member.name)
            if not path.parts or path.parts[0] != PREFIX or ".." in path.parts or "\\" in member.name:
                raise ValueError("unexpected archive path")
            if member.isdir():
                continue
            if not member.isfile() or member.size > LIMIT:
                raise ValueError("unexpected archive entry")
            relative = str(PurePosixPath(*path.parts[1:]))
            if relative in COPIED or relative.startswith(("src/", "tests/")):
                result[relative] = archive.extractfile(member).read()
    if not PATCHED <= result.keys():
        raise ValueError("missing upstream files")
    return result


def patch_text(original: dict[str, bytes], directory: Path) -> bytes:
    paths = list(directory.rglob("*"))
    if any(path.is_symlink() or getattr(path, "is_junction", lambda: False)() for path in [directory, *paths]):
        raise ValueError("vendored source cannot contain links")
    actual = {path.relative_to(directory).as_posix() for path in paths if path.is_file()}
    if actual != set(original) | LOCAL:
        raise ValueError("unexpected or missing vendored files")
    diff = []
    for name, expected in sorted(original.items()):
        path = directory / name
        if path.is_symlink():
            raise ValueError("vendored source cannot be a symlink")
        content = path.read_bytes()
        if name not in PATCHED:
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
    args = parser.parse_args()
    try:
        path = args.archive
        if path is None:
            cache = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")) / "registry" / "cache"
            path = next(cache.glob(f"*/{PREFIX}.crate"), None)
            downloaded = ROOT / "target" / "upstream" / f"{PREFIX}.crate"
            if path is None and downloaded.is_file():
                path = downloaded
        if path is None and args.download:
            path = ROOT / "target" / "upstream" / f"{PREFIX}.crate"
            with urllib.request.urlopen(URL, timeout=30) as response:
                data = response.read(LIMIT + 1)
            upstream(data)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        if path is None:
            raise ValueError("pinned archive unavailable; supply --archive or --download")
        with path.open("rb") as archive:
            original = upstream(archive.read(LIMIT + 1))
        expected = patch_text(original, VENDOR)
        destination = VENDOR / "tg-safety.patch"
        if args.command == "generate":
            destination.write_bytes(expected)
        elif destination.read_bytes().replace(b"\r\n", b"\n") != expected:
            raise ValueError("recorded patch differs from actual source")
        print(f"{args.command}: pinned archive and {len(original) - len(PATCHED)} unchanged files; two reviewed source/manifest changes")
        return 0
    except (OSError, ValueError, tarfile.TarError):
        print("grammers patch verification failed", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
