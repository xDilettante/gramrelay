"""Rehearse closed Cargo packages in an owned file registry; never publish."""
import argparse
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zlib

import check_compat
import check_external
import source_bundle

ROOT = Path(__file__).resolve().parents[1]
DIRECTORIES = check_external.LIBRARIES | {name: f"vendor/{name}" for name in check_external.SDK}
PUBLIC_INDEX = "https://github.com/rust-lang/crates.io-index"
PUBLIC_CACHE = "index.crates.io-1949cf8c6b5b557f"
KINDS = ("dependencies", "dev-dependencies", "build-dependencies")
REGISTRY = "telegram-private"


def dependency_tables(data):
    for target, table in [(None, data), *data.get("target", {}).items()]:
        for key, kind in zip(KINDS, ("normal", "dev", "build")):
            for name, entry in table.get(key, {}).items():
                yield target, kind, name, {"version": entry} if isinstance(entry, str) else entry


def stage_manifests(source, registry):
    check_external.check_manifests(source)
    original = {name: (source / relative / "Cargo.toml").read_text(encoding="utf-8")
                for name, relative in DIRECTORIES.items()}
    versions = {name: tomllib.loads(text)["package"]["version"] + ("-tg.1" if name in check_external.SDK else "")
                for name, text in original.items()}
    for name, text in original.items():
        expected = copy.deepcopy(tomllib.loads(text))
        upstream_manifest = source / DIRECTORIES[name] / "Cargo.toml.orig"
        if upstream_manifest.is_file():
            destination = upstream_manifest.with_name("Cargo.upstream.toml")
            if destination.exists():
                raise ValueError("upstream manifest asset already exists")
            upstream_manifest.rename(destination)
        expected["package"]["version"] = versions[name]
        for _, _, key, entry in dependency_tables(expected):
            actual = entry.get("package", key)
            if actual in versions:
                entry.update(version="=" + versions[actual], registry=REGISTRY)
        if name == "tg":
            expected["workspace"] = {"members": [value for value in DIRECTORIES.values() if value],
                                     "default-members": [".", "tg-agent", "tg-mtproto"], "resolver": "3"}
            section = "[workspace]\n" + "\n".join(f"{key} = {json.dumps(value)}" for key, value in expected["workspace"].items()) + "\n\n"
            text, count = re.subn(r"(?ms)^\[workspace\]\n.*?(?=^\[|\Z)", lambda _: section, text)
            if count != 1:
                raise ValueError("unsupported workspace manifest layout")
        if "include" in expected["package"]:
            assets = sorted(path.name for path in (source / DIRECTORIES[name]).iterdir()
                            if path.is_file() and (path.name.startswith("LICENSE") or path.name in ("PATCH.md", "Cargo.upstream.toml") or path.suffix == ".patch"))
            assets += [folder + "/**" for folder in ("examples", "tests", "benches") if (source / DIRECTORIES[name] / folder).is_dir()]
            expected["package"]["include"] += [asset for asset in assets if asset not in expected["package"]["include"]]
            text, count = re.subn(r"(?ms)^include\s*=\s*\[.*?\]", lambda _: "include = " + json.dumps(expected["package"]["include"]), text)
            if count != 1:
                raise ValueError("unsupported include manifest layout")
        section, lines = "", []
        for line in text.splitlines(keepends=True):
            header = re.match(r"^\[([^\[].*?)\]\s*$", line)
            if header:
                section = header[1]
            if section == "package" and re.match(r"^version\s*=", line):
                line = f"version = {json.dumps(versions[name])}\n"
            table = re.search(r"(?:^|\.)(?:dependencies|dev-dependencies|build-dependencies)\.([A-Za-z0-9_-]+)$", section)
            if table and table[1] in versions and re.match(r"^path\s*=", line):
                line += f'version = "={versions[table[1]]}"\nregistry = "{REGISTRY}"\n'
            elif section in KINDS or any(section.endswith("." + kind) for kind in KINDS):
                key = re.match(r"^([A-Za-z0-9_-]+)\s*=", line)
                if key and key[1] in versions:
                    line, count = re.subn(r'\bpath\s*=\s*"[^"]*"', lambda match: match[0] + f', version = "={versions[key[1]]}", registry = "{REGISTRY}"', line)
                    if count != 1:
                        raise ValueError("unsupported internal dependency layout")
            lines.append(line)
        rewritten = "".join(lines)
        if tomllib.loads(rewritten) != expected:
            raise ValueError(f"unexpected staging manifest change: {name}")
        (source / DIRECTORIES[name] / "Cargo.toml").write_text(rewritten, encoding="utf-8", newline="\n")
    config = source / ".cargo/config.toml"
    config.parent.mkdir(exist_ok=True)
    with config.open("a", encoding="utf-8", newline="\n") as stream:
        stream.write(f"\n[registries.{REGISTRY}]\nindex = {json.dumps(registry)}\n")
    return versions


