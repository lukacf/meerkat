#!/usr/bin/env python3
import json
import sys
import time
import urllib.request


def call(base_url: str, method: str, params: dict) -> dict:
    payload = json.dumps({
        "jsonrpc": "2.0",
        "id": f"python-smoke:{method}",
        "method": method,
        "params": params,
    }).encode("utf-8")
    request = urllib.request.Request(
        base_url + "/console/rpc",
        data=payload,
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.loads(response.read().decode("utf-8"))


def rpc(base_url: str, method: str, params: dict):
    body = call(base_url, method, params)
    if "error" in body:
        raise RuntimeError(body["error"])
    return body["result"]


def main() -> int:
    base_url = sys.argv[1]
    status = rpc(base_url, "meta/status_identity", {"identity": "incident-commander"})
    assert status["identity"] == "incident-commander"

    console_identities = rpc(base_url, "meta/console/list_identities", {})["identities"]
    assert any(entry["identity"] == "incident-commander" for entry in console_identities)

    console_inspect = rpc(base_url, "meta/console/inspect_identity", {"identity": "incident-commander"})
    assert console_inspect["identity"]["identity"] == "incident-commander"
    assert console_inspect["identity"]["visibility"] == "addressable"

    # Boot-time kickoff turns produce the first canonical frames; on a fresh
    # server they land a few seconds after /console/experience goes 200, so
    # poll instead of asserting the very first read.
    deadline = time.time() + 90
    while True:
        timeline = rpc(base_url, "meta/console/query_timeline", {
            "identity": "incident-commander",
            "limit": 20,
        })
        if timeline["frames"] or time.time() >= deadline:
            break
        time.sleep(1)
    assert timeline["frames"], "expected canonical console timeline frames"
    assert all(frame["identity"] == "incident-commander" for frame in timeline["frames"])

    inspect = rpc(base_url, "meta/inspect_identity", {"identity": "payments-sre"})
    assert inspect["identity"] == "payments-sre"

    rpc(base_url, "meta/reset", {"identity": "merchant-success"})
    inspect_after_reset = rpc(base_url, "meta/inspect_identity", {"identity": "merchant-success"})
    assert inspect_after_reset["identity"] == "merchant-success"

    pending = rpc(base_url, "meta/gating/pending", {})["pending"]
    assert pending, "expected seeded pending gating entry"
    first_pending = pending[0]

    # The console decides an approval only as the signed-in person on the
    # connection. This pack's console has no sign-in, so a decision is
    # refused and the request stays pending.
    refused = call(base_url, "meta/gating/decide", {
        "pending_id": first_pending["pending_id"],
        "decision": "escalate",
        "reason": "python_smoke_escalate",
    })
    assert refused.get("error", {}).get("code") == -32057, refused
    assert refused["error"].get("data", {}).get("kind") == "approver_unauthenticated", refused
    pending_after = rpc(base_url, "meta/gating/pending", {})["pending"]
    assert any(item["pending_id"] == first_pending["pending_id"] for item in pending_after)

    routes = rpc(base_url, "meta/routing/routes/list", {})["routes"]
    assert any(route["recipient"] == "statuspage@example.test" for route in routes)

    history = rpc(base_url, "meta/delivery/history", {"limit": 20})["deliveries"]
    assert history, "expected seeded delivery history"

    print("incident python smoke passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
