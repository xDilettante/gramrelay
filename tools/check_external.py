"""Check the hardened graph from a separate Cargo consumer, optionally run it."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
SDK = {name: "0.12.0" for name in (
    "grammers-session", "grammers-mtsender", "grammers-mtproto", "grammers-tl-types",
)} | {"rmcp": "3.11.0"}
LIBRARIES = {"tg": "", "tg-agent": "tg-agent", "tg-mtproto": "tg-mtproto"}


def check_manifests(source: Path) -> None:
    source = source.resolve()
    expected = {**LIBRARIES, **{name: f"vendor/{name}" for name in SDK}}
    for name, relative in expected.items():
        directory = source / relative
        data = tomllib.loads((directory / "Cargo.toml").read_text(encoding="utf-8"))
        if data["package"].get("publish") is not False or "patch" in data or "replace" in data:
            raise ValueError(f"private source package/override policy violated: {name}")
        tables = [data, *data.get("target", {}).values()]
        for table in tables:
            for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
                for key, dependency in table.get(kind, {}).items():
                    package = dependency.get("package", key) if isinstance(dependency, dict) else key
                    if package not in expected:
                        continue
                    if (not isinstance(dependency, dict) or "path" not in dependency
                            or any(key in dependency for key in ("version", "git", "registry", "registry-index", "workspace"))
                            or (directory / dependency["path"]).resolve() != source / expected[package]):
                        raise ValueError(f"hardened dependency can escape/fall back: {name}/{package}")


def check_graph(metadata: dict, source: Path, *, revision=None) -> None:
    packages = metadata["packages"]
    if any(package["name"] == "chrono" for package in packages):
        raise ValueError("chrono returned to the external graph")
    resolved = {node["id"] for node in metadata["resolve"]["nodes"]}
    expected_source = None if revision is None else f"git+{source.as_uri()}?rev={revision}#{revision}"
    checkout = source
    if revision is not None:
        roots = [package for package in packages if package["name"] == "tg"]
        if len(roots) != 1:
            raise ValueError("missing or duplicate root library")
        checkout = Path(roots[0]["manifest_path"]).resolve().parent
    expected = {**LIBRARIES, **{name: f"vendor/{name}" for name in SDK}}
    for name, relative in expected.items():
        matches = [package for package in packages if package["name"] == name]
        if len(matches) != 1:
            raise ValueError(f"missing or duplicate package: {name}")
        package = matches[0]
        if (package["source"] != expected_source or package["id"] not in resolved
                or Path(package["manifest_path"]).resolve() != checkout / relative / "Cargo.toml"):
            raise ValueError(f"unexpected package source: {name}")
        if name in SDK and package["version"] != SDK[name]:
            raise ValueError(f"unexpected SDK base version: {name}")


def manifest(source: Path, *, test_kit: bool, revision=None) -> str:
    text = (source / "tools/fixtures/external-consumer/Cargo.toml.in").read_text(encoding="utf-8")
    for name, relative in LIBRARIES.items():
        location = (f"path = {json.dumps((source / relative).as_posix())}" if revision is None
                    else f"git = {json.dumps(source.as_uri())}, rev = {json.dumps(revision)}")
        text = text.replace(f"@{name}@", location)
    return text.replace("@agent-test-kit@", ', "mtproto-test-kit"' if test_kit else "").replace(
        "@mt-test-kit@", '"test-kit"' if test_kit else "")


def run(source: Path, target: Path, *, execute: bool, git=False) -> None:
    source, target = source.resolve(), target.resolve()
    check_manifests(source)
    revision = None
    if git:
        if subprocess.check_output(["git", "status", "--porcelain"], cwd=source):
            raise ValueError("Git consumer requires a clean committed source snapshot")
        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True).strip()
    # Let Cargo create/tag its build directory. Precreating it with evidence
    # files makes recent Cargo refuse clean because CACHEDIR.TAG is missing.
    target.parent.mkdir(parents=True, exist_ok=True)
    evidence = target.with_name(target.name + "-evidence")
    evidence.mkdir(parents=True, exist_ok=True)
    if shutil.disk_usage(target.parent).free < 20 * 1024**3:
        raise ValueError("less than 20 GiB free; clean completed build caches first")
    # Owned temp tree is outside the library workspace; no consumer patch table.
    with tempfile.TemporaryDirectory(prefix="telegram external consumer ") as temporary:
        consumer = Path(temporary).resolve()
        if source == consumer or source in consumer.parents:
            raise ValueError("consumer must be outside the source workspace")
        (consumer / "src").mkdir()
        (consumer / "Cargo.toml").write_text(manifest(source, test_kit=execute, revision=revision), encoding="utf-8")
        shutil.copyfile(source / "tools/fixtures/external-consumer/main.rs", consumer / "src/main.rs")
        # Initial local Git checkout may need Cargo's Git-cache bootstrap. This
        # metadata step may refresh the public registry index; run stays offline.
        command = ["cargo", "metadata", *([] if git else ["--offline"]), "--format-version", "1", "--manifest-path", str(consumer / "Cargo.toml")]
        result = subprocess.run(command, cwd=source, capture_output=True, check=False)
        (evidence / "metadata.log").write_bytes(result.stderr)
        if result.returncode:
            raise ValueError("external metadata failed; see sibling *-evidence/metadata.log")
        (evidence / "metadata.json").write_bytes(result.stdout)
        metadata = json.loads(result.stdout)
        check_graph(metadata, source, revision=revision)
        lock = (consumer / "Cargo.lock").read_bytes()
        if any(package["name"] == "chrono" for package in tomllib.loads(lock.decode())["package"]):
            raise ValueError("chrono returned to the external lockfile")
        (evidence / "consumer.Cargo.lock").write_bytes(lock)
        print("external graph: 3 libraries / 5 hardened SDKs, no overrides or chrono", flush=True)
        if execute:
            command = ["cargo", "run", "--locked", "--offline", "--manifest-path", str(consumer / "Cargo.toml"), "--target-dir", str(target)]
            result = subprocess.run(command, cwd=source, check=False)
            if result.returncode:
                raise ValueError("external public-API fixture failed")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=ROOT)
    parser.add_argument("--target-dir", type=Path, default=ROOT / "target/external-consumer")
    parser.add_argument("--run", action="store_true")
    parser.add_argument("--git", action="store_true", help="local Git dependency pinned to the clean source commit")
    args = parser.parse_args()
    try:
        run(args.source, args.target_dir, execute=args.run, git=args.git)
        return 0
    except (OSError, ValueError, KeyError) as error:
        print(f"external consumer check failed: {error}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