def index_path(name):
    name = name.lower()
    return f"1/{name}" if len(name) == 1 else f"2/{name}" if len(name) == 2 else f"3/{name[0]}/{name}" if len(name) == 3 else f"{name[:2]}/{name[2:4]}/{name}"


def index_entry(data, checksum, registry):
    deps = []
    for target, kind, key, entry in dependency_tables(data):
        url = entry.get("registry-index", registry if entry.get("registry") == REGISTRY else PUBLIC_INDEX)
        if url not in (registry, PUBLIC_INDEX):
            raise ValueError("unexpected dependency registry")
        deps.append({"name": key, "package": entry.get("package"), "req": entry["version"],
                     "features": entry.get("features", []), "optional": entry.get("optional", False),
                     "default_features": entry.get("default-features", True), "target": target,
                     "kind": kind, "registry": None if url == registry else PUBLIC_INDEX})
    deps.sort(key=lambda entry: (entry["target"] or "", entry["kind"], entry["name"]))
    package = data["package"]
    result = {"name": package["name"], "vers": package["version"], "deps": deps, "cksum": checksum,
              "features": {}, "features2": data.get("features", {}), "v": 2, "yanked": False}
    for source_key, key in [("rust-version", "rust_version"), ("links", "links")]:
        if source_key in package:
            result[key] = package[source_key]
    return result


