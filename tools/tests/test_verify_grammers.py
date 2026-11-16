import hashlib
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("verify_grammers", Path(__file__).resolve().parents[1] / "verify_grammers.py")
verify = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(verify)


class VendorVerification(unittest.TestCase):
    def test_archive_is_authenticated_before_parsing(self):
        with self.assertRaisesRegex(ValueError, "checksum"):
            verify.upstream(b"not a trusted archive")

    def test_archive_paths_are_checked_without_extracting_files(self):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
            entry = tarfile.TarInfo(f"{verify.PREFIX}/../outside")
            entry.size = 1
            archive.addfile(entry, io.BytesIO(b"x"))
        data = buffer.getvalue()
        with patch.object(verify, "SHA256", hashlib.sha256(data).hexdigest()):
            with self.assertRaisesRegex(ValueError, "path"):
                verify.upstream(data)

    def test_only_the_two_reviewed_files_may_differ(self):
        original = {"Cargo.toml": b"original\n", "src/sender_pool.rs": b"original\n", "src/crypto.rs": b"unchanged\n"}
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for name in set(original) | verify.LOCAL:
                destination = directory / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(original.get(name, b"local\n"))
            (directory / "src/sender_pool.rs").write_bytes(b"patched\r\n")
            result = verify.patch_text(original, directory)
            self.assertIn(b"-original\n+patched\n", result)
            (directory / "src/crypto.rs").write_bytes(b"modified\n")
            with self.assertRaisesRegex(ValueError, "unreviewed"):
                verify.patch_text(original, directory)
            (directory / "src/crypto.rs").write_bytes(original["src/crypto.rs"])
            (directory / "unexpected.rs").write_bytes(b"new\n")
            with self.assertRaisesRegex(ValueError, "unexpected"):
                verify.patch_text(original, directory)


if __name__ == "__main__":
    unittest.main()
