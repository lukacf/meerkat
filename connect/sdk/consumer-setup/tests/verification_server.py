"""Inert wire fixture for exercising the real SDK stdio boundary."""
import json
import os
from pathlib import Path
import sys
import time

mode, log_path = sys.argv[1:]


def record(value):
    with open(log_path, "a") as stream:
        stream.write(json.dumps(value) + "\n")


def send(identity, result):
    result.setdefault("resultType", "complete")
    if "tools" in result or "supportedVersions" in result:
        result.update(cacheScope="private", ttlMs=0)
    print(json.dumps({"jsonrpc": "2.0", "id": identity, "result": result}), flush=True)


record({"pid": os.getpid(), "cwd": os.getcwd(), "marker": os.environ.get("MARKER")})
list_count = 0
for line in sys.stdin:
    request = json.loads(line)
    record(request)
    method, identity = request.get("method"), request.get("id")
    if identity is None:
        continue
    if method == "server/discover":
        send(identity, {"supportedVersions": ["2026-07-28"], "capabilities": {"tools": {}}})
    elif method == "initialize":
        send(identity, {"protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
                        "serverInfo": {"name": "inert", "version": "1"}})
    elif method == "tools/list":
        list_count += 1
        schema = {"type": "object", "properties": {"n": {"type": "integer", "minimum": 1}},
                  "required": ["n"], "additionalProperties": False}
        if mode == "changed" or (mode == "later_changed" and list_count > 1):
            schema["properties"]["n"]["minimum"] = True
        tool = {"name": "increment", "inputSchema": schema,
                "outputSchema": {"type": "object", "properties": {"value": {"type": "integer"}},
                                 "required": ["value"], "additionalProperties": False}}
        tools = [] if mode == "missing" else [tool]
        result = {"tools": tools}
        if mode == "extra":
            tools.append({"name": "unselected", "inputSchema": {"type": "object"}})
        if mode in ("pages", "duplicate", "cycle"):
            cursor = request.get("params", {}).get("cursor")
            if cursor and mode != "duplicate":
                result["tools"] = [{"name": "page2", "inputSchema": {"type": "object"}}]
            if not cursor or mode == "cycle":
                result["nextCursor"] = "next"
        if mode == "output_absent":
            del tool["outputSchema"]
        if mode == "output_null":
            tool["outputSchema"] = None
        send(identity, result)
    elif method == "tools/call":
        if mode == "timeout":
            time.sleep(30)
        if mode == "input":
            send(identity, {"resultType": "input_required", "requestState": "PRIVATE_AUTH_URL"})
        elif mode == "task":
            send(identity, {"resultType": "task", "taskId": "PRIVATE_TASK"})
        elif mode == "error":
            send(identity, {"content": [{"type": "text", "text": "fixture error"}], "isError": True})
        elif mode == "overflow":
            send(identity, {"content": [{"type": "text", "text": "x" * 20000}],
                            "structuredContent": {"value": 2}})
        else:
            value = request["params"]["arguments"]["n"] + 1
            send(identity, {"content": [{"type": "text", "text": str(value)}],
                            "structuredContent": {"value": value}})
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": identity,
                          "error": {"code": -32601, "message": "unknown"}}), flush=True)
record({"eof": True})
if mode == "shutdown_stall":
    time.sleep(30)
