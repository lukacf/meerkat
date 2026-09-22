#!/usr/bin/env python3
"""Local JSONL peer for executing SDK examples without providers."""

import json
import os
from pathlib import Path
import signal
import sys

log = Path(os.environ["EXAMPLE_TEST_LOG"])
mode = os.environ["EXAMPLE_TEST_MODE"]
session_id = "019d1234-0000-7000-8000-000000000001"
summary = {
    "session_id": session_id,
    "created_at": 0,
    "updated_at": 0,
    "message_count": 8,
    "is_active": True,
    "total_tokens": 12,
    "model": "claude-sonnet-4-6",
    "provider": "anthropic",
}


def record(value):
    with log.open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(value) + "\n")


def send(value):
    print(json.dumps({"jsonrpc": "2.0", **value}), flush=True)


def terminate(signum, _frame):
    record({"exit_signal": signum})
    sys.exit(0)


signal.signal(signal.SIGTERM, terminate)
record({"pid": os.getpid(), "args": sys.argv[1:]})
for line in sys.stdin:
    request = json.loads(line)
    record(request)
    method = request["method"]
    params = request.get("params", {})
    if method == "initialize":
        if mode == "exit-before-reply":
            sys.exit(13)
        result = {
            "contract_version": (
                "99.0.0" if mode == "mismatch" else os.environ["EXAMPLE_TEST_VERSION"]
            ),
            "methods": [],
        }
    elif method == "capabilities/get":
        result = {"capabilities": [
            {"id": "skills", "description": "Synthetic skill", "status": "Available"}
        ]}
    elif method == "config/get":
        result = {
            "config": {}, "generation": 0,
            "realm_id": "example-test", "instance_id": "example-test",
        }
    elif method in {"session/create", "turn/start"}:
        refuse = mode == "create-error" or (
            mode == "comparison-error" and "provider_params" not in params
        )
        if method == "session/create" and refuse:
            send({"id": request["id"], "error": {
                "code": -32000, "message": "Synthetic session refusal",
            }})
            continue
        result = {
            "session_id": session_id, "text": "Synthetic answer",
            "turns": 1, "tool_calls": 0,
            "usage": {"input_tokens": 1, "output_tokens": 2, "total_tokens": 3},
        }
        if "output_schema" in params:
            result["structured_output"] = {
                "sentiment": "positive", "confidence": 0.5,
                "key_phrases": ["synthetic"], "summary": "Synthetic sentiment",
            }
        if method == "turn/start":
            send({"method": "session/event", "params": {
                "session_id": session_id,
                "event": {
                    "event_id": "019d1234-0000-7000-8000-000000000002",
                    "source": {"type": "session", "session_id": session_id},
                    "seq": 1, "timestamp_ms": 0,
                    "payload": {"type": "text_delta", "delta": "Synthetic stream"},
                },
            }})
    elif method == "session/read":
        result = summary
    elif method == "session/list":
        result = {"sessions": [summary]}
    elif method == "session/archive":
        result = {"archived": True}
    else:
        raise RuntimeError(f"Unexpected fixture method: {method}")
    send({"id": request["id"], "result": result})
record({"eof": True})
