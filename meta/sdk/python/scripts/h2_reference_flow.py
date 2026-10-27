#!/usr/bin/env python3

from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Callable

import httpx

EXPECTED_CAPABILITY_METHODS = [
    "meta/status",
    "meta/capabilities",
    "meta/reconcile",
    "meta/spawn_member",
    "meta/routing/resolve",
    "meta/routing/routes/list",
    "meta/routing/routes/add",
    "meta/routing/routes/delete",
    "meta/delivery/send",
    "meta/delivery/history",
    "meta/events/subscribe",
    "meta/memory/stores",
    "meta/memory/index",
    "meta/memory/query",
    "meta/session_store/bigquery",
    "meta/gating/evaluate",
    "meta/gating/pending",
    "meta/gating/decide",
    "meta/gating/audit",
]
EXPECTED_CONTRACT_VERSION = "0.5.0"


def _bind_listener() -> socket.socket:
    """Bind and listen on a free loopback port, handed to the app as an open fd.

    Picking a free port, closing it, and letting the app bind the number later
    raced every other process on the host for that port.
    """
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.bind(("127.0.0.1", 0))
    sock.listen(128)
    sock.set_inheritable(True)
    return sock


def _assert_eq(observed: Any, expected: Any, label: str) -> None:
    if observed != expected:
        raise AssertionError(f"{label}: observed={observed} expected={expected}")


