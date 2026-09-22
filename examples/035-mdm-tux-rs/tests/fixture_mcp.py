"""Synthetic local MCP server: no credentials, devices, or network access."""
import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {
            "protocolVersion": request["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "mdm-fixture", "version": "1"},
        }
    elif method == "tools/list":
        result = {"tools": [{
            "name": "synthetic_probe",
            "description": "Read a synthetic constant.",
            "inputSchema": {"type": "object", "properties": {}},
        }]}
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": "synthetic fixture"}]}
    elif method == "ping":
        result = {}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"],
                          "error": {"code": -32601, "message": method}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
