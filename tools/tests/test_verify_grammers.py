import hashlib
import importlib.util
import io
import os
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("verify_grammers", Path(__file__).resolve().parents[1] / "verify_grammers.py")
verify = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(verify)


class VendorVerification(unittest.TestCase):
    def test_bundled_release_needs_no_cache_and_corruption_cannot_fall_back(self):
        name = "fixture"
        prefix = f"{name}-{verify.VERSION}"
        original = b"original manifest\n"
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
            entry = tarfile.TarInfo(f"{prefix}/Cargo.toml")
            entry.size = len(original)
            archive.addfile(entry, io.BytesIO(original))
        data = buffer.getvalue()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundled = root / "vendor/upstream" / f"{prefix}.crate"
            bundled.parent.mkdir(parents=True)
            bundled.write_bytes(data)
            directory = root / "vendor" / name
            directory.mkdir()
            (directory / "Cargo.toml").write_bytes(original)
            (directory / "record.patch").write_bytes(b"")
            cache = root / "cache"
            packages = {name: (hashlib.sha256(data).hexdigest(), {"Cargo.toml"}, {"record.patch"}, "record.patch")}
            with patch.object(verify, "ROOT", root), patch.object(verify, "PACKAGES", packages), \
                 patch.dict(os.environ, {"CARGO_HOME": str(cache)}), \
                 patch.object(verify.urllib.request, "urlopen", side_effect=AssertionError("unexpected network")), \
                 patch.object(sys, "argv", ["verify_grammers.py", "check"]):
                self.assertEqual(verify.main(), 0)
                cached = cache / "registry/cache/index" / bundled.name
                cached.parent.mkdir(parents=True)
                cached.write_bytes(data)
                bundled.write_bytes(b"corrupt bundled input")
                self.assertEqual(verify.main(), 1)
                with patch.object(sys, "argv", ["verify_grammers.py", "check", "--package", name, "--archive", str(cached)]):
                    self.assertEqual(verify.main(), 0)
                bundled.unlink()
                bundled.mkdir()
                self.assertEqual(verify.main(), 1)

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

    def test_only_declared_reviewed_files_may_differ(self):
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

    def test_package_allowlists_do_not_leak_into_each_other(self):
        original={"Cargo.toml":b"original\n","src/sender_pool.rs":b"original\n"}
        with tempfile.TemporaryDirectory() as temporary:
            directory=Path(temporary)
            for name,content in original.items():
                destination=directory/name;destination.parent.mkdir(parents=True,exist_ok=True);destination.write_bytes(content)
            (directory/"src/sender_pool.rs").write_bytes(b"unexpected\n")
            with self.assertRaisesRegex(ValueError,"unreviewed"):
                verify.patch_text(original,directory,patched={"Cargo.toml"},local=set())


if __name__ == "__main__":
    unittest.main()
