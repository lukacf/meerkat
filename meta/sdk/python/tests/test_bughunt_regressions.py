"""Regression tests for the Python SDK bug-hunt fixes.

Each test pins one specific behavior identified by the bug-hunt sweep:
- AsgiApp /rpc must surface the real RPC code/message on RpcError.
- _transport.send_sync must reject empty/duplicate ids.
- EventEnvelope / KeepAliveConfig must coerce numeric fields.
- MobEvent.from_sse / AgentEvent.from_sse must preserve non-dict
  payload values instead of silently dropping them.
- subscribe_mob_events must accept both dict-envelope and bare-list
  shapes.
"""

from __future__ import annotations

import asyncio
from concurrent.futures import TimeoutError as FutureTimeoutError
import json
import subprocess
import threading
from typing import Any
from unittest.mock import AsyncMock, MagicMock, call, patch

import pytest

from meerkat_meta._sse import SseEvent
from meerkat_meta._transport import (
    _GATEWAY_SHUTDOWN_GRACE_SECONDS,
    _PROCESS_KILL_GRACE_SECONDS,
    _PROCESS_TERMINATE_GRACE_SECONDS,
    _PROVIDER_CALLBACK_COMPLETION_SECONDS,
    PersistentTransport,
    TransportTermination,
)
from meerkat_meta.errors import RpcError
from meerkat_meta.events import AgentEvent, MobEvent
from meerkat_meta.types import EventEnvelope, KeepAliveConfig


# -- Bug #4: AsgiApp /rpc no longer raises AttributeError on RpcError --

@pytest.mark.asyncio
async def test_asgi_rpc_handler_returns_real_rpc_error_code_and_message():
    """Pre-fix: AsgiApp's /rpc handler read `exc.message` (which doesn't
    exist on RpcError), fell through to `except Exception`, and returned
    -32603 / `'RpcError' object has no attribute 'message'`.
    """
    from meerkat_meta.runtime import AsgiApp

    runtime = MagicMock()
    runtime._rpc = AsyncMock(
        side_effect=RpcError(
            code=-32001,
            message="boom",
            request_id="rid",
            method="meta/status",
            data={"detail": "x"},
        )
    )

    app = AsgiApp.__new__(AsgiApp)
    app._runtime = runtime
    app._console = True
    app._auth_config = None
    app._fallback_app = None

    request_body = json.dumps(
        {"jsonrpc": "2.0", "id": 1, "method": "meta/status", "params": {}}
    ).encode()

    received: list[dict] = []
    sent: list[dict] = []

    async def receive():
        return {"type": "http.request", "body": request_body, "more_body": False}

    async def send(message):
        sent.append(message)

    scope = {"type": "http", "method": "POST", "path": "/rpc", "headers": []}
    await app(scope, receive, send)

    body_bytes = b""
    for msg in sent:
        if msg.get("type") == "http.response.body":
            body_bytes += msg.get("body", b"")
    body = json.loads(body_bytes.decode())

    assert body["error"]["code"] == -32001, (
        f"AsgiApp must surface the original RPC code; got {body['error']}"
    )
    assert body["error"]["message"] == "boom"
    assert body["error"].get("data") == {"detail": "x"}


# -- Bug #15: _transport.send_sync rejects empty/duplicate ids --

def test_send_sync_rejects_request_with_no_id():
    """Pre-fix, requests without `id` collided on `_pending[""]`,
    deadlocking concurrent callers for the full timeout.
    """
    transport = PersistentTransport.__new__(PersistentTransport)
    transport._pending = {}
    transport._results = {}
    transport._pending_lock = threading.Lock()
    transport._write_lock = threading.Lock()
    transport._timeout = 1.0
    transport._process = MagicMock()  # passes _ensure_running gate
    transport._ensure_running = lambda: None

    with pytest.raises(ValueError, match="non-empty `id`"):
        transport.send_sync({"jsonrpc": "2.0", "method": "x", "params": {}})


