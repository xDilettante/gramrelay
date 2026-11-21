import io
import json
import os
import shlex
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile
import struct
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import source_bundle as bundle

PREFIX = "telegram-rust-0.1.0-alpha.1/"
COMMIT = "a" * 40
CARGO = b'[package]\nname="tg"\nversion="0.1.0-alpha.1"\n'


def archive(entries=(), *, comment=COMMIT.encode()):
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as stream:
        stream.comment = comment
        for name, data, mode in [("Cargo.toml", CARGO, 0o100644), ("src/lib.rs", b"// fixture\n", 0o100644), *entries]:
            info = zipfile.ZipInfo(PREFIX + name)
            # ZipInfo normalizes backslashes on Windows; preserve hostile raw
            # names so the test reaches the reader's metadata preflight.
            info.filename = info.orig_filename = PREFIX + name
            info.external_attr = mode << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            stream.writestr(info, data)
    return output.getvalue()


class SourceBundle(unittest.TestCase):
    def test_integrity_and_exact_snapshot_extraction(self):
        data = archive()
        document, files = bundle.inspect_bundle(data)
        manifest = bundle.json_bytes(document)
        bundle.verify_bundle(data, manifest)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "archive.zip").write_bytes(data)
            (root / "archive.zip").write_bytes(b"replaced after validation")
            bundle.materialize(files, root / "source")
            self.assertEqual((root / "source/Cargo.toml").read_bytes(), CARGO)
            with self.assertRaises(FileExistsError):
                bundle.materialize(files, root / "source")
        altered = dict(document, commit="b" * 40)
        for damaged in [data[:-1], data + b"trailing"]:
            with self.assertRaises((ValueError, zipfile.BadZipFile)):
                bundle.verify_bundle(damaged, manifest)
        with self.assertRaises(ValueError):
            bundle.verify_bundle(data, bundle.json_bytes(altered))
        with self.assertRaises(ValueError):
            bundle.verify_bundle(data, b'{"format_version":1,"format_version":1}')

    def test_unsafe_paths_links_aliases_and_parents_refuse_before_writes(self):
        for name in ["../outside", "/absolute", "C:/outside", "a\\outside", "x:stream", "NUL.txt", "CON .txt", "COM\u00b9", "LONGNA~1/x", "a.", "a ", "a/./b", ".git/config", "target/cache", ".ENV", "a/" * 17 + "b", "x" * 256]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                bundle.inspect_bundle(archive([(name, b"x", 0o100644)]))
        cases = [
            [("link", b"../outside", 0o120777)],
            [("SRC/other.rs", b"x", 0o100644)],
            [("src/lib.rs", b"duplicate", 0o100644)],
            [("parent", b"x", 0o100644), ("parent/child", b"x", 0o100644)],
            [("caf\u00e9/a", b"x", 0o100644), ("cafe\u0301/b", b"x", 0o100644)],
            [("\u1e96\u031b/a", b"x", 0o100644), ("H\u031b\u0331/b", b"x", 0o100644)],
        ]
        for entries in cases:
            with self.subTest(entries=entries), self.assertRaises(ValueError):
                bundle.inspect_bundle(archive(entries))

    def test_local_header_and_actual_deflate_size_must_match_directory(self):
        data = archive()
        first = data.index(b"PK\x01\x02")
        central = data.index(b"PK\x01\x02", first + 4)
        local = struct.unpack_from("<I", data, central + 42)[0]
        for local_field, value in [(4, 45), (6, 1), (8, 99)]:
            damaged = bytearray(data)
            struct.pack_into("<H", damaged, local + local_field, value)
            with self.subTest(field=local_field), self.assertRaises(ValueError):
                bundle.inspect_bundle(bytes(damaged))
        damaged = bytearray(data)
        for offset in [central + 16, central + 24, local + 14, local + 22]:
            struct.pack_into("<I", damaged, offset, 0)
        with self.assertRaises(ValueError):
            bundle.inspect_bundle(bytes(damaged))
        damaged = bytearray(data)
        name_len = struct.unpack_from("<H", damaged, local + 26)[0]
        insertion = local + 30 + name_len
        damaged[insertion:insertion] = b"\x01\x00\x00\x00"  # local-only ZIP64 extra
        struct.pack_into("<H", damaged, local + 28, 4)
        end = len(damaged) - 22 - len(COMMIT)
        struct.pack_into("<I", damaged, end + 16, struct.unpack_from("<I", damaged, end + 16)[0] + 4)
        with self.assertRaises(ValueError):
            bundle.inspect_bundle(bytes(damaged))

    def test_resource_limits_and_metadata_preflight(self):
        data = archive()
        for constant, limit in [("MAX_ARCHIVE", len(data) - 1), ("MAX_FILE", 4), ("MAX_EXPANDED", 4), ("MAX_ENTRIES", 1)]:
            with self.subTest(constant=constant), patch.object(bundle, constant, limit), self.assertRaises(ValueError):
                bundle.inspect_bundle(data)
        damaged = bytearray(data)
        central = damaged.index(b"PK\x01\x02")
        damaged[central + 28:central + 30] = (65535).to_bytes(2, "little")
        with patch.object(bundle.zipfile, "ZipFile", side_effect=AssertionError("preflight must reject before parser")):
            with self.assertRaises(ValueError):
                bundle.inspect_bundle(bytes(damaged))
            damaged = bytearray(data)
            end = len(data) - 22 - len(COMMIT)
            damaged[end - 20:end - 16] = b"PK\x06\x07"
            with self.assertRaises(ValueError):
                bundle.inspect_bundle(bytes(damaged))

    def test_git_snapshot_reproducibility_attributes_and_replacements(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repo = root / "repo"
            repo.mkdir()
            hooks = root / "empty-hooks"
            hooks.mkdir()
            env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
            env.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")

            def git(*arguments):
                return subprocess.check_output(["git", "-c", f"core.hooksPath={hooks}", "-c", "commit.gpgsign=false", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", *arguments], cwd=repo, env=env)
            git("init", "-q")
            (repo / "src").mkdir()
            (repo / "Cargo.toml").write_bytes(CARGO)
            original = b"// $Format:%H$\n"
            (repo / "src/lib.rs").write_bytes(original)
            git("add", ".")
            git("commit", "-qm", "fixture")
            first = git("rev-parse", "HEAD").decode().strip()
            with self.assertRaises(ValueError):
                bundle._git(repo, "show", f"{first}:Cargo.toml", limit=1)
            with patch.object(bundle, "MAX_MANIFEST", 4), self.assertRaises(ValueError):
                bundle.build(repo, root / "too-large-manifest")
            one, manifest = bundle.build(repo, root / "one")
            two, _ = bundle.build(repo, root / "two")
            self.assertEqual(one.read_bytes(), two.read_bytes())
            self.assertEqual(bundle.verify_bundle(one.read_bytes(), manifest.read_bytes())[0]["commit"], first)
            attrs = repo / ".git/info/attributes"
            for value in ["src/lib.rs export-ignore\n", "src/lib.rs export-subst\n"]:
                attrs.write_text(value)
                with self.assertRaises(ValueError):
                    bundle.build(repo, root / "bad")
            attrs.write_text("")
            (repo / "src/lib.rs").write_bytes(b"// replacement\n")
            git("add", ".")
            git("commit", "-qm", "replacement")
            second = git("rev-parse", "HEAD").decode().strip()
            git("update-ref", "HEAD", first)
            git("read-tree", first)
            (repo / "src/lib.rs").write_bytes(original)
            git("replace", first, second)
            replaced, _ = bundle.build(repo, root / "replacement-ignored")
            self.assertEqual(replaced.read_bytes(), one.read_bytes())
            marker = root / "fsmonitor-invoked"
            hook = root / "fsmonitor-hook"
            hook.write_text("#!/bin/sh\nprintf invoked > " + shlex.quote(marker.as_posix()) + "\nprintf 'token\\0'\n", newline="\n")
            hook.chmod(0o755)
            git("config", "core.fsmonitor", hook.as_posix())
            bundle.build(repo, root / "no-fsmonitor")
            self.assertFalse(marker.exists(), "source reads must not launch configured helpers")
            (repo / "dirty").write_bytes(b"untracked")
            with self.assertRaises(ValueError):
                bundle.build(repo, root / "dirty-output")
