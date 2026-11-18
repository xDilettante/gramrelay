"""Compile and execute reviewed Rust answers against isolated per-task fixtures."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib

import check_compat
import check_external
import source_bundle


ROOT = check_compat.ROOT
EVALS = Path("tools/evals/v1")
DOCS = ("docs/AGENT-INDEX.md", "tools/evals/v1/API-NOTES.md")
AREA_DOCS = {
    "runtime": ("docs/START.md", "examples/polling.rs", "examples/webhook_mock.rs"),
    "agent": ("docs/AGENT-CLI.md", "tg-agent/examples/journal.rs", "tg-agent/examples/authorized_document.rs"),
    "mtproto": ("tg-mtproto/README.md",),
}
MAX_ANSWER = 64 * 1024


def digest(data):
    return hashlib.sha256(data).hexdigest()


def read(path, limit=MAX_ANSWER):
    return source_bundle.read_bounded(path, limit)


def inventory(root):
    suite = json.loads(read(root / EVALS / "tasks.json"))
    tasks = {task["id"]: task for task in suite["tasks"]}
    if (suite["schema_version"] != 1 or len(suite["tasks"]) != 30
            or list(tasks) != [f"T{i:02d}" for i in range(1, 31)]):
        raise ValueError("expected the ordered 30-task v1 corpus")
    signatures = json.loads(read(root / EVALS / "signatures.json"))
    if suite["target_first_check_passed"] != 24 or suite["controlled_context_max_utf8_bytes"] != 49152:
        raise ValueError("unexpected v1 acceptance criteria")
    if not signatures.keys() <= tasks.keys():
        raise ValueError("signature for unknown task")
    paths = [root / EVALS / folder for folder in ("checks", "reference")]
    for directory in paths:
        if {path.stem for path in directory.glob("*.rs")} != signatures.keys():
            raise ValueError(f"missing or unexpected case files in {directory.name}")
    return suite, tasks, signatures


def packet(root, suite, task, signature):
    names = DOCS + AREA_DOCS.get(task["area"], ())
    pieces = [
        "# " + task["id"] + " " + task["title"],
        task["prompt"],
        "Верните один Rust-модуль с функцией указанной сигнатуры. "
        "Используйте только этот пакет документации. Ответ проверяется с "
        "синтетическими данными и предоставленными аргументами. "
        "Файлы, окружение и сеть доступны только через явно заданные в задании интерфейсы.",
        "```rust\n" + signature + "\n```",
    ]
    for name in names:
        pieces.append("## " + name + "\n\n" + read(root / name).decode("utf-8"))
    document = ("\n\n".join(pieces) + "\n").encode("utf-8")
    if len(document) > suite["controlled_context_max_utf8_bytes"]:
        raise ValueError("documentation packet exceeds context byte budget")
    return document


def locked_packages(lock):
    return {(p["name"], p["version"], p["source"], p["checksum"])
            for p in tomllib.loads(lock.decode("utf-8"))["package"] if "source" in p}


def check_paths(graph, source, crate):
    check_external.check_graph(graph, source)
    expected = {(source / relative / "Cargo.toml").resolve()
                for relative in check_external.LIBRARIES.values()}
    expected.update((source / "vendor" / name / "Cargo.toml").resolve()
                    for name in check_external.SDK)
    expected.add((crate / "Cargo.toml").resolve())
    found = {Path(p["manifest_path"]).resolve() for p in graph["packages"] if p["source"] is None}
    if found != expected:
        raise ValueError("fixture graph contains unexpected local dependencies")


def check_provenance(provenance, reference, summary, selected):
    ids = set(selected)
    if not isinstance(provenance, dict) or not isinstance(reference, dict):
        raise ValueError("expected JSON objects for provenance and reference receipt")
    attempts = provenance.get("attempts")
    if (provenance.get("schema_version") != 1 or provenance.get("model") != summary["model"]
            or provenance.get("source_commit") != summary["source_commit"]
            or provenance.get("protocol") != "fresh-context-docs-only"
            or not isinstance(attempts, dict) or set(attempts) != ids
            or any(not isinstance(attempt, dict) for attempt in attempts.values())):
        raise ValueError("first-check provenance must identify the model, source, protocol and 30 task attempts")
    sessions = [attempts[task_id].get("session", "") for task_id in selected]
    if (any(not isinstance(session, str) or not session.strip() for session in sessions)
            or len({session.strip() for session in sessions}) != len(selected)):
        raise ValueError("first attempts require distinct exposed session identifiers")
    reference_cases = reference.get("cases")
    if (reference.get("mode") != "reference" or reference.get("status") != "finished"
            or reference.get("passed") != 30 or not isinstance(reference_cases, list)
            or len(reference_cases) != 30 or any(not isinstance(case, dict) for case in reference_cases)
            or reference.get("rustc") != summary["rustc"]
            or reference.get("lock_sha256") != summary["lock_sha256"]
            or reference.get("archive_sha256") != summary["archive_sha256"]):
        raise ValueError("first-check requires a passing reference run on this snapshot and compiler")
    checked = {case["id"]: case for case in reference_cases if case.get("status") == "passed"}
    if set(checked) != ids:
        raise ValueError("reference receipt lacks a passing result for every task")
    for case in summary["cases"]:
        attempt = attempts[case["id"]]
        if attempt.get("packet_sha256") != case["packet_sha256"] or checked[case["id"]]["packet_sha256"] != case["packet_sha256"]:
            raise ValueError(f"first-check packet hash mismatch for {case['id']}")
        if case["status"] in ("missing", "invalid-answer"):
            if attempt.get("disposition") != case["status"]:
                raise ValueError(f"record the unavailable answer disposition for {case['id']}")
            continue
        if attempt.get("answer_sha256") != case.get("answer_sha256"):
            raise ValueError(f"first-check packet or answer hash mismatch for {case['id']}")
        if case.get("answer_sha256") and attempt.get("contract_review") not in ("pass", "fail"):
            raise ValueError(f"record the manual contract review for {case['id']}")


def record(path, value):
    temporary = path.with_suffix(".pending")
    temporary.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    temporary.replace(path)


def executable(log, target, name):
    artifacts = []
    for line in read(log, 16 * 1024**2).decode("utf-8").splitlines():
        event = json.loads(line)
        if (event["reason"] == "compiler-artifact" and event["target"]["kind"] == ["test"]
                and event["target"]["name"] == name and event["executable"]):
            artifacts.append(Path(event["executable"]).resolve())
    if len(artifacts) != 1 or not artifacts[0].is_relative_to(target) or not artifacts[0].is_file():
        raise ValueError("Cargo did not report the expected test executable")
    return artifacts[0]


def stage(root, crate, ids):
    (crate / "src").mkdir(parents=True)
    (crate / "src/lib.rs").write_text("", encoding="utf-8")
    (crate / "tests").mkdir()
    manifest = check_external.manifest(root, test_kit=True)
    manifest = manifest.replace('name = "telegram-external-consumer"', 'name = "telegram-eval-coding"')
    manifest += '\ntokio-util = { version = "0.7", features = ["rt"] }\n'
    manifest += 'reqwest = { version = "0.13", default-features = false, features = ["json"] }\n'
    (crate / "Cargo.toml").write_text(manifest, encoding="utf-8")
    for task_id in ids:
        case = crate / "tests" / task_id.lower()
        case.mkdir()
        (case / "main.rs").write_text(
            "#![forbid(unsafe_code)]\nmod fixture;\nmod solution;\ninclude!(\"check.rs\");\n", encoding="utf-8")
        shutil.copyfile(root / EVALS / "fixture.rs", case / "fixture.rs")
        shutil.copyfile(root / EVALS / "checks" / (task_id + ".rs"), case / "check.rs")


def oracle_name(check):
    names = re.findall(r"(?m)^#\[(?:tokio::)?test\]\s*\n(?:async )?fn ([A-Za-z0-9_]+)\(",
                       read(check).decode("utf-8"))
    if len(names) != 1:
        raise ValueError("each case needs exactly one named evaluator test")
    return names[0]


def oracle_passed(log, name):
    with log.open("rb") as stream:
        # The harness footer is last. Candidate stdout cannot suppress later tests.
        stream.seek(max(0, log.stat().st_size - 32768))
        tail = stream.read(32768)
    return (
        re.search(rb"^test " + name.encode() + rb" \.\.\. ok\r?$", tail, re.MULTILINE)
        and re.search(rb"^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured;", tail, re.MULTILINE)
    )


def run(args):
    check_compat.guard(ROOT)
    archive, sidecar = source_bundle.build(ROOT, ROOT / "target/dist")
    source_info, files = source_bundle.verify_bundle(read(archive, source_bundle.MAX_ARCHIVE),
                                                     read(sidecar, source_bundle.MAX_MANIFEST))
    target = ROOT / "target" / f"eval-coding-{args.toolchain}"
    parent = ROOT / "target/eval-coding-evidence"
    parent.mkdir(parents=True, exist_ok=True)
    evidence = Path(tempfile.mkdtemp(prefix="run-", dir=parent))
    scratch_parent = Path(tempfile.gettempdir()).resolve()
    scratch = Path(tempfile.mkdtemp(prefix="tg-coding-eval-", dir=scratch_parent)).resolve()
    print(f"evidence {evidence}", flush=True)
    summary = {
        "status": "preflight", "mode": "reference" if args.reference else args.attempt,
        "model": args.model, "toolchain": args.toolchain,
        "source_commit": source_info["commit"], "archive_sha256": source_info["archive_sha256"],
        "started_utc": datetime.now(timezone.utc).isoformat(), "cases": [],
    }
    receipt = evidence / "summary.json"
    retain = False
    cargo = ["cargo", "+" + args.toolchain]
    env = os.environ | {"RUSTUP_AUTO_INSTALL": "0", "CARGO_INCREMENTAL": "0",
                       "CARGO_BUILD_JOBS": "3", "CARGO_PROFILE_DEV_DEBUG": "line-tables-only",
                       "CARGO_PROFILE_TEST_DEBUG": "line-tables-only", "RUSTC_WRAPPER": "",
                       "RUSTC_WORKSPACE_WRAPPER": "", "CARGO_ENCODED_RUSTFLAGS": ""}
    env.pop("CARGO_BUILD_TARGET", None)

    def command(argv, name, cwd, timeout=900, runtime=False):
        # The fixture needs OS/temp discovery, but no service or Cargo credentials.
        allowed = {"PATH", "SYSTEMROOT", "WINDIR", "TMP", "TEMP", "TMPDIR", "USERPROFILE", "HOME"}
        process_env = {k: v for k, v in env.items() if k.upper() in allowed} if runtime else env
        with (evidence / (name + ".stdout")).open("xb") as stdout, (evidence / (name + ".stderr")).open("xb") as stderr:
            return subprocess.run(argv, cwd=cwd, env=process_env, stdin=subprocess.DEVNULL,
                                  stdout=stdout, stderr=stderr, timeout=timeout, check=False).returncode

    record(receipt, summary)
    try:
        source = scratch / "source"
        source_bundle.materialize(files, source)
        suite, tasks, signatures = inventory(source)
        ids = args.task or list(signatures)
        if len(set(ids)) != len(ids) or not ids or any(task_id not in signatures for task_id in ids):
            raise ValueError("select distinct implemented cases")
        if not args.reference and args.attempt != "pilot" and suite["status"] != "frozen":
            raise ValueError("scoring requires the frozen corpus and all 30 executable cases")
        if suite["status"] == "frozen" and len(signatures) != 30:
            raise ValueError("frozen corpus requires 30 cases")
        if not args.reference and args.attempt == "first" and len(ids) != 30:
            raise ValueError("a first-check run requires all 30 tasks")
        answers = source / EVALS / "reference" if args.reference else args.submissions
        summary.update(corpus_sha256=digest(read(source / EVALS / "tasks.json")),
                       suite=suite["suite"], context_max_utf8_bytes=suite["controlled_context_max_utf8_bytes"],
                       selected=ids, denominator=30, frozen=suite["status"] == "frozen")
        if command(["rustup", "which", "--toolchain", args.toolchain, "rustc"], "compiler", scratch):
            raise ValueError("installed compiler unavailable")
        env["RUSTC"] = read(evidence / "compiler.stdout").decode().strip()
        if command([env["RUSTC"], "-Vv"], "rustc", scratch):
            raise ValueError("installed compiler did not report its version")
        summary["rustc"] = read(evidence / "rustc.stdout").decode().strip()
        crate = scratch / "consumer"
        stage(source, crate, ids)
        seed = read(source / "Cargo.lock", 512 * 1024)
        (crate / "Cargo.lock").write_bytes(seed)
        # Cargo retains locked versions while adding the independent consumer.
        graph_cmd = [*cargo, "metadata", "--offline", "--format-version", "1"]
        if command(graph_cmd, "metadata", crate):
            raise ValueError("offline fixture dependencies unavailable")
        graph = json.loads(read(evidence / "metadata.stdout", 16 * 1024**2))
        check_paths(graph, source, crate)
        lock = read(crate / "Cargo.lock", 512 * 1024)
        if not locked_packages(lock) <= locked_packages(seed):
            raise ValueError("fixture resolved a dependency outside the source lock")
        (evidence / "Cargo.lock").write_bytes(lock)
        summary.update(status="running", lock_sha256=digest(lock), resolve_command=graph_cmd)
        for task_id in ids:
            context = packet(source, suite, tasks[task_id], signatures[task_id])
            (evidence / f"{task_id}.packet.md").write_bytes(context)
            item = {"id": task_id, "packet_sha256": digest(context), "packet_bytes": len(context)}
            summary["cases"].append(item)
            try:
                answer = read(answers / f"{task_id}.rs")
            except FileNotFoundError:
                item.update(status="missing")
                record(receipt, summary)
                continue
            except (ValueError, OSError) as error:
                item.update(status="invalid-answer", error_type=type(error).__name__)
                record(receipt, summary)
                continue
            item.update(status="pending", answer_sha256=digest(answer))
            (evidence / f"{task_id}.answer.rs").write_bytes(answer)
        record(receipt, summary)
        if summary["mode"] == "first":
            provenance_bytes = read(args.provenance)
            reference_bytes = read(args.reference_receipt, 256 * 1024)
            check_provenance(json.loads(provenance_bytes), json.loads(reference_bytes), summary, ids)
            attempts = json.loads(provenance_bytes)["attempts"]
            for item in summary["cases"]:
                item["contract_review"] = attempts[item["id"]].get("contract_review")
                if item["status"] == "pending" and item["contract_review"] == "fail":
                    item["status"] = "contract-failed"
            (evidence / "provenance.json").write_bytes(provenance_bytes)
            (evidence / "reference-receipt.json").write_bytes(reference_bytes)
        for item in summary["cases"]:
            if item["status"] != "pending":
                continue
            check_compat.guard(ROOT)
            task_id = item["id"]
            name = task_id.lower()
            case_dir = crate / "tests" / name
            oracle = oracle_name(case_dir / "check.rs")
            shutil.copyfile(evidence / f"{task_id}.answer.rs", case_dir / "solution.rs")
            build = [*cargo, "test", "--locked", "--offline", "--no-run",
                     "--message-format=json-render-diagnostics", "--test", name, "--target-dir", str(target)]
            item.update(status="building", build=build, build_exit=None)
            record(receipt, summary)
            print(task_id, flush=True)
            code = command(build, name + "-build", crate)
            item["build_exit"] = code
            if code < 0 or code >= 2**31:
                retain = True
                raise ValueError("compiler terminated abnormally")
            if code:
                item.update(status="build-failed")
            else:
                test = [str(executable(evidence / (name + "-build.stdout"), target, name)),
                        oracle, "--exact", "--test-threads=1"]
                item.update(status="testing", test=test, test_exit=None)
                record(receipt, summary)
                try:
                    result = command(test, name + "-test", crate, timeout=30, runtime=True)
                    passed = result == 0 and oracle_passed(evidence / (name + "-test.stdout"), oracle)
                    item.update(test_exit=result, status="passed" if passed else "test-failed")
                except subprocess.TimeoutExpired:
                    # Only reviewed answers are executed here. They may not spawn child processes.
                    item["status"] = "test-timeout"
            record(receipt, summary)
        check_compat.guard(ROOT)
        count = sum(item["status"] == "passed" for item in summary["cases"])
        summary.update(status="finished", passed=count)
        print(f"{summary['mode']}: {count}/{len(ids)} selected cases", flush=True)
        if summary["mode"] == "first":
            threshold = suite["target_first_check_passed"]
            summary.update(target_passed=threshold, meets_target=count >= threshold)
            return 0 if summary["meets_target"] else 1
        return 0 if count == len(ids) else 1
    except BaseException as error:
        retain = retain or isinstance(error, (subprocess.TimeoutExpired, KeyboardInterrupt))
        summary.update(status="failed", error_type=type(error).__name__)
        if retain:
            summary["retained_scratch"] = str(scratch)
        raise
    finally:
        summary["finished_utc"] = datetime.now(timezone.utc).isoformat()
        if not retain:
            try:
                if scratch.resolve().parent != scratch_parent or scratch.is_symlink():
                    raise OSError("temporary evaluation path changed")
                shutil.rmtree(scratch)
            except OSError as error:
                summary.update(status="failed", cleanup_error=type(error).__name__, retained_scratch=str(scratch))
                raise
            finally:
                record(receipt, summary)
        else:
            record(receipt, summary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create_packet = commands.add_parser("packet")
    create_packet.add_argument("id")
    evaluate = commands.add_parser("run")
    source = evaluate.add_mutually_exclusive_group(required=True)
    source.add_argument("--reference", action="store_true")
    source.add_argument("--submissions", type=Path)
    evaluate.add_argument("--model")
    evaluate.add_argument("--provenance", type=Path)
    evaluate.add_argument("--reference-receipt", type=Path)
    evaluate.add_argument("--attempt", choices=("first", "repair", "pilot"), default="first")
    evaluate.add_argument("--toolchain", required=True)
    evaluate.add_argument("--task", action="append")
    args = parser.parse_args()
    try:
        if args.command == "packet":
            suite, tasks, signatures = inventory(ROOT)
            sys.stdout.buffer.write(packet(ROOT, suite, tasks[args.id], signatures[args.id]))
            return 0
        if not re.fullmatch(r"stable|[0-9]+\.[0-9]+\.[0-9]+", args.toolchain):
            raise ValueError("toolchain must be stable or a release version")
        if args.submissions is not None and not (args.model and args.model.strip()):
            raise ValueError("provide the exposed model identity for submissions")
        if args.submissions is not None and args.attempt == "first" and not (args.provenance and args.reference_receipt):
            raise ValueError("provide --provenance and --reference-receipt for a first-check run")
        return run(args)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(f"coding evaluation failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
