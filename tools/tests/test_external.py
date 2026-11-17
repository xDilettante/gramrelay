import copy
from pathlib import Path
import sys
import tempfile
import shutil
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from check_external import ROOT, LIBRARIES, SDK, check_graph, check_manifests


class ExternalGraph(unittest.TestCase):
    def test_manifest_policy_rejects_fallbacks_and_child_overrides(self):
        expected = {**LIBRARIES, **{name: f"vendor/{name}" for name in SDK}}
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary).resolve()
            for relative in expected.values():
                destination = source / relative / "Cargo.toml"
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / relative / "Cargo.toml", destination)
            check_manifests(source)
            path = source / "tg-agent/Cargo.toml"
            original = path.read_text()
            for modified in (
                original.replace('path = "../vendor/rmcp"', 'path = "../vendor/rmcp", version = "3.11.0"'),
                original.replace('tg = { path = ".."', 'tg = { path = "..", version = "0.1.0-alpha.1"'),
                original.replace('tg-mtproto = { path = "../tg-mtproto"', 'tg-mtproto = { path = "../tg-mtproto", version = "0.1.0-alpha.1"'),
                original.replace('publish = false', 'publish = true'),
                original + '\n[patch.crates-io]\nrmcp = { path = "../vendor/rmcp" }\n',
                original.replace('path = "../vendor/rmcp"', 'path = "../other/rmcp"'),
            ):
                path.write_text(modified)
                with self.assertRaises(ValueError):
                    check_manifests(source)
            path.write_text(original)

    def test_source_identity_duplicates_and_inactive_packages_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary).resolve()
            expected = {**LIBRARIES, **{name: f"vendor/{name}" for name in SDK}}
            packages = [{"id": name, "name": name, "source": None, "version": SDK.get(name, "0.1.0"),
                         "manifest_path": str(source / relative / "Cargo.toml")} for name, relative in expected.items()]
            original = {"packages": packages, "resolve": {"nodes": [{"id": name} for name in expected]}}
            check_graph(original, source)
            for modification in ("registry", "duplicate", "wrong-path", "unresolved", "version", "chrono"):
                metadata = copy.deepcopy(original)
                sdk = next(package for package in metadata["packages"] if package["name"] == "rmcp")
                if modification == "registry": sdk["source"] = "registry+https://example.invalid"
                elif modification == "duplicate": metadata["packages"].append(copy.deepcopy(sdk))
                elif modification == "wrong-path": sdk["manifest_path"] = str(source / "other/Cargo.toml")
                elif modification == "unresolved": metadata["resolve"]["nodes"] = [{"id": "consumer"}]
                elif modification == "version": sdk["version"] = "3.10.0"
                else: metadata["packages"].append({"name": "chrono"})
                with self.subTest(modification=modification), self.assertRaises(ValueError):
                    check_graph(metadata, source)

            revision = "a" * 40
            git_source = f"git+{source.as_uri()}?rev={revision}#{revision}"
            for package in original["packages"]:
                package["source"] = git_source
                relative = expected[package["name"]]
                package["manifest_path"] = str(source / "git-checkout" / relative / "Cargo.toml")
            check_graph(original, source, revision=revision)
            original["packages"][-1]["source"] = git_source.replace(revision, "b" * 40)
            with self.assertRaises(ValueError):
                check_graph(original, source, revision=revision)