def inspect_package(archive, directory, versions, registry):
    compressed = source_bundle.read_bounded(archive, source_bundle.MAX_ARCHIVE)
    decoder = zlib.decompressobj(31)
    raw = decoder.decompress(compressed, source_bundle.MAX_EXPANDED + 1)
    if len(raw) > source_bundle.MAX_EXPANDED or not decoder.eof or decoder.unused_data:
        raise ValueError("invalid or excessive package compression")
    staged = tomllib.loads((directory / "Cargo.toml").read_text(encoding="utf-8"))
    name, version = staged["package"]["name"], staged["package"]["version"]
    prefix, files = f"{name}-{version}/", {}
    with tarfile.open(fileobj=io.BytesIO(raw), mode="r:") as stream:
        for member in stream:
            if len(files) >= 10000 or not member.isfile() or not member.name.startswith(prefix) or member.size > source_bundle.MAX_FILE:
                raise ValueError("unsupported package member")
            source_bundle.parts(member.name)
            relative = member.name[len(prefix):]
            if relative in files:
                raise ValueError("duplicate package member")
            files[relative] = stream.extractfile(member).read(source_bundle.MAX_FILE + 1)
    data = tomllib.loads(files["Cargo.toml"].decode())
    if (data["package"]["name"] != name or data["package"]["version"] != versions[name]
            or data["package"].get("publish") is not False
            or any(key in data for key in ("workspace", "patch", "replace")) or "Cargo.lock" in files):
        raise ValueError("normalized private package policy violated")
    for _, _, key, entry in dependency_tables(data):
        actual = entry.get("package", key)
        if any(field in entry for field in ("path", "git", "workspace", "registry")):
            raise ValueError("non-registry normalized dependency")
        if actual in versions and (entry.get("version") != "=" + versions[actual] or entry.get("registry-index") != registry):
            raise ValueError("hardened registry dependency changed")
    # Compare the whole dependency/feature contract as well as protected edges.
    if index_entry(data, "", registry) != index_entry(staged, "", registry):
        raise ValueError("normalized dependency/feature contract changed")
    for field in ("license", "rust-version"):
        if data["package"].get(field) != staged["package"].get(field):
            raise ValueError("package license/compiler metadata changed")
    expected_build = staged["package"].get("build", "build.rs" if (directory / "build.rs").is_file() else False)
    if (data["package"].get("build", False) != expected_build
            or data["package"].get("edition", "2015") != staged["package"].get("edition", "2015")):
        raise ValueError("package build/edition behavior changed")
    license_file = staged["package"].get("license-file")
    if license_file and files.get(data["package"].get("license-file")) != (directory / license_file).read_bytes():
        raise ValueError("private license content changed")
    for relative, content in files.items():
        if relative == "Cargo.toml":
            continue
        original = directory / ("Cargo.toml" if relative == "Cargo.toml.orig" else relative)
        if license_file and relative == data["package"].get("license-file"):
            original = directory / license_file
        if not original.is_file() or content != source_bundle.read_bounded(original, source_bundle.MAX_FILE):
            raise ValueError(f"package content bytes changed: {name}/{relative}")
    mandatory = {path.relative_to(directory).as_posix() for path in (directory / "src").rglob("*") if path.is_file()}
    mandatory |= {path.name for path in directory.iterdir() if path.is_file() and (
        path.name.startswith("LICENSE") or path.name in ("build.rs", "PATCH.md", "Cargo.upstream.toml") or path.suffix == ".patch")}
    mandatory |= {path.relative_to(directory).as_posix() for path in (directory / "tl").rglob("*") if path.is_file()}
    for folder in ("examples", "tests", "benches"):
        # Cargo never packages its VCS selection policies; the complete source
        # ZIP retains these. All actual source/fixture assets remain mandatory.
        mandatory |= {path.relative_to(directory).as_posix() for path in (directory / folder).rglob("*")
                      if path.is_file() and path.name not in (".gitignore", ".gitattributes")}
    for kind in ("lib", "bin", "example", "test", "bench"):
        targets = [data[kind]] if kind == "lib" and kind in data else data.get(kind, [])
        mandatory |= {target["path"] for target in targets if "path" in target}
    if not mandatory <= files.keys() or "Cargo.toml.orig" not in files:
        raise ValueError(f"package omitted source/license/patch assets: {name}: {sorted(mandatory - files.keys())[:16]}")
    return data


def copy_public_cache(source, isolated, caller):
    lock = tomllib.loads((source / "Cargo.lock").read_text(encoding="utf-8"))
    index = isolated / "registry/index" / PUBLIC_CACHE
    index.mkdir(parents=True, exist_ok=True)
    (index / "config.json").write_text(json.dumps({"dl": "https://static.crates.io/crates", "api": "https://crates.io"}), encoding="utf-8")
    for package in lock["package"]:
        if "source" not in package:
            continue
        if package["source"] != "registry+" + PUBLIC_INDEX:
            raise ValueError("unexpected root lock registry source")
        relative = Path("registry/cache") / PUBLIC_CACHE / f"{package['name']}-{package['version']}.crate"
        content = source_bundle.read_bounded(caller / relative, source_bundle.MAX_ARCHIVE)
        if hashlib.sha256(content).hexdigest() != package["checksum"]:
            raise ValueError("public archive checksum mismatch")
        destination = isolated / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(content)
        relative = Path(".cache") / index_path(package["name"])
        content = source_bundle.read_bounded(caller / "registry/index" / PUBLIC_CACHE / relative, source_bundle.MAX_FILE)
        destination = index / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(content)


