"""Bounded, provider-free MCP subprocess checks against the actual executable."""

import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading


def start(binary, root, stderr, capture=True):
    process = subprocess.Popen(
        [binary],
        cwd=root,
        env={"PATH": os.environ.get("PATH", ""), "HOME": str(root), "RUST_LOG": "warn"},
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=stderr,
        text=True,
    )
    responses = queue.Queue()
    if capture:
        def read():
            for line in process.stdout:
                responses.put(json.loads(line))
        threading.Thread(target=read, daemon=True).start()
    return process, responses


def send(process, value):
    process.stdin.write(json.dumps(value) + "\n")
    process.stdin.flush()


def check(binary, root):
    with (root / "stderr.log").open("w+") as stderr:
        process, responses = start(binary, root, stderr)
        sequence = 0

        def request(method, params=None):
            nonlocal sequence
            sequence += 1
            send(process, {"jsonrpc": "2.0", "id": sequence, "method": method, "params": params or {}})
            reply = responses.get(timeout=20)
            assert reply["id"] == sequence, reply
            return reply

        def tool(name, arguments=None):
            return request("tools/call", {"name": name, "arguments": arguments or {}})

        def packs():
            return {pack["name"]: pack for pack in json.loads(tool("list_packs")["result"]["content"][0]["text"])}

        try:
            assert request("initialize")["result"]["serverInfo"]["name"] == "codemob-mcp"
            send(process, {"jsonrpc": "2.0", "method": "notifications/initialized"})
            assert request("ping")["result"] == {}
            assert {tool["name"] for tool in request("tools/list")["result"]["tools"]} == {
                "list_packs", "consult", "deliberate", "list_sessions", "destroy_session",
                "create_mob", "get_mob", "update_mob", "delete_mob",
            }
            assert request("missing/method")["error"]["code"] == -32601
            process.stdin.write("{\n")
            process.stdin.flush()
            assert responses.get(timeout=10)["error"]["code"] == -32700
            baseline = packs()["review"]
            assert len(baseline["roles"]) == baseline["agents"]
            for name in ("review", "synthetic-custom"):
                definition = {
                    "name": name, "description": "synthetic override",
                    "agents": {"only": {"model": "gemini-3.5-flash", "skill": "Return a synthetic answer."}},
                }
                assert "result" in tool("create_mob", {"definition": definition})
                assert json.loads(tool("get_mob", {"name": name})["result"]["content"][0]["text"])["name"] == name
                assert packs()[name]["roles"] == {"only": "gemini-3.5-flash"}
                definition["description"] = "updated synthetic"
                assert "result" in tool("update_mob", {"definition": definition})
                assert packs()[name]["description"] == "updated synthetic"
                assert "result" in tool("delete_mob", {"name": name})
                assert tool("get_mob", {"name": name})["error"]["code"] == -32602
                current = packs()
                if name == "review":
                    assert current[name] == baseline
                else:
                    assert name not in current
            assert tool("get_mob", {"name": "../escape"})["error"]["code"] == -32602
            for params in ({"temperature": 0.2}, {"temperature": "invalid"}):
                reply = tool("consult", {"question": "synthetic", "session_id": "invalid", "provider_params": params})
                assert reply["error"]["code"] == -32602
                assert "provider_params" in reply["error"]["message"]
            assert tool("consult", {"question": "synthetic", "model": "not-a-catalog-model"})["error"]["code"] == -32603
            assert json.loads(tool("list_sessions")["result"]["content"][0]["text"]) == []
            # Cancellation is a race over a real transport. Either terminal is
            # legal; deterministic admission interleavings are covered in Rust.
            sequence += 1
            send(process, {"jsonrpc": "2.0", "id": sequence, "method": "tools/call",
                           "params": {"name": "consult", "arguments": {"question": "synthetic", "model": "not-a-catalog-model"}}})
            send(process, {"jsonrpc": "2.0", "method": "notifications/cancelled",
                           "params": {"requestId": sequence}})
            reply = responses.get(timeout=20)
            assert reply["id"] == sequence and reply["error"]["code"] in (-32005, -32603), reply
            assert request("ping")["result"] == {}
            assert json.loads(tool("list_sessions")["result"]["content"][0]["text"]) == []
            process.stdin.close()
            assert process.wait(timeout=15) == 0
            stderr.seek(0)
            assert "panicked" not in stderr.read()
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=10)

    # Fail stdout while stdin remains open, forcing the writer-completion lane.
    with (root / "broken-stderr.log").open("w+") as stderr:
        process, _ = start(binary, root, stderr, capture=False)
        try:
            process.stdout.close()
            send(process, {"jsonrpc": "2.0", "id": 1, "method": "ping"})
            assert process.wait(timeout=15) == 0
            stderr.seek(0)
            assert "panicked" not in stderr.read()
        finally:
            process.stdin.close()
            if process.poll() is None:
                process.kill()
                process.wait(timeout=10)


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix=".mcp-test-", dir=Path(__file__).resolve().parent) as directory:
        check(str(Path(sys.argv[1]).resolve()), Path(directory))
    print("MCP handshake, failure, cancellation race, CRUD, EOF, and broken stdout passed")