def test_send_sync_rejects_duplicate_id_already_in_flight():
    transport = PersistentTransport.__new__(PersistentTransport)
    transport._pending = {"abc": threading.Event()}  # someone already waiting
    transport._results = {}
    transport._pending_lock = threading.Lock()
    transport._write_lock = threading.Lock()
    transport._timeout = 1.0
    transport._process = MagicMock()
    transport._ensure_running = lambda: None

    with pytest.raises(ValueError, match="already"):
        transport.send_sync({"jsonrpc": "2.0", "id": "abc", "method": "x"})


@pytest.mark.asyncio
async def test_send_async_forwards_per_request_timeout():
    transport = PersistentTransport("unused")
    transport.send_sync = MagicMock(return_value={"result": {}})
    request = {"jsonrpc": "2.0", "id": "wait-1", "method": "wait"}

    response = await transport.send_async(request, timeout=125.0)

    assert response == {"result": {}}
    transport.send_sync.assert_called_once_with(request, timeout=125.0)


@pytest.mark.parametrize("request_timeout", [None, 0.01, 600.0])
def test_provider_callback_uses_dedicated_completion_deadline(request_timeout):
    assert _PROVIDER_CALLBACK_COMPLETION_SECONDS == 125.0
    options = {} if request_timeout is None else {"timeout": request_timeout}
    transport = PersistentTransport("unused", **options)
    transport._loop = MagicMock()
    transport._loop.is_running.return_value = True
    transport._callback_handler = AsyncMock(return_value={"released": True})
    transport._write_line = MagicMock()
    future = MagicMock()
    future.result.return_value = {"released": True}

    def schedule(coroutine, loop):
        assert loop is transport._loop
        coroutine.close()
        return future

    with patch(
        "meerkat_meta._transport.asyncio.run_coroutine_threadsafe",
        side_effect=schedule,
    ):
        transport._dispatch_callback(
            {
                "jsonrpc": "2.0",
                "id": "cb-dedicated-deadline",
                "method": "callback/lease/release",
                "params": {},
            }
        )

    future.result.assert_called_once_with(
        timeout=_PROVIDER_CALLBACK_COMPLETION_SECONDS
    )
    future.cancel.assert_not_called()
    transport._write_line.assert_called_once_with(
        {
            "jsonrpc": "2.0",
            "id": "cb-dedicated-deadline",
            "result": {"released": True},
        },
        callback_response=True,
        process=None,
    )


def test_provider_callback_timeout_cancels_event_loop_future():
    transport = PersistentTransport("unused", timeout=0.01)
    transport._loop = MagicMock()
    transport._loop.is_running.return_value = True
    transport._callback_handler = AsyncMock(return_value=None)
    transport._write_line = MagicMock()
    future = MagicMock()
    future.result.side_effect = FutureTimeoutError()
    future.done.return_value = False

    def schedule(coroutine, loop):
        assert loop is transport._loop
        coroutine.close()
        return future

    with patch(
        "meerkat_meta._transport.asyncio.run_coroutine_threadsafe",
        side_effect=schedule,
    ):
        transport._dispatch_callback(
            {
                "jsonrpc": "2.0",
                "id": "cb-timeout",
                "method": "callback/lease/release",
                "params": {},
            }
        )

    future.result.assert_called_once_with(
        timeout=_PROVIDER_CALLBACK_COMPLETION_SECONDS
    )
    future.cancel.assert_called_once_with()
    response = transport._write_line.call_args.args[0]
    assert response["id"] == "cb-timeout"
    assert response["error"]["code"] == -32000
    assert "125s host completion deadline" in response["error"]["message"]


def test_provider_raised_timeout_error_is_not_rewritten_as_host_deadline():
    transport = PersistentTransport("unused")
    transport._loop = MagicMock()
    transport._loop.is_running.return_value = True
    transport._callback_handler = AsyncMock(return_value=None)
    transport._write_line = MagicMock()
    future = MagicMock()
    future.result.side_effect = FutureTimeoutError("provider-owned timeout")
    future.done.return_value = True

    def schedule(coroutine, loop):
        assert loop is transport._loop
        coroutine.close()
        return future

    with patch(
        "meerkat_meta._transport.asyncio.run_coroutine_threadsafe",
        side_effect=schedule,
    ):
        transport._dispatch_callback(
            {
                "jsonrpc": "2.0",
                "id": "cb-provider-timeout",
                "method": "callback/lease/release",
                "params": {},
            }
        )

    future.cancel.assert_not_called()
    response = transport._write_line.call_args.args[0]
    assert response["id"] == "cb-provider-timeout"
    assert response["error"]["message"] == "provider-owned timeout"