def check_registry_graph(metadata, lock, registry, versions, checksums, home):
    if any(package["name"] == "chrono" for package in [*metadata["packages"], *lock["package"]]):
        raise ValueError("chrono returned to the registry consumer")
    resolved = {node["id"] for node in metadata["resolve"]["nodes"]}
    for name, version in versions.items():
        matches = [package for package in metadata["packages"] if package["name"] == name]
        entries = [package for package in lock["package"] if package["name"] == name]
        if len(matches) != 1 or len(entries) != 1:
            raise ValueError(f"missing/duplicate hardened registry package: {name}")
        package, entry = matches[0], entries[0]
        if (package["version"] != version or entry["version"] != version
                or package["source"] != "registry+" + registry or entry["source"] != "registry+" + registry
                or package["id"] not in resolved
                or not Path(package["manifest_path"]).resolve().is_relative_to((home / "registry/src").resolve())):
            raise ValueError(f"unexpected private registry source/version: {name}")
        if entry["checksum"] != checksums[name]:
            raise ValueError(f"private registry lock checksum mismatch: {name}")


def package_models(metadata):
    """Compare Cargo's effective targets, including automatic discovery/build.rs."""
    models = {}
    for package in metadata["packages"]:
        if package["name"] not in DIRECTORIES:
            continue
        directory = Path(package["manifest_path"]).resolve().parent
        targets = []
        for target in package["targets"]:
            relative = Path(target["src_path"]).resolve().relative_to(directory).as_posix()
            targets.append(target | {"src_path": relative})
        models[package["name"]] = {key: package.get(key) for key in ("edition", "rust_version", "links", "publish", "features")}
        models[package["name"]]["targets"] = sorted(targets, key=lambda target: (target["name"], target["kind"], target["src_path"]))
    return models


def choose_scratch_parent():
    # Stat only: do not read caller configuration. CARGO_HOME does not disable
    # Cargo's search for ancestor .cargo/config[.toml], especially under Windows TEMP.
    for directory in (Path(tempfile.gettempdir()).resolve(), ROOT.parent):
        if not any((parent / name).exists()
                   for parent in (directory, *directory.parents)
                   for name in (".cargo/config", ".cargo/config.toml", "Cargo.toml")):
            return directory
    raise ValueError("no neutral scratch parent without inherited Cargo configuration/workspace")


def isolated_environment(home):
    # Preserve compiler/OS discovery, never inherit service tokens, Cargo
    # credentials/config overrides or arbitrary compiler wrappers/flags.
    names = {"PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT", "TEMP", "TMP", "USERPROFILE", "HOME",
             "LOCALAPPDATA", "APPDATA", "PROGRAMFILES", "PROGRAMFILES(X86)", "PROGRAMDATA", "NUMBER_OF_PROCESSORS",
             "RUSTUP_HOME", "INCLUDE", "LIB", "LIBPATH", "VCTOOLSINSTALLDIR", "VCINSTALLDIR", "VSINSTALLDIR"}
    return {key: value for key, value in os.environ.items() if key.upper() in names} | {
        "CARGO_HOME": str(home), "CARGO_NET_OFFLINE": "true", "CARGO_BUILD_JOBS": "2",
        "CARGO_INCREMENTAL": "0", "CARGO_PROFILE_DEV_DEBUG": "line-tables-only", "CARGO_PROFILE_TEST_DEBUG": "line-tables-only",
        "CARGO_HTTP_PROXY": "http://127.0.0.1:9", "CARGO_HTTP_TIMEOUT": "5", "CARGO_NET_RETRY": "0",
        "RUSTUP_AUTO_INSTALL": "0",
        "GIT_ALLOW_PROTOCOL": "file", "GIT_TERMINAL_PROMPT": "0", "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
    }


