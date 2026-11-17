import copy
import hashlib
import io
import json
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile
import tomllib
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import private_packages as packages

ROOT = Path(__file__).resolve().parents[2]


def fixture(root):
    for name, relative in packages.DIRECTORIES.items():
        directory = root / relative
        (directory / "src").mkdir(parents=True, exist_ok=True)
        (directory / "Cargo.toml").write_bytes((ROOT / relative / "Cargo.toml").read_bytes())
        if (ROOT / relative / "Cargo.toml.orig").is_file():
            (directory / "Cargo.toml.orig").write_bytes((ROOT / relative / "Cargo.toml.orig").read_bytes())
        (directory / "src/lib.rs").write_bytes(b"pub fn fixture() {}\n")
        for path in (ROOT / relative).glob("*"):
            if path.is_file() and (path.name.startswith("LICENSE") or path.name == "PATCH.md" or path.suffix == ".patch"):
                shutil.copyfile(path, directory / path.name)
    return root


def archive(path, directory, manifest):
    data = tomllib.loads(manifest)
    prefix = f"{data['package']['name']}-{data['package']['version']}/"
    files = {"Cargo.toml": manifest.encode(), "Cargo.toml.orig": (directory / "Cargo.toml").read_bytes(),
             "src/lib.rs": (directory / "src/lib.rs").read_bytes()}
    for item in directory.glob("LICENSE*"):
        files[item.name] = item.read_bytes()
    if data["package"].get("license-file"):
        files["LICENSE"] = (ROOT / "LICENSE").read_bytes()
    with tarfile.open(path, "w:gz") as stream:
        for name, content in files.items():
            info = tarfile.TarInfo(prefix + name)
            info.size = len(content)
            stream.addfile(info, io.BytesIO(content))


