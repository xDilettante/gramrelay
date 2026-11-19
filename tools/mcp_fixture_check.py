"""Exercise the synthetic archive over real MCP stdio; this is not an LLM score."""
import argparse
from contextlib import contextmanager
import json
import os
import queue
import subprocess
import threading
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
READ_TOOLS = {"get_me", "get_chat", "read_updates"}


@contextmanager
def archive(executable):
    allowed = {"PATH", "SYSTEMROOT", "WINDIR", "TMP", "TEMP", "TMPDIR", "USERPROFILE", "HOME"}
    env = {key: value for key, value in os.environ.items() if key.upper() in allowed}
    process = subprocess.Popen([str(executable)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, env=env)
    messages = queue.Queue(maxsize=4)

    def read():
        while True:
            line = process.stdout.readline(2 * 1024 * 1024)
            messages.put(line)
            if not line:
                return

    threading.Thread(target=read, daemon=True).start()
    serial = 0

    def exchange(method, params):
        nonlocal serial
        serial += 1
        frame = {"jsonrpc": "2.0", "id": serial, "method": method, "params": params}
        process.stdin.write(json.dumps(frame).encode() + b"\n")
        process.stdin.flush()
        line = messages.get(timeout=10)
        if not line.endswith(b"\n"):
            raise ValueError("incomplete MCP response frame")
        reply = json.loads(line)
        if reply.get("jsonrpc") != "2.0" or reply.get("id") != serial or "error" in reply:
            raise ValueError("invalid or rejected MCP exchange")
        return reply["result"]

    def call(name, arguments):
        request = {"schema_version": 1, "profile": "archive", "call": {"tool": name, "arguments": arguments}}
        result = exchange("tools/call", {"name": name, "arguments": request})
        content = result["structuredContent"]
        if content != json.loads(result["content"][0]["text"]) or not content.get("ok"):
            raise ValueError("invalid or rejected MCP tool call")
        return content["result"]

    try:
        version = "2025-11-25"
        initialized = exchange("initialize", {
            "protocolVersion": version, "capabilities": {},
            "clientInfo": {"name": "fixture-check", "version": "1"},
        })
        if initialized["protocolVersion"] != version:
            raise ValueError("unexpected MCP archive protocol")
        process.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        process.stdin.flush()
        tools = {tool["name"] for tool in exchange("tools/list", {})["tools"]}
        if len(tools) != 8 or not READ_TOOLS <= tools:
            raise ValueError("unexpected MCP archive catalog")
        yield call
        process.stdin.close()
        if messages.get(timeout=10) or process.wait(timeout=10) != 0 or process.stderr.read(1):
            raise ValueError("MCP archive did not shut down cleanly")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=10)
        process.stdin.close()
        process.stdout.close()
        process.stderr.close()


def check(executable):
    fixture = json.loads((ROOT / "tg-agent/evals/v1/archive.json").read_text(encoding="utf-8"))
    questions = ET.parse(ROOT / "tg-agent/evals/v1/questions.xml").getroot()
    assert questions.tag == "evaluation" and len(questions) == 10
    assert all(item.findtext("question") and item.findtext("answer") for item in questions)
    with archive(executable) as call:
        assert call("get_me", {})["bot"]["username"] == "offline_archive_bot"
        for chat in fixture["chats"]:
            assert call("get_chat", {"chat_id": chat["id"]})["chat"]["title"] == chat["title"]
        events, cursor = [], None
        for _ in fixture["events"]:
            arguments = {"limit": 3}
            if cursor is not None:
                arguments["cursor"] = cursor
            page = call("read_updates", arguments)["updates"]
            assert len(page["events"]) <= 3
            for event in page["events"]:
                assert event["untrusted"] is True
                events.append(json.loads(event["telegram_json"])["message"])
            cursor = page["next_cursor"]
            if not page["has_more"]:
                break
        else:
            raise ValueError("MCP archive pagination did not terminate")
        assert len(events) == len(fixture["events"])
        for actual, expected in zip(events, fixture["events"]):
            assert str(actual["chat"]["id"]) == expected["chat"]
            assert actual["from"]["first_name"] == expected["author"]
            assert actual["text"] == expected["text"]
            assert actual.get("document") == expected.get("document")
    print(f"MCP archive fixture: {len(events)} events, 3 chats, 10 evaluation questions; no LLM score measured.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path, help="Built mcp_fixture example executable")
    check(parser.parse_args().executable.resolve())