def rehearse(toolchain, output):
    check_compat.guard(ROOT)
    archive, manifest = source_bundle.build(ROOT, ROOT / "target/dist")
    document, files = source_bundle.verify_bundle(source_bundle.read_bounded(archive, source_bundle.MAX_ARCHIVE),
                                                 source_bundle.read_bounded(manifest, source_bundle.MAX_MANIFEST))
    output.mkdir(parents=True, exist_ok=True)
    candidate = Path(tempfile.mkdtemp(prefix=document["commit"][:12] + "-", dir=output)).resolve()
    evidence = candidate / "evidence"
    evidence.mkdir()
    summary = {"status": "running", "source_commit": document["commit"], "source_archive_sha256": document["archive_sha256"],
               "candidate": str(candidate), "toolchain": toolchain, "commands": []}
    scratch = None
    cargo = [shutil.which("cargo") or "cargo", "+" + toolchain]
    uncertain_children = False

    def save():
        (evidence / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")

    def run(label, command, cwd, *, bootstrap=False, capture=False):
        nonlocal uncertain_children
        if label != "clean":
            check_compat.guard(ROOT)
        record = {"name": label, "command": command, "cwd": str(cwd), "exit_code": None}
        summary["commands"].append(record)
        save()
        print(label, flush=True)
        with (evidence / (label + ".log")).open("wb") as log, (evidence / (label + ".stdout")).open("wb") as stdout:
            try:
                result = subprocess.run(command, cwd=cwd, env=env | ({"CARGO_NET_OFFLINE": "false"} if bootstrap else {}),
                                        stdout=stdout if capture else log, stderr=log if capture else subprocess.STDOUT, timeout=1800)
            except (subprocess.TimeoutExpired, KeyboardInterrupt):
                uncertain_children = True
                summary["retained_scratch"] = str(scratch)
                raise
        record["exit_code"] = result.returncode
        if result.returncode < 0 or result.returncode >= 2**31:
            uncertain_children = True
            summary["retained_scratch"] = str(scratch)
        save()
        if result.returncode:
            raise ValueError(f"{label} failed; see {evidence / (label + '.log')}")
        return source_bundle.read_bounded(evidence / (label + ".stdout"), source_bundle.MAX_ARCHIVE) if capture else None

    try:
        scratch_parent = choose_scratch_parent()
        scratch = Path(tempfile.mkdtemp(prefix="tg-private-packages-", dir=scratch_parent)).resolve()
        home, source = scratch / "cargo", scratch / "source"
        if os.name != "nt":
            scratch.chmod(0o700)
        env = isolated_environment(home)
        source_bundle.materialize(files, source)
        copy_public_cache(source, home, Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")))
        for script in ("verify_grammers.py", "verify_rmcp.py"):
            run(script.removesuffix(".py"), [sys.executable, str(source / "tools" / script), "check"], source)
        index, crates = candidate / "index", candidate / "crates"
        index.mkdir()
        crates.mkdir()
        registry = index.as_uri()
        versions = stage_manifests(source, registry)
        summary.update(versions=versions, registry=registry)
        run("compiler", [shutil.which("rustc") or "rustc", "+" + toolchain, "--version", "--verbose"], source)
        staged_metadata = json.loads(run("staged-metadata", [*cargo, "metadata", "--no-deps", "--offline", "--format-version", "1"], source, capture=True))
        models = package_models(staged_metadata)
        if set(models) != set(DIRECTORIES):
            raise ValueError("staged Cargo model is incomplete")
        run("package", [*cargo, "package", "--workspace", "--registry", REGISTRY, "--no-verify", "--exclude-lockfile", "--offline",
                        "--target-dir", str(candidate / "build")], source)
        checksums = {}
        for name, relative in DIRECTORIES.items():
            filename = f"{name}-{versions[name]}.crate"
            packaged = candidate / "build/package" / filename
            data = inspect_package(packaged, source / relative, versions, registry)
            checksums[name] = hashlib.sha256(source_bundle.read_bounded(packaged, source_bundle.MAX_ARCHIVE)).hexdigest()
            shutil.copyfile(packaged, crates / filename)
            entry = index / index_path(name)
            entry.parent.mkdir(parents=True, exist_ok=True)
            entry.write_text(json.dumps(index_entry(data, checksums[name], registry)) + "\n", encoding="utf-8")
        (index / "config.json").write_text(json.dumps({"dl": crates.as_uri() + "/{crate}-{version}.crate"}), encoding="utf-8")
        run("index-init", ["git", "init", "--quiet"], index)
        run("index-add", ["git", "add", "."], index)
        run("index-commit", ["git", "-c", "user.name=Telegram Rust local rehearsal", "-c", "user.email=local@invalid",
                             "-c", "commit.gpgsign=false", "commit", "--quiet", "-m", "Local private candidate"], index)
        bootstrap = scratch / "bootstrap"
        (bootstrap / ".cargo").mkdir(parents=True)
        config = f'[registries.{REGISTRY}]\nindex = {json.dumps(registry)}\n'
        (bootstrap / ".cargo/config.toml").write_text(config, encoding="utf-8")
        # No Cargo.toml in this directory or its parents: info does not resolve
        # the real consumer or refresh the unrelated public sparse registry.
        for name, version in versions.items():
            run("bootstrap-" + name, [*cargo, "info", f"{name}@{version}", "--registry", REGISTRY], bootstrap, bootstrap=True)
        consumer = scratch / "consumer"
        (consumer / "src").mkdir(parents=True)
        (consumer / ".cargo").mkdir()
        text = (source / "tools/fixtures/external-consumer/Cargo.toml.in").read_text(encoding="utf-8")
        for name in check_external.LIBRARIES:
            text = text.replace("@" + name + "@", f'version = "={versions[name]}", registry = "{REGISTRY}"')
        text = text.replace("@agent-test-kit@", ', "mtproto-test-kit"').replace("@mt-test-kit@", '"test-kit"')
        (consumer / "Cargo.toml").write_text(text, encoding="utf-8")
        (consumer / ".cargo/config.toml").write_text(config, encoding="utf-8")
        shutil.copyfile(source / "tools/fixtures/external-consumer/main.rs", consumer / "src/main.rs")
        metadata = json.loads(run("metadata", [*cargo, "metadata", "--offline", "--format-version", "1"], consumer, capture=True))
        lock_bytes = (consumer / "Cargo.lock").read_bytes()
        lock = tomllib.loads(lock_bytes.decode())
        check_registry_graph(metadata, lock, registry, versions, checksums, home)
        if package_models(metadata) != models:
            raise ValueError("normalized registry targets/features differ from staging Cargo model")
        (evidence / "consumer.Cargo.lock").write_bytes(lock_bytes)
        summary.update(checksums=checksums, packages=len(lock["package"]), consumer_lock_sha256=hashlib.sha256(lock_bytes).hexdigest())
        run("runtime", [*cargo, "run", "--locked", "--offline", "--target-dir", str(candidate / "build")], consumer)
        # Cargo must actually execute the public fixture, not merely compile it.
        runtime = source_bundle.read_bounded(evidence / "runtime.log", source_bundle.MAX_ARCHIVE)
        if b"external fixture: Bot message/callback/upload/Unknown, MCP discovery, MT real-TL and joined close passed" not in runtime:
            raise ValueError("public registry runtime acceptance marker missing")
        summary["status"] = "verified"
    except BaseException as error:
        summary.update(status="failed", error=type(error).__name__ + ": " + str(error))
        raise
    finally:
        save()
        if scratch is not None and not uncertain_children:
            try:
                if (candidate / "build/CACHEDIR.TAG").is_file():
                    run("clean", [*cargo, "clean", "--target-dir", str(candidate / "build"), "--profile", "dev"], source)
                if not uncertain_children:
                    # This is the exact freshly created private temp directory.
                    if scratch.parent != scratch_parent or not scratch.name.startswith("tg-private-packages-") or scratch.is_symlink():
                        raise ValueError("unexpected scratch cleanup target")
                    shutil.rmtree(scratch)
            except BaseException as error:
                summary.update(status="failed", cleanup_error=type(error).__name__ + ": " + str(error), retained_scratch=str(scratch))
                save()
                raise
        save()
    try:
        check_compat.guard(ROOT)
    except BaseException:
        summary["status"] = "failed"
        save()
        raise
    summary["status"] = "passed"
    save()
    print(f"private registry acceptance passed: {candidate}", flush=True)
    return candidate


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--toolchain", default="stable")
    parser.add_argument("--output-dir", type=Path, default=ROOT / "target/private-packages")
    args = parser.parse_args()
    try:
        rehearse(args.toolchain, args.output_dir.resolve())
        return 0
    except (OSError, ValueError, subprocess.SubprocessError, tarfile.TarError, zlib.error) as error:
        print(f"private packaging failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