def test_reader_drops_response_after_request_is_no_longer_pending():
    transport = PersistentTransport("unused")
    process = MagicMock()
    process.stdout.readline.side_effect = [
        b'{"jsonrpc":"2.0","id":"late","result":{}}\n',
        b"",
    ]
    transport._process = process
    transport._reader_loop()

    assert transport._results == {}


def test_init_negotiates_explicit_gateway_shutdown_horizon():
    transport = PersistentTransport("unused")
    transport._ensure_running = lambda: None
    transport._send_sync_running = MagicMock(
        return_value={
            "jsonrpc": "2.0",
            "id": "init-horizon",
            "result": {
                "stdio_shutdown_handshake": True,
                "stdio_shutdown_horizon_ms": 321_000,
            },
        }
    )

    transport.send_sync(
        {
            "jsonrpc": "2.0",
            "id": "init-horizon",
            "method": "meta/init",
            "params": {},
        }
    )

    assert transport._supports_shutdown_handshake is True
    assert transport._shutdown_horizon_seconds == 321.0


@pytest.mark.parametrize("invalid_horizon", [None, True, 0, -1, 1.5, "335000"])
def test_init_uses_safe_shutdown_horizon_for_invalid_capability(invalid_horizon):
    assert _GATEWAY_SHUTDOWN_GRACE_SECONDS == 337.0
    transport = PersistentTransport("unused")
    transport._ensure_running = lambda: None
    transport._send_sync_running = MagicMock(
        return_value={
            "jsonrpc": "2.0",
            "id": "init-invalid-horizon",
            "result": {
                "stdio_shutdown_handshake": True,
                "stdio_shutdown_horizon_ms": invalid_horizon,
            },
        }
    )

    transport.send_sync(
        {
            "jsonrpc": "2.0",
            "id": "init-invalid-horizon",
            "method": "meta/init",
            "params": {},
        }
    )

    assert transport._supports_shutdown_handshake is True
    assert transport._shutdown_horizon_seconds == _GATEWAY_SHUTDOWN_GRACE_SECONDS


def test_start_resets_shutdown_capabilities_for_replacement_process():
    transport = PersistentTransport("unused")
    transport._supports_shutdown_handshake = True
    transport._shutdown_horizon_seconds = 321.0
    process = MagicMock()
    process.poll.return_value = None
    process.stdout.readline.return_value = b""

    with patch(
        "meerkat_meta._transport.subprocess.Popen",
        return_value=process,
    ):
        transport.start()

    assert transport._supports_shutdown_handshake is False
    assert transport._shutdown_horizon_seconds == _GATEWAY_SHUTDOWN_GRACE_SECONDS
    transport._process = None


def test_stop_gives_gateway_full_cleanup_budget_before_termination():
    transport = PersistentTransport("unused")
    process = MagicMock()
    process.poll.return_value = None
    process.wait.side_effect = [
        subprocess.TimeoutExpired("rpc_gateway", _GATEWAY_SHUTDOWN_GRACE_SECONDS),
        subprocess.TimeoutExpired("rpc_gateway", _PROCESS_TERMINATE_GRACE_SECONDS),
        0,
    ]
    transport._process = process
    transport._request_gateway_shutdown = MagicMock(
        side_effect=RuntimeError("old gateway")
    )

    with patch(
        "meerkat_meta._transport.time.monotonic",
        side_effect=[100.0, 100.0],
    ):
        transport.stop()

    process.stdin.close.assert_called_once_with()
    assert process.wait.call_args_list == [
        call(timeout=_GATEWAY_SHUTDOWN_GRACE_SECONDS),
        call(timeout=_PROCESS_TERMINATE_GRACE_SECONDS),
        call(timeout=_PROCESS_KILL_GRACE_SECONDS),
    ]
    process.terminate.assert_called_once_with()
    process.kill.assert_called_once_with()
    transport._request_gateway_shutdown.assert_not_called()
    assert transport._process is None
    assert transport.last_termination == TransportTermination(
        "exited", 0, None, "sigkill", "not_negotiated"
    )


