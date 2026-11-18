"""Record and score fresh-agent answers obtained through the offline MCP archive."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import queue
import subprocess
import tempfile
import xml.etree.ElementTree as ET

from mcp_fixture_check import READ_TOOLS, ROOT, archive, check

CORPUS = ROOT / "tg-agent/evals/v1"
CASES = tuple(f"Q{number:02}" for number in range(1, 11))


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write(path, value):
    with path.open("x", encoding="utf-8") as stream:
        stream.write(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def read(path):
    return json.loads(path.read_text(encoding="utf-8"))


def questions():
    root = ET.parse(CORPUS / "questions.xml").getroot()
    items = [(item.findtext("question"), item.findtext("answer")) for item in root]
    if root.tag != "evaluation" or len(items) != len(CASES) or not all(q and a for q, a in items):
        raise ValueError("expected ten archive questions with answer keys")
    return dict(zip(CASES, items))


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def prepare(args):
    if git("status", "--porcelain"):
        raise ValueError("prepare evaluation from a clean source commit")
    executable = args.executable.resolve(strict=True)
    check(executable)
    parent = ROOT / "target/mcp-eval-evidence"
    parent.mkdir(parents=True, exist_ok=True)
    run = Path(tempfile.mkdtemp(prefix="run-", dir=parent))
    packets = {}
    for case, (question, _) in questions().items():
        packet = run / f"{case}.packet.txt"
        packet.write_text(question + "\n", encoding="utf-8")
        packets[case] = digest(packet)
    write(run / "run.json", {
        "source_commit": git("rev-parse", "HEAD"), "model": args.model,
        "executable": str(executable), "executable_sha256": digest(executable),
        "questions_sha256": digest(CORPUS / "questions.xml"),
        "archive_sha256": digest(CORPUS / "archive.json"), "packet_sha256": packets,
        "started_utc": datetime.now(timezone.utc).isoformat(),
    })
    print(run)


def calls(run, case):
    trace = run / f"{case}.calls.jsonl"
    return [json.loads(line) for line in trace.read_text(encoding="utf-8").splitlines()] if trace.exists() else []


def invoke(args):
    run_path = args.run
    if (run_path / f"{args.case}.answer.json").exists() or (run_path / "summary.json").exists():
        raise ValueError("question already answered or scored")
    run = read(run_path / "run.json")
    executable = Path(run["executable"])
    if digest(executable) != run["executable_sha256"]:
        raise ValueError("archive executable changed during evaluation")
    arguments = {}
    if args.tool == "read_updates":
        arguments["limit"] = args.limit
        if args.cursor is not None:
            issued = {record["result"]["updates"]["next_cursor"]
                      for record in calls(run_path, args.case)
                      if record["tool"] == "read_updates" and record["status"] == "passed"}
            if args.cursor not in issued:
                raise ValueError("cursor was not issued to this question")
            arguments["cursor"] = args.cursor
    elif args.tool == "get_chat":
        arguments["chat_id"] = str(args.chat_id)
    record = {"tool": args.tool, "arguments": arguments, "status": "failed"}
    try:
        with archive(executable) as call:
            result = call(args.tool, arguments)
            record["result"] = result
            if args.tool == "read_updates" and len(result["updates"]["events"]) > args.limit:
                raise ValueError("archive returned an oversized page")
        record["status"] = "passed"
    except (OSError, ValueError, KeyError, queue.Empty, subprocess.SubprocessError) as error:
        record["error"] = type(error).__name__
        raise
    finally:
        with (run_path / f"{args.case}.calls.jsonl").open("a", encoding="utf-8") as trace:
            trace.write(json.dumps(record, ensure_ascii=False) + "\n")
    print(json.dumps(result, ensure_ascii=True))


def submit(args):
    run = read(args.run / "run.json")
    if digest(args.run / f"{args.case}.packet.txt") != run["packet_sha256"][args.case]:
        raise ValueError("question packet changed before answer")
    if not args.session.strip():
        raise ValueError("session identifier is required")
    write(args.run / f"{args.case}.answer.json", {
        "session": args.session, "answer": None if args.missing else args.answer,
        "submitted_utc": datetime.now(timezone.utc).isoformat(),
    })


def score(args):
    run = read(args.run / "run.json")
    if (run["questions_sha256"] != digest(CORPUS / "questions.xml")
            or run["archive_sha256"] != digest(CORPUS / "archive.json")):
        raise ValueError("corpus changed during evaluation")
    attempts = {case: read(args.run / f"{case}.answer.json") for case in CASES}
    sessions = [attempts[case]["session"] for case in CASES]
    if not all(isinstance(session, str) and session.strip() for session in sessions) or len(set(sessions)) != len(CASES):
        raise ValueError("expected ten distinct fresh sessions")
    results = []
    for case, (_, key) in questions().items():
        if digest(args.run / f"{case}.packet.txt") != run["packet_sha256"][case]:
            raise ValueError(f"question packet changed for {case}")
        answer = attempts[case]["answer"]
        trace = calls(args.run, case)
        if any(call["tool"] not in READ_TOOLS for call in trace):
            raise ValueError(f"unexpected tool in trace for {case}")
        successful = [call for call in trace if call["status"] == "passed"]
        supported = any(call["tool"] == "read_updates" for call in successful)
        if case == "Q09":
            supported &= any(call["tool"] == "get_chat" and call["result"]["chat"]["title"] == key
                             for call in successful)
        results.append({"id": case, "session": attempts[case]["session"], "answer": answer,
                        "calls": len(trace), "correct": supported and answer is not None and answer.strip() == key})
    passed = sum(item["correct"] for item in results)
    write(args.run / "summary.json", run | {"passed": passed, "total": len(CASES), "cases": results})
    print(f"{passed}/{len(CASES)} archive answers correct")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    setup = commands.add_parser("prepare")
    setup.add_argument("executable", type=Path)
    setup.add_argument("--model", required=True)
    call = commands.add_parser("call")
    call.add_argument("run", type=Path)
    call.add_argument("case", choices=CASES)
    call.add_argument("tool", choices=sorted(READ_TOOLS))
    call.add_argument("--limit", type=int, choices=range(1, 4), default=3)
    call.add_argument("--cursor")
    call.add_argument("--chat-id", type=int)
    submission = commands.add_parser("submit")
    submission.add_argument("run", type=Path)
    submission.add_argument("case", choices=CASES)
    submission.add_argument("--session", required=True)
    response = submission.add_mutually_exclusive_group(required=True)
    response.add_argument("--answer")
    response.add_argument("--missing", action="store_true")
    evaluate = commands.add_parser("score")
    evaluate.add_argument("run", type=Path)
    args = parser.parse_args()
    if args.command == "call" and ((args.tool == "get_chat") != (args.chat_id is not None)
                                   or (args.cursor is not None and args.tool != "read_updates")):
        parser.error("get_chat requires --chat-id; --cursor belongs to read_updates")
    try:
        {"prepare": prepare, "call": invoke, "submit": submit, "score": score}[args.command](args)
    except (AssertionError, OSError, KeyError, ValueError, queue.Empty, subprocess.SubprocessError) as error:
        parser.exit(1, f"{type(error).__name__}: {error}\n")


if __name__ == "__main__":
    main()
