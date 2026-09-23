"""Opt-in, bounded Gemini smoke test; sends only fixed synthetic prompts."""

import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading


def check(binary, root):
    key = os.environ.get("GEMINI_API_KEY")
    if not key:
        raise SystemExit("GEMINI_API_KEY is required (no other provider is contacted)")
    with (root / "stderr.log").open("w") as stderr:
        process = subprocess.Popen(
            [binary], cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=stderr, text=True,
            env={"PATH": os.environ.get("PATH", ""), "HOME": str(root),
                 "GEMINI_API_KEY": key, "RUST_LOG": "warn"},
        )
        replies = queue.Queue()

        def read():
            for line in process.stdout:
                replies.put(json.loads(line))
        threading.Thread(target=read, daemon=True).start()
        sequence = 0

        def request(method, params):
            nonlocal sequence
            sequence += 1
            process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": sequence, "method": method, "params": params}) + "\n")
            process.stdin.flush()
            try:
                reply = replies.get(timeout=45)
            except queue.Empty:
                process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": sequence}}) + "\n")
                process.stdin.flush()
                raise RuntimeError("Synthetic consult exceeded 45s; cancellation sent") from None
            assert reply["id"] == sequence
            return reply

        def tool(name, arguments):
            return request("tools/call", {"name": name, "arguments": arguments})

        try:
            request("initialize", {})
            first = tool("consult", {
                "question": "Reply exactly SYNTHETIC_OK.", "model": "gemini-3.5-flash",
                "system_prompt": "Only return the requested literal. Never use tools.",
                "shell": False,
            })
            if "error" in first:
                raise RuntimeError(json.dumps(first["error"]).replace(key, "[REDACTED]")[:500])
            assert "SYNTHETIC_OK" in first["result"]["content"][0]["text"]
            sid = first["result"]["content"][1]["text"].split("session_id: ")[1]
            print("PASS: synthetic Gemini new-session consult", flush=True)
            second = tool("consult", {"question": "Reply exactly SYNTHETIC_AGAIN.", "session_id": sid})
            cleanup = tool("destroy_session", {"session_id": sid})
            assert "result" in cleanup
            if "error" in second:
                raise RuntimeError(json.dumps(second["error"]).replace(key, "[REDACTED]")[:1000])
            assert "SYNTHETIC_AGAIN" in second["result"]["content"][0]["text"]
            print("PASS: synthetic Gemini continuation and session cleanup", flush=True)
        finally:
            process.stdin.close()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix=".live-test-", dir=Path(__file__).resolve().parent) as directory:
        check(str(Path(sys.argv[1]).resolve()), Path(directory))