def test_stop_rejects_and_retains_unreaped_gateway_after_sigkill():
    transport = PersistentTransport("unused")
    process = MagicMock()
    process.poll.return_value = None
    process.wait.side_effect = [
        subprocess.TimeoutExpired("rpc_gateway", _GATEWAY_SHUTDOWN_GRACE_SECONDS),
        subprocess.TimeoutExpired("rpc_gateway", _PROCESS_TERMINATE_GRACE_SECONDS),
        subprocess.TimeoutExpired("rpc_gateway", _PROCESS_KILL_GRACE_SECONDS),
    ]
    transport._process = process

    with patch(
        "meerkat_meta._transport.time.monotonic",
        side_effect=[100.0, 100.0],
    ), pytest.raises(RuntimeError, match="did not terminate after bounded cleanup"):
        transport.stop()

    process.terminate.assert_called_once_with()
    process.kill.assert_called_once_with()
    assert transport._process is process
    assert transport.last_termination == TransportTermination(
        "unknown", None, None, "sigkill", "not_negotiated"
    )

    # A later stop finishes the same child and keeps the attempted escalation;
    # the child already had its horizon, so it is not waited out again.
    process.wait.side_effect = None
    process.wait.return_value = -9
    with patch(
        "meerkat_meta._transport.time.monotonic",
        side_effect=[200.0, 200.0],
    ):
        transport.stop()

    assert process.wait.call_args_list[-1] == call(timeout=0.0)
    process.terminate.assert_called_once_with()
    process.kill.assert_called_once_with()
    assert transport._process is None
    assert transport.last_termination == TransportTermination(
        "exited", None, "SIGKILL", "sigkill", "not_negotiated"
    )


def test_stop_honors_negotiated_horizon_and_waits_for_gated_shutdown_callback():
    transport = PersistentTransport("unused")
    process = MagicMock()
    process.poll.return_value = None
    process.wait.return_value = 0
    transport._process = process
    transport._supports_shutdown_handshake = True
    transport._shutdown_horizon_seconds = 321.0
    gate = threading.Barrier(2)

    def answer_shutdown(request, *, timeout, process):
        assert process is transport._process
        process.stdin.close.assert_not_called()
        assert request["method"] == "meta/shutdown"
        assert request["id"].startswith("meta-shutdown-")
        assert timeout == 321.0
        gate.wait(timeout=2)
        gate.wait(timeout=2)
        return {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "shutdown": True,
                "runtime_cleanup_completed": True,
            },
        }

    transport._send_sync_running = MagicMock(side_effect=answer_shutdown)
    errors: list[BaseException] = []

    def stop_transport():
        try:
            transport.stop()
        except BaseException as exc:  # pragma: no cover - asserted below
            errors.append(exc)

    # Patch the transport module's `time` binding rather than the process-wide
    # `time.monotonic` function. `threading.Barrier` also uses monotonic time on
    # current Python versions and must remain real while this callback is gated.
    with patch("meerkat_meta._transport.time") as transport_time:
        transport_time.monotonic.return_value = 100.0
        worker = threading.Thread(target=stop_transport)
        worker.start()
        gate.wait(timeout=2)
        process.stdin.close.assert_not_called()
        assert worker.is_alive(), "stop must remain gated on the callback-backed handshake"
        gate.wait(timeout=2)
        worker.join(timeout=2)

    assert not worker.is_alive()
    assert errors == []
    process.stdin.close.assert_called_once_with()
    process.wait.assert_called_once_with(timeout=321.0)
    assert transport._process is None
    assert transport.last_termination == TransportTermination(
        "exited", 0, None, "none", "acknowledged"
    )


