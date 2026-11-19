"""Run locked native toolchain/feature checks, retaining explicit pass/fail evidence."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
CRATES = (".", "tg-agent", "tg-mtproto")


def cases(source, toolchain, target, suite):
    result = []
    def add(name, operation, *arguments):
        result.append((name, ["cargo", f"+{toolchain}", operation, "--locked", "--target-dir", str(target), *arguments]))
    if suite in ("all", "tests"):
        add("workspace-default", "test", "--workspace", "--all-targets")
        add("workspace-all", "test", "--workspace", "--all-features", "--all-targets")
        add("workspace-doc", "test", "--workspace", "--all-features", "--doc")
    if suite in ("all", "features"):
        metadata = json.loads(subprocess.check_output(
            ["cargo", f"+{toolchain}", "metadata", "--locked", "--no-deps", "--format-version", "1"],
            cwd=source, stderr=subprocess.PIPE))
        packages = {Path(package["manifest_path"]).resolve(): package for package in metadata["packages"]}
        for relative in CRATES:
            package = packages[(source / relative / "Cargo.toml").resolve()]
            name = package["name"]
            # Cargo includes implicit optional-dependency features and applies
            # dep: suppression, target tables and workspace inheritance itself.
            features = sorted(set(package["features"]) - {"default"})
            base = ["--package", name, "--lib", "--no-default-features"]
            add(f"{name}-minimal", "check", *base)
            for feature in features:
                add(f"{name}-{feature}", "check", *base, "--features", feature)
            add(f"{name}-all", "check", *base, "--all-features")
            if name == "tg-agent":
                for adapter in ("host", "cli", "mcp"):
                    if {adapter, "mtproto"} <= set(features):
                        add(f"{name}-{adapter}-mtproto", "check", *base, "--features", f"{adapter},mtproto")
    return result


def new_evidence(target):
    parent = target.with_name(target.name + "-evidence")
    parent.mkdir(parents=True, exist_ok=True)
    return Path(tempfile.mkdtemp(prefix="run-", dir=parent))


def guard(source):
    if shutil.disk_usage(source).free < 20 * 1024**3:
        raise ValueError("less than 20 GiB free; clean completed build caches first")
    size = 0
    for directory, _, names in os.walk(source / "target", followlinks=False):
        for name in names:
            path = Path(directory) / name
            try:
                if not path.is_symlink():
                    size += path.stat().st_size
            except FileNotFoundError:
                pass  # Another independent target can finish cleanup during inspection.
    if size > 20 * 1024**3:
        raise ValueError("target exceeds 20 GiB; clean completed build caches first")


def execute(commands, evidence, context, source):
    evidence.mkdir(parents=True, exist_ok=True)
    summary = dict(context, status="running", cases=[], started_utc=datetime.now(timezone.utc).isoformat())
    def save():
        (evidence / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    save()  # Invalidate any previous success before starting work.
    try:
        for index, (name, command) in enumerate(commands, 1):
            guard(source)
            print(f"[{index}/{len(commands)}] {name}", flush=True)
            log = f"{index:02}.log"
            case = {"name": name, "command": command, "exit_code": None, "log": log, "status": "running"}
            summary["cases"].append(case)
            save()
            with (evidence / log).open("wb") as stream:
                result = subprocess.run(command, cwd=source, stdout=stream, stderr=subprocess.STDOUT, check=False)
            case.update(exit_code=result.returncode, status="passed" if result.returncode == 0 else "failed")
            if result.returncode:
                summary["status"] = "failed"
                save()
                print(f"failed: {name}; see {evidence / log}", file=sys.stderr)
                return 1
            save()
        guard(source)
        summary["status"] = "passed"
        summary["finished_utc"] = datetime.now(timezone.utc).isoformat()
        save()
        return 0
    except BaseException as error:
        summary["status"] = "interrupted"
        summary["failure_type"] = type(error).__name__
        if summary["cases"] and summary["cases"][-1]["status"] == "running":
            summary["cases"][-1]["status"] = "interrupted"
        save()
        raise


def git_context(source):
    root = subprocess.run(
        ["git", "--no-replace-objects", "rev-parse", "--show-toplevel"],
        cwd=source, capture_output=True, encoding="utf-8", check=False,
    )
    if root.returncode or Path(root.stdout.rstrip("\r\n")).resolve() != source.resolve():
        return None, None
    revision = subprocess.run(
        ["git", "--no-replace-objects", "rev-parse", "HEAD"],
        cwd=source, capture_output=True, text=True, check=False,
    )
    status = subprocess.run(
        ["git", "--no-replace-objects", "-c", "core.fsmonitor=false", "status", "--porcelain", "--ignore-submodules=all"],
        cwd=source, capture_output=True, check=False,
    )
    return (revision.stdout.strip() if revision.returncode == 0 else None,
            bool(status.stdout) if status.returncode == 0 else None)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--toolchain", required=True, help="stable or an installed exact version, e.g. 1.89.0")
    parser.add_argument("--suite", choices=("all", "tests", "features"), default="all")
    args = parser.parse_args()
    if not re.fullmatch(r"stable|[0-9]+\.[0-9]+\.[0-9]+", args.toolchain):
        parser.error("toolchain must be stable or an exact release version")
    target = ROOT / "target" / f"compat-{args.toolchain}"
    evidence = new_evidence(target)
    print(f"evidence: {evidence}", flush=True)
    # Unique per-run evidence prevents overlapping CLI runs from mixing logs.
    (evidence / "summary.json").write_text('{"status":"preflight"}\n', encoding="utf-8")
    dispatched = False
    context = {"toolchain": args.toolchain, "suite": args.suite,
               "platform": platform.platform(), "source": str(ROOT)}
    try:
        versions = {tomllib.loads((ROOT / relative / "Cargo.toml").read_text(encoding="utf-8"))["package"].get("rust-version") for relative in CRATES}
        if len(versions) != 1 or None in versions:
            raise ValueError("all three libraries must declare one verified rust-version")
        context["rustc"] = subprocess.check_output(["rustup", "run", args.toolchain, "rustc", "-Vv"], text=True)
        commit, dirty = git_context(ROOT)
        context.update(commit=commit, dirty=dirty,
                       declared_msrv=next(iter(versions)),
                       lock_sha256=hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest())
        commands = cases(ROOT, args.toolchain, target, args.suite)
        dispatched = True
        return execute(commands, evidence, context, ROOT)
    except (OSError, ValueError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        if not dispatched:
            failure = dict(context, status="failed-preflight", failure_type=type(error).__name__)
            if isinstance(error, subprocess.CalledProcessError):
                failure["preflight_error"] = {"command": error.cmd, "exit_code": error.returncode}
                for channel, data in (("stdout", error.stdout), ("stderr", error.stderr)):
                    name = f"preflight.{channel}.log"
                    (evidence / name).write_bytes(data.encode("utf-8") if isinstance(data, str) else data or b"")
                    failure["preflight_error"][channel + "_log"] = name
            (evidence / "summary.json").write_text(json.dumps(failure, indent=2) + "\n", encoding="utf-8")
        print(f"compatibility failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
