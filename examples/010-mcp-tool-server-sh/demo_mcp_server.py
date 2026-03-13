#!/usr/bin/env python3
"""Tiny stdio MCP server used by example 010.

This intentionally uses line-delimited JSON-RPC, matching the lightweight test
server shape used in this repository, so the example stays self-contained and
easy to read.
"""

from __future__ import annotations

import json
import sys
from typing import Any


SERVICE_DATA: dict[str, dict[str, Any]] = {
    "checkout-api": {
        "severity": "sev-1",
        "owner": "payments-platform",
        "recent_deploy": "2026-03-13 08:42 UTC - rollout build 2026.03.13-rc2",
        "customer_impact": "card authorizations intermittently fail in EU checkout",
        "rollback_command": "deployctl rollback checkout-api --to 2026.03.12.4",
        "playbook": "PB-17 Checkout rollback + payment gateway failover",
        "status": "degraded",
        "release_readiness": "hold",
        "recommended_next_action": "rollback the checkout-api canary and pin gateway traffic to stable",
    },
    "search-api": {
        "severity": "sev-3",
        "owner": "discovery-platform",
        "recent_deploy": "2026-03-13 07:15 UTC - synonym expansion refresh",
        "customer_impact": "search latency elevated but requests succeed",
        "rollback_command": "deployctl rollback search-api --to 2026.03.13.1",
        "playbook": "PB-08 Search latency regression",
        "status": "degraded",
        "release_readiness": "proceed-with-monitoring",
        "recommended_next_action": "disable synonym refresh and watch p95 for 10 minutes",
    },
}


TOOLS = [
    {
        "name": "incident_digest",
        "description": (
            "Summarize the current incident state for a service, including "
            "severity, owner, recent deploy, rollback command, and customer impact."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "service": {
                    "type": "string",
                    "description": "Service name, for example 'checkout-api'.",
                }
            },
            "required": ["service"],
            "additionalProperties": False,
        },
    },
    {
        "name": "release_readiness",
        "description": (
            "Decide whether a release should continue, hold, or rollback based "
            "on the current service status and incident posture."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "service": {
                    "type": "string",
                    "description": "Service name, for example 'checkout-api'.",
                }
            },
            "required": ["service"],
            "additionalProperties": False,
        },
    },
]


def rpc_result(request_id: Any, result: dict[str, Any]) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": request_id, "result": result}


def rpc_error(request_id: Any, code: int, message: str) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}}


def text_result(text: str, *, is_error: bool = False) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "content": [{"type": "text", "text": text}],
    }
    if is_error:
        payload["isError"] = True
    return payload


def lookup_service(arguments: dict[str, Any]) -> tuple[str, dict[str, Any]] | None:
    service = str(arguments.get("service", "")).strip()
    data = SERVICE_DATA.get(service)
    if not data:
        return None
    return service, data


def handle_request(request: dict[str, Any]) -> dict[str, Any] | None:
    request_id = request.get("id")
    method = request.get("method")

    if request_id is None:
        return None

    if method == "initialize":
        return rpc_result(
            request_id,
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "incident-kit", "version": "1.0.0"},
            },
        )

    if method == "ping":
        return rpc_result(request_id, {})

    if method == "tools/list":
        return rpc_result(request_id, {"tools": TOOLS})

    if method == "tools/call":
        params = request.get("params", {})
        name = params.get("name")
        arguments = params.get("arguments", {})
        resolved = lookup_service(arguments)
        if not resolved:
            return rpc_result(
                request_id,
                text_result(
                    "Unknown service. Try one of: checkout-api, search-api.",
                    is_error=True,
                ),
            )

        service, data = resolved

        if name == "incident_digest":
            text = (
                f"service: {service}\n"
                f"severity: {data['severity']}\n"
                f"owner: {data['owner']}\n"
                f"status: {data['status']}\n"
                f"recent_deploy: {data['recent_deploy']}\n"
                f"customer_impact: {data['customer_impact']}\n"
                f"rollback_command: {data['rollback_command']}\n"
                f"playbook: {data['playbook']}\n"
                f"recommended_next_action: {data['recommended_next_action']}"
            )
            return rpc_result(request_id, text_result(text))

        if name == "release_readiness":
            text = (
                f"service: {service}\n"
                f"release_readiness: {data['release_readiness']}\n"
                f"reason: current service status is {data['status']} with {data['severity']} customer impact\n"
                f"next_action: {data['recommended_next_action']}"
            )
            return rpc_result(request_id, text_result(text))

        return rpc_error(request_id, -32601, f"Unknown tool: {name}")

    return rpc_error(request_id, -32601, f"Unknown method: {method}")


def main() -> int:
    for raw_line in sys.stdin:
        line = raw_line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError as exc:
            print(json.dumps(rpc_error(None, -32700, f"Parse error: {exc}")), flush=True)
            continue

        response = handle_request(request)
        if response is not None:
            print(json.dumps(response), flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