_MISSING_CLEANUP_ATTESTATION = object()


@pytest.mark.parametrize(
    "cleanup_attestation",
    [_MISSING_CLEANUP_ATTESTATION, None, "true", False, 1],
)
def test_stop_rejects_non_true_cleanup_attestation_after_reaping_gateway(
    cleanup_attestation,
):
    transport = PersistentTransport("unused")
    process = MagicMock()
    process.poll.return_value = None
    process.wait.return_value = 0
    transport._process = process
    transport._supports_shutdown_handshake = True
    transport._shutdown_horizon_seconds = 321.0
    result = {"shutdown": True}
    if cleanup_attestation is not _MISSING_CLEANUP_ATTESTATION:
        result["runtime_cleanup_completed"] = cleanup_attestation
    transport._send_sync_running = MagicMock(
        return_value={
            "jsonrpc": "2.0",
            "id": "shutdown-failed",
            "result": result,
        }
    )

    with patch(
        "meerkat_meta._transport.time.monotonic",
        side_effect=[100.0, 100.0],
    ), pytest.raises(RuntimeError, match="failed after bounded cleanup"):
        transport.stop()

    process.stdin.close.assert_called_once_with()
    process.wait.assert_called_once_with(timeout=321.0)
    assert transport._process is None
    assert transport.last_termination == TransportTermination(
        "exited", 0, None, "none", "refused"
    )


def _stop_with_shutdown_result(result):
    transport = PersistentTransport("unused")
    process = MagicMock()
    process.poll.return_value = None
    process.wait.return_value = 0
    transport._process = process
    transport._supports_shutdown_handshake = True
    transport._shutdown_horizon_seconds = 321.0
    transport._send_sync_running = MagicMock(
        return_value={"jsonrpc": "2.0", "id": "shutdown", "result": result}
    )
    with patch(
        "meerkat_meta._transport.time.monotonic",
        side_effect=[100.0, 100.0],
    ):
        transport.stop()
    process.wait.assert_called_once_with(timeout=321.0)
    assert transport._process is None
    return transport


def test_stop_reaps_without_failing_on_process_exit_child_mob_retention():
    # Cleanup is still incomplete; the only cause is durable child mobs held
    # until the gateway exits, which the reap releases.
    transport = _stop_with_shutdown_result(
        {
            "shutdown": False,
            "runtime_cleanup_completed": False,
            "runtime_cleanup_retained_until_exit": True,
        }
    )
    assert transport.last_termination == TransportTermination(
        "exited", 0, None, "none", "retained_until_exit"
    )


@pytest.mark.parametrize(
    "result",
    [
        {"shutdown": False, "runtime_cleanup_completed": False},
        {
            "shutdown": False,
            "runtime_cleanup_completed": False,
            "runtime_cleanup_retained_until_exit": "true",
        },
        {
            "shutdown": False,
            "runtime_cleanup_completed": None,
            "runtime_cleanup_retained_until_exit": True,
        },
    ],
)
def test_stop_still_fails_on_cleanup_that_is_not_process_exit_retention(result):
    with pytest.raises(RuntimeError, match="failed after bounded cleanup"):
        _stop_with_shutdown_result(result)


# -- Termination record: what a stop observed for its own child --

def _negotiated_transport(process, answer):
    transport = PersistentTransport("unused")
    transport._process = process
    transport._supports_shutdown_handshake = True
    transport._shutdown_horizon_seconds = 321.0
    transport._send_sync_running = MagicMock(**answer)
    return transport


def _surviving_process():
    process = MagicMock()
    process.poll.return_value = None
    process.wait.side_effect = subprocess.TimeoutExpired("rpc_gateway", 1)
    return process