def _start_reference_app(gateway_bin: str, listener: socket.socket) -> subprocess.Popen[str]:
    app_dir = Path(__file__).resolve().parents[1] / "examples"
    env = {**os.environ, "META_RPC_GATEWAY_BIN": gateway_bin}
    return subprocess.Popen(
        [
            sys.executable,
            "-m",
            "uvicorn",
            "h2_reference_app:app",
            "--app-dir",
            str(app_dir),
            "--fd",
            str(listener.fileno()),
            "--log-level",
            "warning",
        ],
        env=env,
        pass_fds=(listener.fileno(),),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def _wait_for_health(
    base_url: str,
    process: subprocess.Popen[str],
    timeout_seconds: float = 15.0,
) -> None:
    deadline = time.monotonic() + timeout_seconds
    last_error: Exception | None = None
    with httpx.Client(base_url=base_url, timeout=1.0) as client:
        while time.monotonic() < deadline:
            if process.poll() is not None:
                stdout, stderr = process.communicate()
                raise RuntimeError(
                    f"reference app exited before healthz (code={process.returncode}) stdout={stdout.strip()} stderr={stderr.strip()}"
                )
            try:
                response = client.get("/healthz")
                if response.status_code == 200 and response.json().get("ok") is True:
                    return
            except Exception as exc:  # broad for startup polling diagnostics
                last_error = exc
            time.sleep(0.2)
    raise RuntimeError(
        f"timed out waiting for /healthz after {timeout_seconds:.1f}s (last_error={last_error})"
    )


def _terminate_process(process: subprocess.Popen[str] | None) -> None:
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def _request_json(
    client: httpx.Client, method: str, path: str, payload: dict[str, Any] | None = None
) -> dict[str, Any]:
    response = client.request(method, path, json=payload)
    if response.status_code != 200:
        raise AssertionError(
            f"unexpected status for {method} {path}: {response.status_code} body={response.text}"
        )
    try:
        parsed = response.json()
    except json.JSONDecodeError as exc:
        raise AssertionError(
            f"response for {method} {path} was not JSON: {response.text}"
        ) from exc
    if not isinstance(parsed, dict):
        raise AssertionError(f"response for {method} {path} must be object: {parsed}")
    return parsed


def main() -> int:
    gateway_bin = os.environ.get("META_RPC_GATEWAY_BIN")
    if not gateway_bin:
        print("META_RPC_GATEWAY_BIN must be set", file=sys.stderr)
        return 2

    checks: list[dict[str, Any]] = []

    def check(name: str, fn: Callable[[], None]) -> None:
        try:
            fn()
            checks.append({"name": name, "ok": True})
        except Exception as exc:  # broad for per-check reporting
            checks.append({"name": name, "ok": False, "error": str(exc)})

    listener = _bind_listener()
    base_url = f"http://127.0.0.1:{listener.getsockname()[1]}"
    process: subprocess.Popen[str] | None = None
    client: httpx.Client | None = None
    startup_error: Exception | None = None

    try:
        try:
            process = _start_reference_app(gateway_bin, listener)
        finally:
            # The app holds its own copy of the listening socket.
            listener.close()
        _wait_for_health(base_url, process)
        client = httpx.Client(base_url=base_url, timeout=5.0)
    except Exception as exc:  # broad for startup diagnostics
        startup_error = exc

    def _require_client() -> httpx.Client:
        if startup_error is not None:
            raise RuntimeError(f"reference app startup failed: {startup_error}")
        if client is None:
            raise RuntimeError("http client was not initialized")
        return client

    def startup_check() -> None:
        if startup_error is not None:
            raise RuntimeError(f"reference app startup failed: {startup_error}")

    check("reference app boots and responds", startup_check)

    def health_check() -> None:
        payload = _request_json(_require_client(), "GET", "/healthz")
        if payload.get("ok") is not True:
            raise AssertionError(f"unexpected /healthz payload: {payload}")
        _assert_eq(
            payload.get("gateway_bin"), gateway_bin, "gateway bin should round-trip in health"
        )

    check("health route reports gateway binding", health_check)

    def status_check() -> None:
        body = _request_json(
            _require_client(), "POST", "/rpc/status", {"request_id": "h2-status"}
        )
        envelope = body.get("jsonrpc_envelope")
        if not isinstance(envelope, dict):
            raise AssertionError(f"status envelope should be object: {envelope}")
        _assert_eq(envelope.get("jsonrpc"), "2.0", "status jsonrpc mismatch")
        _assert_eq(envelope.get("id"), "h2-status", "status id mismatch")
        result = envelope.get("result")
        if not isinstance(result, dict):
            raise AssertionError(f"status result should be object: {result}")
        _assert_eq(
            {key: result.get(key) for key in ("contract_version", "running", "loaded_modules")},
            {
                "contract_version": EXPECTED_CONTRACT_VERSION,
                "running": True,
                "loaded_modules": ["routing"],
            },
            "status envelope mismatch",
        )
        # meta/status also carries the gateway's storage census. The reference
        # app boots an ephemeral gateway, so its blob store is declared
        # ephemeral and every composed slot is listed.
        storage = result.get("storage")
        if not isinstance(storage, dict):
            raise AssertionError(f"status storage should be the storage census: {storage}")
        _assert_eq(
            storage.get("blob_durability"),
            "declared_ephemeral",
            "status storage blob_durability mismatch",
        )
        _assert_eq(
            storage.get("blob_store_persistent"),
            False,
            "status storage blob_store_persistent mismatch",
        )
        slots = storage.get("slots")
        if not isinstance(slots, list) or not slots:
            raise AssertionError(f"status storage should list its slots: {storage}")
        if body.get("typed_error") is not None:
            raise AssertionError(f"status typed call should not fail: {body['typed_error']}")
        _assert_eq(
            body.get("typed_result"),
            envelope["result"],
            "status typed result should mirror envelope result",
        )

    check("status route matches json-rpc contract", status_check)

    def capabilities_check() -> None:
        body = _request_json(
            _require_client(),
            "POST",
            "/rpc/capabilities",
            {"request_id": "h2-capabilities"},
        )
        envelope = body.get("jsonrpc_envelope")
        if not isinstance(envelope, dict):
            raise AssertionError(f"capabilities envelope should be object: {envelope}")
        _assert_eq(envelope.get("jsonrpc"), "2.0", "capabilities jsonrpc mismatch")
        _assert_eq(envelope.get("id"), "h2-capabilities", "capabilities id mismatch")
        result = envelope.get("result")
        if not isinstance(result, dict):
            raise AssertionError(f"capabilities result should be object: {result}")
        _assert_eq(
            result.get("contract_version"),
            EXPECTED_CONTRACT_VERSION,
            "capabilities contract_version mismatch",
        )
        _assert_eq(
            result.get("loaded_modules"),
            ["routing"],
            "capabilities loaded_modules mismatch",
        )
        methods = result.get("methods")
        if not isinstance(methods, list):
            raise AssertionError(f"capabilities methods should be list: {methods}")
        missing_methods = [
            method for method in EXPECTED_CAPABILITY_METHODS if method not in methods
        ]
        if missing_methods:
            raise AssertionError(
                f"capabilities methods missing expected subset: {missing_methods}"
            )
        if body.get("typed_error") is not None:
            raise AssertionError(
                f"capabilities typed call should not fail: {body['typed_error']}"
            )
        _assert_eq(
            body.get("typed_result", {}).get("contract_version"),
            result.get("contract_version"),
            "capabilities typed result contract_version mismatch",
        )
        _assert_eq(
            body.get("typed_result", {}).get("loaded_modules"),
            result.get("loaded_modules"),
            "capabilities typed result loaded_modules mismatch",
        )

    check("capabilities route matches json-rpc contract", capabilities_check)

    def reconcile_check() -> None:
        body = _request_json(
            _require_client(),
            "POST",
            "/rpc/reconcile",
            {"request_id": "h2-reconcile", "modules": ["routing"]},
        )
        envelope = body.get("jsonrpc_envelope")
        _assert_eq(
            envelope,
            {
                "jsonrpc": "2.0",
                "id": "h2-reconcile",
                "result": {
                    "accepted": True,
                    "reconciled_modules": ["routing"],
                    "added": 0,
                },
            },
            "reconcile envelope mismatch",
        )
        if body.get("typed_error") is not None:
            raise AssertionError(f"reconcile typed call should not fail: {body['typed_error']}")
        _assert_eq(
            body.get("typed_result"),
            envelope["result"],
            "reconcile typed result should mirror envelope result",
        )

    check("reconcile route matches json-rpc contract", reconcile_check)

    def spawn_member_check() -> None:
        body = _request_json(
            _require_client(),
            "POST",
            "/rpc/spawn_member",
            {"request_id": "h2-spawn", "module_id": "routing"},
        )
        envelope = body.get("jsonrpc_envelope")
        _assert_eq(
            envelope.get("jsonrpc"),
            "2.0",
            "spawn_member jsonrpc mismatch",
        )
        _assert_eq(
            envelope.get("id"),
            "h2-spawn",
            "spawn_member id mismatch",
        )
        result = envelope.get("result")
        if not isinstance(result, dict):
            raise AssertionError(f"spawn_member result should be object: {result}")
        _assert_eq(result.get("accepted"), True, "spawn_member accepted mismatch")
        _assert_eq(result.get("module_id"), "routing", "spawn_member module_id mismatch")
        if body.get("typed_error") is not None:
            raise AssertionError(
                f"spawn_member typed call should not fail: {body['typed_error']}"
            )
        _assert_eq(
            body.get("typed_result", {}).get("accepted"),
            result.get("accepted"),
            "spawn_member typed accepted mismatch",
        )
        _assert_eq(
            body.get("typed_result", {}).get("module_id"),
            result.get("module_id"),
            "spawn_member typed module_id mismatch",
        )

    check("spawn_member route matches json-rpc contract", spawn_member_check)

    def events_subscribe_check() -> None:
        body = _request_json(
            _require_client(),
            "POST",
            "/rpc/events/subscribe",
            {"request_id": "h2-events", "scope": "mob"},
        )
        envelope = body.get("jsonrpc_envelope")
        if not isinstance(envelope, dict):
            raise AssertionError(f"events envelope should be object: {envelope}")
        _assert_eq(envelope.get("jsonrpc"), "2.0", "events jsonrpc version mismatch")
        _assert_eq(envelope.get("id"), "h2-events", "events response id mismatch")

        result = envelope.get("result")
        if not isinstance(result, dict):
            raise AssertionError(f"events result should be object: {result}")
        _assert_eq(result.get("scope"), "mob", "events scope mismatch")
        _assert_eq(
            result.get("replay_from_event_id"),
            None,
            "events replay_from_event_id mismatch",
        )
        _assert_eq(
            result.get("keep_alive"),
            {"interval_ms": 15000, "event": "keep-alive"},
            "events keep_alive mismatch",
        )
        _assert_eq(
            result.get("keep_alive_comment"),
            ": keep-alive\n\n",
            "events keep_alive_comment mismatch",
        )

        frames = result.get("event_frames")
        if not isinstance(frames, list) or len(frames) < 1:
            raise AssertionError(f"unexpected event_frames: {frames}")
        first_frame = frames[0]
        if (
            not isinstance(first_frame, str)
            or "id: evt-routing" not in first_frame
            or "event: ready" not in first_frame
        ):
            raise AssertionError(f"unexpected first event frame: {first_frame}")

        events = result.get("events")
        if not isinstance(events, list) or len(events) < 1:
            raise AssertionError(f"unexpected events payload: {events}")
        first_event = events[0]
        if not isinstance(first_event, dict):
            raise AssertionError(f"unexpected first event type: {first_event}")
        _assert_eq(first_event.get("event_id"), "evt-routing", "first event_id mismatch")
        _assert_eq(first_event.get("source"), "module", "first event source mismatch")
        _assert_eq(first_event.get("timestamp_ms"), 101, "first event timestamp mismatch")
        event_payload = first_event.get("event")
        if not isinstance(event_payload, dict):
            raise AssertionError(f"unexpected first event payload: {event_payload}")
        _assert_eq(event_payload.get("kind"), "module", "first event kind mismatch")
        _assert_eq(event_payload.get("module"), "routing", "first event module mismatch")
        _assert_eq(event_payload.get("event_type"), "ready", "first event_type mismatch")

        if body.get("typed_error") is not None:
            raise AssertionError(
                f"events typed subscribe should not fail: {body['typed_error']}"
            )
        typed_result = body.get("typed_result")
        if not isinstance(typed_result, dict):
            raise AssertionError(f"typed events result should be object: {typed_result}")
        _assert_eq(
            typed_result.get("scope"),
            result.get("scope"),
            "typed events scope mismatch",
        )
        _assert_eq(
            typed_result.get("keep_alive"),
            result.get("keep_alive"),
            "typed events keep_alive mismatch",
        )
        typed_events = typed_result.get("events")
        if not isinstance(typed_events, list) or len(typed_events) < 1:
            raise AssertionError(f"typed events should include one event: {typed_events}")
        _assert_eq(
            typed_events[0].get("event_id"),
            "evt-routing",
            "typed first event_id mismatch",
        )

    check("events subscribe route matches json-rpc contract", events_subscribe_check)

    def spawn_member_invalid_check() -> None:
        body = _request_json(
            _require_client(),
            "POST",
            "/rpc/spawn_member",
            {"request_id": "h2-spawn-invalid"},
        )
        envelope = body.get("jsonrpc_envelope")
        _assert_eq(
            envelope,
            {
                "jsonrpc": "2.0",
                "id": "h2-spawn-invalid",
                "error": {
                    "code": -32602,
                    "message": "Invalid params: module_id required",
                },
            },
            "spawn_member invalid envelope mismatch",
        )
        typed_error = body.get("typed_error")
        if not isinstance(typed_error, dict):
            raise AssertionError(f"spawn_member typed_error should be object: {typed_error}")
        _assert_eq(
            typed_error.get("code"),
            -32602,
            "spawn_member typed error code mismatch",
        )
        _assert_eq(
            typed_error.get("message"),
            "Invalid params: module_id required",
            "spawn_member typed error message mismatch",
        )
        _assert_eq(
            typed_error.get("method"),
            "meta/spawn_member",
            "spawn_member typed error method mismatch",
        )

    check(
        "spawn_member invalid params matches rust parity error",
        spawn_member_invalid_check,
    )

    def events_agent_validation_check() -> None:
        body = _request_json(
            _require_client(),
            "POST",
            "/rpc/events/subscribe",
            {"request_id": "h2-events-agent-missing", "scope": "agent"},
        )
        envelope = body.get("jsonrpc_envelope")
        _assert_eq(
            envelope,
            {
                "jsonrpc": "2.0",
                "id": "h2-events-agent-missing",
                "error": {
                    "code": -32602,
                    "message": "Invalid params: failed to subscribe to unified events: MissingAgentId",
                },
            },
            "events agent validation envelope mismatch",
        )
        typed_error = body.get("typed_error")
        if not isinstance(typed_error, dict):
            raise AssertionError(f"events typed_error should be object: {typed_error}")
        _assert_eq(typed_error.get("code"), -32602, "events typed error code mismatch")
        _assert_eq(
            typed_error.get("message"),
            "Invalid params: failed to subscribe to unified events: MissingAgentId",
            "events typed error message mismatch",
        )
        _assert_eq(
            typed_error.get("method"),
            "meta/events/subscribe",
            "events typed error method mismatch",
        )

    check(
        "events subscribe agent validation matches rust parity error",
        events_agent_validation_check,
    )

    def flow_route_check() -> None:
        flow = _request_json(_require_client(), "GET", "/flow/reference")
        _assert_eq(flow.get("route"), "h2-flow", "flow route id mismatch")

        expected_ids = {
            "status": "h2-flow-status",
            "capabilities": "h2-flow-capabilities",
            "reconcile": "h2-flow-reconcile",
            "spawn_member": "h2-flow-spawn",
            "events_subscribe": "h2-flow-events",
        }
        for key, expected_id in expected_ids.items():
            envelope = flow.get(key)
            if not isinstance(envelope, dict):
                raise AssertionError(f"flow envelope {key} should be object: {envelope}")
            _assert_eq(
                envelope.get("jsonrpc"),
                "2.0",
                f"flow envelope {key} jsonrpc mismatch",
            )
            _assert_eq(envelope.get("id"), expected_id, f"flow envelope {key} id mismatch")

        typed = flow.get("typed")
        if not isinstance(typed, dict):
            raise AssertionError(f"flow typed payload should be object: {typed}")

        _assert_eq(
            flow["status"]["result"],
            typed.get("status"),
            "flow typed status should match status envelope",
        )
        _assert_eq(
            flow["capabilities"]["result"],
            typed.get("capabilities"),
            "flow typed capabilities should match capabilities envelope",
        )
        _assert_eq(
            flow["reconcile"]["result"],
            typed.get("reconcile"),
            "flow typed reconcile should match reconcile envelope",
        )
        _assert_eq(
            flow["spawn_member"]["result"],
            typed.get("spawn_member"),
            "flow typed spawn should match spawn envelope",
        )
        _assert_eq(
            flow["events_subscribe"]["result"]["scope"],
            "mob",
            "flow events scope mismatch",
        )
        _assert_eq(
            typed.get("events_subscribe", {}).get("scope"),
            "mob",
            "flow typed events scope mismatch",
        )

    check("reference flow route executes end-to-end", flow_route_check)

    if client is not None:
        client.close()
    _terminate_process(process)

    failed = sum(1 for item in checks if not item["ok"])
    passed = len(checks) - failed
    summary = {
        "sdk": "python",
        "suite": "h2_reference_flow",
        "passed": passed,
        "failed": failed,
        "checks": checks,
    }
    print(json.dumps(summary), end="")
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