class PrivatePackages(unittest.TestCase):
    def test_neutral_parent_refusal_leaves_failed_identity_before_any_child(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            document = {"commit": "a" * 40, "archive_sha256": "b" * 64}
            with patch.object(packages, "ROOT", root), patch.object(packages.check_compat, "guard"), \
                 patch.object(packages.source_bundle, "build", return_value=(root / "input.zip", root / "input.json")), \
                 patch.object(packages.source_bundle, "read_bounded", return_value=b"fixture"), \
                 patch.object(packages.source_bundle, "verify_bundle", return_value=(document, {})), \
                 patch.object(packages, "choose_scratch_parent", side_effect=ValueError("configuration refusal")), \
                 patch.object(packages.subprocess, "run") as child:
                with self.assertRaisesRegex(ValueError, "configuration refusal"):
                    packages.rehearse("1.100.0", root / "output")
                child.assert_not_called()
            summary = json.loads(next((root / "output").rglob("summary.json")).read_text())
            self.assertEqual(summary["status"], "failed")
            self.assertEqual(summary["source_commit"], document["commit"])
            self.assertEqual(summary["commands"], [])

    def test_cargo_models_detect_lost_build_target_or_feature_gate(self):
        def model(directory):
            return {"packages": [{"name": "tg", "manifest_path": str(directory / "Cargo.toml"), "edition": "2024",
                                   "targets": [{"name": "tg", "kind": ["lib"], "src_path": str(directory / "src/lib.rs")},
                                               {"name": "build-script-build", "kind": ["custom-build"], "src_path": str(directory / "build.rs")},
                                               {"name": "fixture", "kind": ["bin"], "src_path": str(directory / "src/bin/fixture.rs"), "required-features": ["test-kit"]}]}]}
        left, right = model(ROOT / "stage"), model(ROOT / "registry")
        self.assertEqual(packages.package_models(left), packages.package_models(right))
        right["packages"][0]["targets"].pop(1)
        self.assertNotEqual(packages.package_models(left), packages.package_models(right))
        right = model(ROOT / "registry")
        right["packages"][0]["targets"][2]["required-features"] = []
        self.assertNotEqual(packages.package_models(left), packages.package_models(right))

    def test_bootstrap_environment_and_ancestor_configs_are_isolated(self):
        with patch.dict(packages.os.environ, {"TELEGRAM_TOKEN": "sentinel", "CARGO_REGISTRIES_PRIVATE_TOKEN": "sentinel",
                                             "RUSTC_WRAPPER": "sentinel", "RUSTUP_TOOLCHAIN": "nightly"}):
            env = packages.isolated_environment(ROOT / "isolated")
            self.assertFalse(any(value == "sentinel" for value in env.values()))
            self.assertEqual(env["RUSTUP_AUTO_INSTALL"], "0")
            self.assertEqual(env["CARGO_HOME"], str(ROOT / "isolated"))
        first = ROOT / "fake-temp"
        neutral = ROOT / "neutral"
        with patch.object(packages.tempfile, "gettempdir", return_value=str(first)), patch.object(packages, "ROOT", neutral / "project"):
            for forbidden in (".cargo/config.toml", ".cargo/config", "Cargo.toml"):
                with patch.object(Path, "exists", autospec=True, side_effect=lambda path: path == first / forbidden):
                    self.assertEqual(packages.choose_scratch_parent(), neutral)
            with patch.object(Path, "exists", return_value=True):
                with self.assertRaisesRegex(ValueError, "configuration"):
                    packages.choose_scratch_parent()

    def test_registry_acceptance_rejects_fallback_wrong_checksum_and_staging_sources(self):
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary) / "cargo"
            versions = {name: "0.1.0" for name in packages.DIRECTORIES}
            checksums = {name: "a" * 64 for name in versions}
            registry = "file:///owned/index"
            nodes = [{"name": name, "version": version, "source": "registry+" + registry, "id": name,
                      "manifest_path": str(home / "registry/src/private" / name / "Cargo.toml")}
                     for name, version in versions.items()]
            metadata = {"packages": nodes, "resolve": {"nodes": [{"id": name} for name in versions]}}
            lock = {"package": [{"name": name, "version": version, "source": "registry+" + registry, "checksum": checksums[name]}
                                for name, version in versions.items()]}
            packages.check_registry_graph(metadata, lock, registry, versions, checksums, home)
            for field, value in [("source", "registry+" + packages.PUBLIC_INDEX), ("manifest_path", str(Path(temporary) / "staging/Cargo.toml"))]:
                broken = copy.deepcopy(metadata)
                broken["packages"][0][field] = value
                with self.assertRaisesRegex(ValueError, "registry|source"):
                    packages.check_registry_graph(broken, lock, registry, versions, checksums, home)
            lock["package"][0]["checksum"] = "b" * 64
            with self.assertRaisesRegex(ValueError, "checksum|lock"):
                packages.check_registry_graph(metadata, lock, registry, versions, checksums, home)

    def test_staging_keeps_code_private_features_and_exact_edges(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = fixture(Path(temporary))
            before = {name: (root / relative / "Cargo.toml").read_bytes() for name, relative in packages.DIRECTORIES.items()}
            versions = packages.stage_manifests(root, "file:///local/index")
            for name, relative in packages.DIRECTORIES.items():
                old = tomllib.loads(before[name].decode())
                data = tomllib.loads((root / relative / "Cargo.toml").read_text())
                self.assertIs(data["package"]["publish"], False)
                self.assertEqual(data.get("features"), old.get("features"))
                self.assertEqual(data["package"]["version"], versions[name])
                for table in [data, *data.get("target", {}).values()]:
                    for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
                        for key, entry in table.get(kind, {}).items():
                            actual = entry.get("package", key) if isinstance(entry, dict) else key
                            if actual in versions:
                                self.assertEqual(entry["version"], "=" + versions[actual])
                                self.assertEqual(entry["registry"], "telegram-private")
            tl = tomllib.loads((root / "vendor/grammers-tl-types/Cargo.toml").read_text())
            self.assertTrue({"LICENSE-APACHE", "PATCH.md", "tg-decode.patch"} <= set(tl["package"]["include"]))
            self.assertEqual((root / "vendor/grammers-tl-types/src/lib.rs").read_bytes(), b"pub fn fixture() {}\n")
            self.assertEqual((ROOT / "Cargo.toml").read_bytes(), before["tg"])
            self.assertFalse((root / "vendor/grammers-tl-types/Cargo.toml.orig").exists())
            self.assertEqual((root / "vendor/grammers-tl-types/Cargo.upstream.toml").read_bytes(),
                             (ROOT / "vendor/grammers-tl-types/Cargo.toml.orig").read_bytes())

    def test_index_keeps_dependency_aliases_targets_kinds_and_sources(self):
        data = {"package": {"name": "x", "version": "0.1.0", "rust-version": "1.89"},
                "dependencies": {"alias": {"package": "rmcp", "version": "=3.11.0-tg.1", "registry-index": "file:///index", "optional": True}},
                "target": {"cfg(windows)": {"build-dependencies": {"serde": {"version": "1", "default-features": False}}}},
                "features": {"mcp": ["dep:alias"]}}
        entry = packages.index_entry(data, "a" * 64, "file:///index")
        self.assertEqual(entry["deps"][0]["name"], "alias")
        self.assertEqual(entry["deps"][0]["package"], "rmcp")
        self.assertIsNone(entry["deps"][0]["registry"])
        self.assertTrue(entry["deps"][0]["optional"])
        public = entry["deps"][1]
        self.assertEqual((public["kind"], public["target"], public["registry"]), ("build", "cfg(windows)", packages.PUBLIC_INDEX))

        # Cargo normalizes table order; dependency semantics must not depend on it.
        reordered = copy.deepcopy(data)
        data["dependencies"]["first"] = {"version": "1"}
        reordered["dependencies"] = {"first": {"version": "1"}, **reordered["dependencies"]}
        self.assertEqual(packages.index_entry(data, "a" * 64, "file:///index"),
                         packages.index_entry(reordered, "a" * 64, "file:///index"))
        self.assertEqual(entry["features2"], data["features"])
        self.assertEqual(entry["rust_version"], "1.89")

    def test_normalized_archive_rejects_public_fallback_and_changed_source(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = fixture(Path(temporary) / "source")
            versions = packages.stage_manifests(root, "file:///local/index")
            directory = root / "tg-agent"
            manifest = ('[package]\nname="tg-agent"\nversion="0.1.0-alpha.1"\npublish=false\nlicense-file="LICENSE"\n'
                        '[dependencies.rmcp]\nversion="=3.11.0-tg.1"\nregistry-index="file:///local/index"\n')
            output = Path(temporary) / "agent.crate"
            # Keep this synthetic archive's selected manifest consistent with its
            # source; actual full-manifest semantics are covered by the rehearsal.
            (directory / "Cargo.toml").write_text(manifest.replace('license-file="LICENSE"', 'license-file="../LICENSE"'))
            (directory / "tests").mkdir()
            (directory / "tests/.gitignore").write_text("node_modules")
            archive(output, directory, manifest)
            packages.inspect_package(output, directory, versions, "file:///local/index")
            bad = manifest.replace('registry-index="file:///local/index"', 'registry-index="https://github.com/rust-lang/crates.io-index"')
            archive(output, directory, bad)
            with self.assertRaisesRegex(ValueError, "registry|hardened"):
                packages.inspect_package(output, directory, versions, "file:///local/index")
            archive(output, directory, manifest)
            (directory / "src/lib.rs").write_text("changed")
            with self.assertRaisesRegex(ValueError, "bytes|content"):
                packages.inspect_package(output, directory, versions, "file:///local/index")
            (directory / "src/lib.rs").write_bytes(b"pub fn fixture() {}\n")
            (directory / "tests/required.rs").write_text("required source")
            with self.assertRaisesRegex(ValueError, "omitted.*required.rs"):
                packages.inspect_package(output, directory, versions, "file:///local/index")

    def test_public_cache_authenticates_bytes_and_copies_no_credentials(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, caller, isolated = root / "source", root / "caller", root / "isolated"
            source.mkdir()
            content = b"public archive fixture"
            checksum = hashlib.sha256(content).hexdigest()
            (source / "Cargo.lock").write_text('[version]\n\n[[package]]\nname="serde"\nversion="1.0.0"\nsource="registry+' + packages.PUBLIC_INDEX + '"\nchecksum="' + checksum + '"\n')
            cache = caller / "registry/cache" / packages.PUBLIC_CACHE
            index = caller / "registry/index" / packages.PUBLIC_CACHE / ".cache/se/rd"
            cache.mkdir(parents=True)
            index.mkdir(parents=True)
            (cache / "serde-1.0.0.crate").write_bytes(content)
            (index / "serde").write_bytes(b"public sparse record")
            (caller / "credentials.toml").write_text("private sentinel")
            (caller / "config.toml").write_text("private sentinel")
            packages.copy_public_cache(source, isolated, caller)
            self.assertEqual((isolated / "registry/cache" / packages.PUBLIC_CACHE / "serde-1.0.0.crate").read_bytes(), content)
            self.assertFalse((isolated / "credentials.toml").exists())
            self.assertFalse((isolated / "config.toml").exists())
            (cache / "serde-1.0.0.crate").write_bytes(b"corrupt")
            with self.assertRaisesRegex(ValueError, "checksum"):
                packages.copy_public_cache(source, root / "another", caller)


if __name__ == "__main__":
    unittest.main()