@pytest.mark.parametrize(
    ("status", "exit_code", "signal"),
    [(0, 0, None), (3, 3, None), (-9, None, "SIGKILL"), (-15, None, "SIGTERM")],
)
def test_stop_records_the_observed_exit_status(status, exit_code, signal):
    transport = PersistentTransport("unused")
    process = MagicMock()
    process.poll.return_value = None
    process.wait.return_value = status
    transport._process = process

    transport.stop()

    assert transport.last_termination == TransportTermination(
        "exited", exit_code, signal, "none", "not_negotiated"
    )


def test_stop_records_an_unanswered_shutdown_request_apart_from_the_exit():
    process = MagicMock()
    process.poll.return_value = None
    process.wait.return_value = 0
    transport = _negotiated_transport(
        process,
        {
            "side_effect": RuntimeError(
                "persistent transport: timeout after 321.0s waiting for response"
            )
        },
    )

    with patch(
        "meerkat_meta._transport.time.monotonic",
        side_effect=[100.0, 100.0],
    ), pytest.raises(RuntimeError, match="failed after bounded cleanup"):
        transport.stop()

    assert transport.last_termination == TransportTermination(
        "exited", 0, None, "none", "unanswered"
    )


def test_a_later_stop_finishes_the_same_child_without_asking_it_again():
    process = _surviving_process()
    transport = _negotiated_transport(
        process,
        {
            "return_value": {
                "jsonrpc": "2.0",
                "id": "shutdown",
                "result": {"shutdown": True, "runtime_cleanup_completed": True},
            }
        },
    )
    with patch(
        "meerkat_meta._transport.time.monotonic",
        side_effect=[100.0, 100.0],
    ), pytest.raises(RuntimeError, match="did not terminate after bounded cleanup"):
        transport.stop()
    assert transport._process is process
    assert transport.last_termination == TransportTermination(
        "unknown", None, None, "sigkill", "acknowledged"
    )

    process.wait.side_effect = [subprocess.TimeoutExpired("rpc_gateway", 1), -9]
    with patch(
        "meerkat_meta._transport.time.monotonic",
        side_effect=[200.0, 200.0],
    ):
        transport.stop()

    # The child's one shutdown request was answered; the stdin it used is
    # closed, so asking again could only report a false failure. It already
    # had its horizon: the later stop goes straight to the signals.
    transport._send_sync_running.assert_called_once()
    assert process.wait.call_args_list[-2:] == [
        call(timeout=0.0),
        call(timeout=_PROCESS_TERMINATE_GRACE_SECONDS),
    ]
    assert process.terminate.call_count == 2
    assert transport._process is None
    assert transport.last_termination == TransportTermination(
        "exited", None, "SIGKILL", "sigkill", "acknowledged"
    )


def test_a_refused_shutdown_is_reported_by_the_stop_that_reaps_the_child():
    process = _surviving_process()
    transport = _negotiated_transport(
        process,
        {
            "return_value": {
                "jsonrpc": "2.0",
                "id": "shutdown",
                "error": {"code": -32000, "message": "cleanup refused"},
            }
        },
    )
    with patch(
        "meerkat_meta._transport.time.monotonic",
        side_effect=[100.0, 100.0],
    ), pytest.raises(RuntimeError, match="did not terminate after bounded cleanup"):
        transport.stop()
    assert transport.last_termination == TransportTermination(
        "unknown", None, None, "sigkill", "refused"
    )

    process.wait.side_effect = None
    process.wait.return_value = 0
    with patch(
        "meerkat_meta._transport.time.monotonic",
        side_effect=[200.0, 200.0],
    ), pytest.raises(RuntimeError, match="shutdown failed after bounded cleanup") as failed:
        transport.stop()
    assert "cleanup refused" in str(failed.value.__cause__)
    transport._send_sync_running.assert_called_once()
    reaped = TransportTermination("exited", 0, None, "sigkill", "refused")
    assert transport.last_termination == reaped

    # The child is released: a further stop has nothing left to stop.
    transport.stop()
    assert transport.last_termination == reaped


