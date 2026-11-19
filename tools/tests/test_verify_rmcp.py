import hashlib
import io
import os
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import verify_rmcp as verify


class McpVerification(unittest.TestCase):
    def test_bundled_release_verifies_with_empty_cache_and_no_network(self):
        original = {name: b"original\n" for name in verify.PATCHED}
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
            for name, content in original.items():
                entry = tarfile.TarInfo(f"{verify.PREFIX}/{name}")
                entry.size = len(content)
                archive.addfile(entry, io.BytesIO(content))
        data = buffer.getvalue()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundled = root / "vendor/upstream" / f"{verify.PREFIX}.crate"
            bundled.parent.mkdir(parents=True)
            bundled.write_bytes(data)
            directory = root / "vendor/rmcp"
            for name in set(original) | verify.LOCAL:
                destination = directory / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(original.get(name, b"local\n"))
            verify.verify_patch(original, directory, generate=True)
            cache = root / "cache"
            with patch.object(verify, "ROOT", root), patch.object(verify, "SHA256", hashlib.sha256(data).hexdigest()), \
                 patch.dict(os.environ, {"CARGO_HOME": str(cache)}), \
                 patch.object(verify.urllib.request, "urlopen", side_effect=AssertionError("unexpected network")), \
                 patch.object(sys, "argv", ["verify_rmcp.py", "check"]):
                self.assertEqual(verify.main(), 0)
                cached = cache / "registry/cache/index" / bundled.name
                cached.parent.mkdir(parents=True)
                cached.write_bytes(data)
                bundled.write_bytes(b"corrupt bundled input")
                self.assertEqual(verify.main(), 1)
                with patch.object(sys, "argv", ["verify_rmcp.py", "check", "--archive", str(cached)]):
                    self.assertEqual(verify.main(), 0)

    def test_release_checksum_is_required(self):
        with self.assertRaisesRegex(ValueError, "checksum"):
            verify.original_source(b"untrusted MCP source")

    def test_authenticated_archive_still_cannot_escape_destination(self):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
            entry = tarfile.TarInfo(f"{verify.PREFIX}/../outside")
            entry.size = 1
            archive.addfile(entry, io.BytesIO(b"x"))
        data = buffer.getvalue()
        with patch.object(verify, "SHA256", hashlib.sha256(data).hexdigest()):
            with self.assertRaisesRegex(ValueError, "path"):
                verify.original_source(data)

    def test_source_inventory_and_recorded_diff_are_checked(self):
        original = {"Cargo.toml": b"original\n", "src/crypto.rs": b"unchanged\n"}
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for name in set(original) | verify.LOCAL:
                destination = directory / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(original.get(name, b"local\n"))
            (directory / "Cargo.toml").write_bytes(b"patched\n")
            verify.verify_patch(original, directory, generate=True)
            verify.verify_patch(original, directory)
            (directory / "src/tg_timestamp.rs").write_bytes(b"unrecorded clock change\n")
            with self.assertRaisesRegex(ValueError, "recorded patch"):
                verify.verify_patch(original, directory)
            verify.verify_patch(original, directory, generate=True)
            (directory / "tg-timestamps.patch").write_bytes(b"wrong diff\n")
            with self.assertRaisesRegex(ValueError, "recorded patch"):
                verify.verify_patch(original, directory)
            verify.verify_patch(original, directory, generate=True)
            (directory / "src/crypto.rs").write_bytes(b"unreviewed\n")
            with self.assertRaisesRegex(ValueError, "unreviewed"):
                verify.verify_patch(original, directory)
            (directory / "src/crypto.rs").write_bytes(original["src/crypto.rs"])
            (directory / "extra.rs").write_bytes(b"extra\n")
            with self.assertRaisesRegex(ValueError, "unexpected"):
                verify.verify_patch(original, directory)


if __name__ == "__main__":
    unittest.main()
