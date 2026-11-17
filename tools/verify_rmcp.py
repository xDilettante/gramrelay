"""Verify the MCP timestamp patch against the authenticated official release."""
import argparse
import os
from pathlib import Path
import sys
import tarfile
import urllib.request

from verify_grammers import LIMIT, ROOT, patch_text, upstream

VERSION = "3.11.0"
PREFIX = f"rmcp-{VERSION}"
SHA256 = "391f153431ab68664d946513ea0cb3edbca78eaf99c5a983036cacab6220d85b"
PATCHED = {
    "Cargo.toml", "src/lib.rs", "src/model/annotated.rs",
    "src/model/request_state.rs", "src/task_manager.rs", "src/service.rs",
    "tests/common/handlers.rs", "tests/test_logging.rs",
}
LOCAL = {"Cargo.lock", "PATCH.md", "tg-timestamps.patch", "src/tg_timestamp.rs"}
ADDED_SOURCE = {"src/tg_timestamp.rs"}


def original_source(data: bytes) -> dict[str, bytes]:
    return upstream(data, prefix=PREFIX, checksum=SHA256, patched=PATCHED)


def verify_patch(original: dict[str, bytes], directory: Path, *, generate=False):
    # New executable source is part of the recorded patch, using an empty base.
    # Metadata (including the patch itself) is inventory-checked only.
    expected = patch_text(
        original | {name: b"" for name in ADDED_SOURCE}, directory,
        patched=PATCHED | ADDED_SOURCE, local=LOCAL - ADDED_SOURCE,
    )
    destination = directory / "tg-timestamps.patch"
    if generate:
        destination.write_bytes(expected)
    elif destination.read_bytes().replace(b"\r\n", b"\n") != expected:
        raise ValueError("recorded patch differs from actual source")


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
            with urllib.request.urlopen(f"https://static.crates.io/crates/rmcp/{PREFIX}.crate", timeout=30) as response:
                data = response.read(LIMIT + 1)
            original_source(data)
            path = ROOT / "target" / "upstream" / f"{PREFIX}.crate"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        if path is None:
            raise ValueError("pinned archive unavailable; supply --archive or --download")
        with path.open("rb") as archive:
            original = original_source(archive.read(LIMIT + 1))
        verify_patch(original, ROOT / "vendor" / "rmcp", generate=args.command == "generate")
        print(f"{args.command}: rmcp {VERSION}: authenticated archive, {len(original) - len(PATCHED)} unchanged files, {len(PATCHED)} changes and {len(ADDED_SOURCE)} added source recorded")
        return 0
    except (OSError, ValueError, tarfile.TarError):
        print("rmcp patch verification failed", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