def test_a_stop_with_nothing_to_stop_never_overwrites_a_record():
    transport = PersistentTransport("unused")
    assert transport.last_termination is None
    transport.stop()
    assert transport.last_termination == TransportTermination(
        "not_started", None, None, "none", "not_negotiated"
    )

    transport = PersistentTransport("unused")
    process = MagicMock()
    process.poll.return_value = None
    process.wait.return_value = 0
    transport._process = process
    transport.stop()
    recorded = transport.last_termination
    transport.stop()
    assert transport.last_termination is recorded
    assert recorded == TransportTermination("exited", 0, None, "none", "not_negotiated")


def test_a_new_child_starts_without_the_previous_record():
    transport = PersistentTransport("unused")
    stopped = MagicMock()
    stopped.poll.return_value = None
    stopped.wait.return_value = 0
    transport._process = stopped
    transport.stop()
    assert transport.last_termination is not None

    process = MagicMock()
    process.poll.return_value = None
    process.stdout.readline.return_value = b""
    with patch(
        "meerkat_meta._transport.subprocess.Popen",
        return_value=process,
    ):
        transport.start()

    assert transport.last_termination is None
    transport._process = None


def test_a_failed_spawn_records_not_started():
    transport = PersistentTransport("unused")
    with patch(
        "meerkat_meta._transport.subprocess.Popen",
        side_effect=FileNotFoundError("no gateway binary"),
    ), pytest.raises(FileNotFoundError):
        transport.start()

    assert transport.last_termination == TransportTermination(
        "not_started", None, None, "none", "not_negotiated"
    )


# -- Bug #16: numeric field coercion --

def test_event_envelope_coerces_none_timestamp_to_zero():
    env = EventEnvelope.from_dict(
        {"event_id": "x", "source": "mob", "timestamp_ms": None, "event": {}}
    )
    assert env.timestamp_ms == 0
    assert isinstance(env.timestamp_ms, int)


def test_event_envelope_coerces_string_timestamp_to_int():
    env = EventEnvelope.from_dict(
        {"event_id": "x", "source": "mob", "timestamp_ms": "1700", "event": {}}
    )
    assert env.timestamp_ms == 1700
    assert isinstance(env.timestamp_ms, int)


def test_keep_alive_config_coerces_string_interval():
    cfg = KeepAliveConfig.from_dict({"interval_ms": "15", "event": "ka"})
    assert cfg.interval_ms == 15
    assert isinstance(cfg.interval_ms, int)


# -- Bug #17: from_sse non-dict payload --

def test_mob_event_from_sse_with_non_dict_payload_preserves_raw():
    sse = SseEvent(id=None, event="message", data='{"member_id":"a","payload":"hello"}')
    ev = MobEvent.from_sse(sse)
    # The agent-side parse failed (payload was a string); we should
    # carry the raw value instead of dropping it.
    assert ev.event.type == "non_dict_payload", (
        f"Expected non_dict_payload UnknownEvent, got {ev.event!r}"
    )
    assert ev.event.data == {"raw": "hello"}
    assert ev.member_id == "a"  # member_id survives


def test_agent_event_from_sse_with_string_top_level_preserves_raw():
    sse = SseEvent(id=None, event="message", data='"hi"')
    ev = AgentEvent.from_sse(sse)
    assert ev.event.type == "non_dict_payload"
    assert ev.event.data == {"raw": "hi"}


# -- Bug #24: subscribe_mob_events accepts bare list --

@pytest.mark.asyncio
async def test_subscribe_mob_events_handles_bare_list_response():
    """Pre-fix, only dict envelopes were honored; a bare list yielded
    nothing. query_mob_events handled both shapes — divergent behavior
    silently dropped events.
    """
    from meerkat_meta.runtime import MobHandle

    runtime = MagicMock()
    runtime._rpc = AsyncMock(
        return_value=[
            {
                "event_id": "e1",
                "cursor": 1,
                "mob_id": "m",
                "timestamp_ms": 0,
                "kind": "k",
                "data": {},
            }
        ]
    )
    handle = MobHandle.__new__(MobHandle)
    handle._runtime = runtime

    received = []
    async for ev in handle.subscribe_mob_events():
        received.append(ev)

    assert len(received) == 1, "must yield events from bare-list response"
    assert received[0].event_id == "e1"
