"""Exercise the synthetic archive over real MCP stdio; this is not an LLM score."""
import argparse
import json
import queue
import subprocess
import threading
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def check(executable):
    fixture = json.loads((ROOT / "tg-agent/evals/v1/archive.json").read_text(encoding="utf-8"))
    questions = ET.parse(ROOT / "tg-agent/evals/v1/questions.xml").getroot()
    assert questions.tag == "evaluation" and len(questions) == 10
    assert all(item.findtext("question") and item.findtext("answer") for item in questions)
    process = subprocess.Popen([str(executable)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
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
        reply = json.loads(messages.get(timeout=10))
        assert reply["id"] == serial and "error" not in reply, reply
        return reply["result"]

    def call(name, arguments):
        request = {"schema_version": 1, "profile": "archive", "call": {"tool": name, "arguments": arguments}}
        result = exchange("tools/call", {"name": name, "arguments": request})
        content = result["structuredContent"]
        assert content == json.loads(result["content"][0]["text"])
        assert content["ok"], content
        return content["result"]

    try:
        exchange("initialize", {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "fixture-check", "version": "1"}})
        process.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        process.stdin.flush()
        assert len(exchange("tools/list", {})["tools"]) == 8
        assert call("get_me", {})["bot"]["username"] == "offline_archive_bot"
        for chat in fixture["chats"]:
            assert call("get_chat", {"chat_id": chat["id"]})["chat"]["title"] == chat["title"]
        events, cursor = [], None
        while True:
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
        assert len(events) == len(fixture["events"])
        for actual, expected in zip(events, fixture["events"]):
            assert str(actual["chat"]["id"]) == expected["chat"]
            assert actual["text"] == expected["text"]
            if "document" in expected:
                assert actual["document"]["file_name"] == expected["document"]["file_name"]
        process.stdin.close()
        assert not messages.get(timeout=10), "unexpected frame after all requests completed"
        assert process.wait(timeout=10) == 0
        assert process.stderr.read() == b""
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=10)
        process.stdout.close()
        process.stderr.close()
    print(f"MCP archive fixture: {len(events)} events, 3 chats, 10 evaluation questions; no LLM score measured.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path, help="Built mcp_fixture example executable")
    check(parser.parse_args().executable.resolve())
