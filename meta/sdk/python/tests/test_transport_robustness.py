"""Transport robustness (#550, part 1).

One test per defect:
- a bad stdout line (undecodable bytes, JSON that is not an object) must be
  skipped, not end the reader thread;
- any reader exit (EOF or a reader exception) must fail every waiter with the
  typed ``TransportReaderFailedError`` and fail later sends immediately;
- a callback result with a non-finite number must answer the gateway with a
  typed error, and no line with a ``NaN``/``Infinity`` token is ever written;
- a callback request with no registered handler must be answered at once with
  a typed error instead of leaving the gateway waiting.
"""

from __future__ import annotations

import asyncio
import contextlib
import gc
import json
import os
import queue
import signal
import subprocess
import sys
import textwrap
import threading
import time
import weakref
from unittest.mock import AsyncMock, MagicMock

import pytest

from meerkat_meta import GatewayChildEndedError, TransportError, TransportReaderFailedError
from meerkat_meta._transport import (
    _INIT_SETTLED_METHOD,
    InitWatch,
    PersistentTransport,
    TransportTermination,
)


def _reader_transport(lines: list[bytes]) -> PersistentTransport:
    transport = PersistentTransport("unused")
    process = MagicMock()
    process.stdout.readline.side_effect = lines
    transport._process = process
    return transport


def test_reader_skips_undecodable_and_non_object_lines():
    transport = _reader_transport([
        b"\xff\xfe not utf-8\n",
        b"5\n",
        b"[1, 2]\n",
        b"null\n",
        b'{"jsonrpc":"2.0","id":"r1","result":{"ok":true}}\n',
        b"",
    ])
    event = threading.Event()
    transport._pending["r1"] = event

    transport._reader_loop()

    assert event.is_set()
    assert transport._results["r1"] == {"jsonrpc": "2.0", "id": "r1", "result": {"ok": True}}


class _GatedStdout:
    """stdout whose first readline returns only after the request is written."""

    def __init__(self, ending: object):
        self.written = threading.Event()
        self._ending = ending

    def readline(self) -> bytes:
        self.written.wait(timeout=5)
        if isinstance(self._ending, BaseException):
            raise self._ending
        return self._ending  # type: ignore[return-value]


def _live_transport(ending: object) -> tuple[PersistentTransport, _GatedStdout]:
    transport = PersistentTransport("unused", timeout=30.0)
    stdout = _GatedStdout(ending)
    process = MagicMock()
    process.poll.return_value = None
    process.stdout = stdout
    process.stdin.write.side_effect = lambda _data: stdout.written.set()
    transport._process = process
    transport._ensure_running = lambda: None
    transport._reader_thread = threading.Thread(target=transport._reader_loop, daemon=True)
    transport._reader_thread.start()
    return transport, stdout


@pytest.mark.parametrize(
    ("ending", "reason"),
    [
        (b"", "closed its stdout"),
        (OSError("pipe read failed"), "pipe read failed"),
    ],
)
def test_reader_exit_fails_waiters_with_typed_error(ending, reason):
    transport, _stdout = _live_transport(ending)

    with pytest.raises(TransportReaderFailedError, match=reason) as raised:
        transport.send_sync({"jsonrpc": "2.0", "id": "w1", "method": "meta/status"})

    assert isinstance(raised.value, TransportError)
    assert reason in raised.value.reason
    transport._reader_thread.join(timeout=5)
    # The reader is gone: a later send fails at once, it does not wait out
    # its timeout for a response nobody will read.
    with pytest.raises(TransportReaderFailedError):
        transport.send_sync({"jsonrpc": "2.0", "id": "w2", "method": "meta/status"}, timeout=30.0)


def test_reader_exit_keeps_a_response_that_arrived_before_eof():
    # A fail-closed init writes its typed error and exits: the waiter must get
    # that error, not the reader failure.
    transport = _reader_transport([
        b'{"jsonrpc":"2.0","id":"init","error":{"code":-32014,"message":"refused"}}\n',
        b"",
    ])
    event = threading.Event()
    transport._pending["init"] = event

    transport._reader_loop()

    assert transport._results["init"]["error"]["code"] == -32014


def _run_callback(transport: PersistentTransport, result: object) -> MagicMock:
    import asyncio

    loop = asyncio.new_event_loop()
    thread = threading.Thread(target=loop.run_forever, daemon=True)
    thread.start()
    try:
        transport._loop = loop
        transport._callback_handler = AsyncMock(return_value=result)
        transport._write_line = MagicMock()
        transport._dispatch_callback(
            {"jsonrpc": "2.0", "id": "cb-7", "method": "callback/roster_provider/roster", "params": {}}
        )
        return transport._write_line
    finally:
        loop.call_soon_threadsafe(loop.stop)
        thread.join(timeout=5)
        loop.close()


@pytest.mark.parametrize("value", [float("nan"), float("inf"), float("-inf")])
def test_non_finite_callback_result_answers_a_typed_error(value):
    write = _run_callback(PersistentTransport("unused"), {"roster": [{"weight": value}]})

    response = write.call_args.args[0]
    assert response["id"] == "cb-7"
    assert "result" not in response
    assert response["error"]["data"] == {"kind": "non_finite_result"}
    assert "NaN or Infinity" in response["error"]["message"]


def test_write_line_never_emits_non_finite_tokens():
    transport = PersistentTransport("unused")
    process = MagicMock()
    transport._process = process

    with pytest.raises(ValueError):
        transport._write_line({"jsonrpc": "2.0", "id": "x", "result": float("nan")})

    process.stdin.write.assert_not_called()


def test_callback_without_handler_is_answered_at_once():
    transport = PersistentTransport("unused")
    transport._write_line = MagicMock()

    transport._handle_callback(
        {"jsonrpc": "2.0", "id": "cb-3", "method": "callback/build_agent", "params": {}}
    )

    response = transport._write_line.call_args.args[0]
    assert response["id"] == "cb-3"
    assert response["error"]["data"] == {
        "kind": "callback_handler_unavailable",
        "method": "callback/build_agent",
    }


def test_notification_without_handler_writes_nothing():
    transport = PersistentTransport("unused")
    transport._write_line = MagicMock()

    transport._handle_callback({"jsonrpc": "2.0", "method": "notification/x", "params": {}})

    transport._write_line.assert_not_called()


def test_written_callback_response_is_strict_json():
    transport = PersistentTransport("unused")
    process = MagicMock()
    transport._process = process

    transport._write_line({"jsonrpc": "2.0", "id": "cb-1", "result": {"n": 1.5}})

    line = process.stdin.write.call_args.args[0].decode("utf-8")
    assert json.loads(line, parse_constant=lambda token: pytest.fail(f"non-JSON token {token}"))


def test_callback_response_is_written_before_queued_requests():
    """A callback response waits only for the line being written, never
    behind requests queued for the pipe (#550 head-of-line)."""
    transport = PersistentTransport("unused")
    first_write_started = threading.Event()
    release_first_write = threading.Event()
    written: list[str] = []

    def write(data: bytes) -> None:
        line = json.loads(data.decode("utf-8"))
        written.append(line["id"])
        if line["id"] == "req-1":
            first_write_started.set()
            assert release_first_write.wait(timeout=5)

    process = MagicMock()
    process.stdin.write.side_effect = write
    transport._process = process

    first = threading.Thread(
        target=transport._write_line, args=({"jsonrpc": "2.0", "id": "req-1", "method": "m"},)
    )
    first.start()
    assert first_write_started.wait(timeout=5)
    # A second request queues behind the blocked write, then a callback
    # response arrives.
    second = threading.Thread(
        target=transport._write_line, args=({"jsonrpc": "2.0", "id": "req-2", "method": "m"},)
    )
    second.start()
    while not transport._write_cond._waiters:  # the second request is waiting its turn
        threading.Event().wait(0.001)
    response = threading.Thread(
        target=transport._write_line,
        args=({"jsonrpc": "2.0", "id": "cb-1", "result": {}},),
        kwargs={"callback_response": True},
    )
    response.start()
    while transport._priority_writers_waiting == 0:
        threading.Event().wait(0.001)
    release_first_write.set()
    for thread in (first, second, response):
        thread.join(timeout=5)

    assert written == ["req-1", "cb-1", "req-2"]


def _spawner(children):
    """A ``subprocess.Popen`` stand-in returning the next fake child.

    The real call passes ``env=``, the whole process environment. The fake
    drops its arguments before anything can raise, so a failing test never
    prints that environment in its traceback.
    """

    def popen(*args, **kwargs):
        del args, kwargs
        return next(children)

    return popen


def _callback_process() -> tuple[MagicMock, queue.Queue[bytes], list[dict]]:
    """A child whose actual transport reader and writer exchange JSON lines."""
    lines: queue.Queue[bytes] = queue.Queue()
    writes: list[dict] = []
    process = MagicMock()
    process.poll.return_value = None
    process.stdout.readline.side_effect = lines.get
    process.stdin.write.side_effect = lambda data: writes.append(json.loads(data))

    def reap(*, timeout):
        process.poll.return_value = 0
        lines.put(b"")
        return 0

    process.wait.side_effect = reap
    return process, lines, writes


@pytest.mark.asyncio
@pytest.mark.parametrize("old_outcome", ["result", "error", "cancelled"])
@pytest.mark.parametrize("queued_reply", [False, True])
async def test_callback_completion_cannot_answer_replacement_child(monkeypatch, old_outcome, queued_reply):
    transport = PersistentTransport("unused")
    old, old_lines, old_writes = _callback_process()
    current, current_lines, current_writes = _callback_process()
    children = iter([old, current])
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", _spawner(children))
    entered = {name: asyncio.Event() for name in ("original", "old", "current")}
    release = {name: asyncio.Event() for name in ("old", "current")}
    finished = {name: threading.Event() for name in entered}

    async def handler(_method, params):
        name = params["invocation"]
        entered[name].set()
        if name in release:
            await release[name].wait()
        if name == "old" and old_outcome == "error":
            raise ValueError("old invocation failed")
        if name == "old" and old_outcome == "cancelled":
            raise asyncio.CancelledError()
        return {"invocation": name}

    dispatch = transport._dispatch_callback

    def observe_dispatch(msg, *args, **kwargs):
        try:
            dispatch(msg, *args, **kwargs)
        finally:
            finished[msg["params"]["invocation"]].set()

    monkeypatch.setattr(transport, "_dispatch_callback", observe_dispatch)
    transport.set_callback_handler(handler)

    def send(lines, name, callback_id="cb-1"):
        lines.put(json.dumps({
            "jsonrpc": "2.0", "id": callback_id, "method": "callback/call_tool",
            "params": {"invocation": name},
        }).encode() + b"\n")

    readers = []
    blocker = None
    release_writer = threading.Event()
    try:
        transport.start()
        readers.append(transport._reader_thread)
        send(old_lines, "original", "cb-0")
        assert await asyncio.to_thread(finished["original"].wait, 2)
        assert old_writes == [{
            "jsonrpc": "2.0", "id": "cb-0", "result": {"invocation": "original"},
        }]

        send(old_lines, "old")
        await asyncio.wait_for(entered["old"].wait(), 2)
        if queued_reply:
            writer_started = threading.Event()

            def blocked_write(data):
                old_writes.append(json.loads(data))
                writer_started.set()
                assert release_writer.wait(2)

            old.stdin.write.side_effect = blocked_write
            blocker = threading.Thread(target=transport._write_line, args=({
                "jsonrpc": "2.0", "id": "old-request", "method": "meta/status",
            },))
            blocker.start()
            assert await asyncio.to_thread(writer_started.wait, 2)
            release["old"].set()
            deadline = time.monotonic() + 2
            while transport._priority_writers_waiting == 0:
                assert time.monotonic() < deadline
                await asyncio.sleep(0.001)
        transport.stop()
        await asyncio.to_thread(readers[0].join, 2)
        transport.start()
        readers.append(transport._reader_thread)
        send(current_lines, "current")
        await asyncio.wait_for(entered["current"].wait(), 2)

        # The replacement owns this reused callback id. Let the old handler
        # finish completely before its own handler is allowed to answer.
        release["old"].set()
        release_writer.set()
        assert await asyncio.to_thread(finished["old"].wait, 2)
        assert current_writes == [], "old completion answered the replacement's callback"
        assert len(old_writes) == 1 + int(queued_reply), "retired child received a late callback reply"

        release["current"].set()
        assert await asyncio.to_thread(finished["current"].wait, 2)
        assert current_writes == [{
            "jsonrpc": "2.0", "id": "cb-1", "result": {"invocation": "current"},
        }]
    finally:
        release_writer.set()
        for gate in release.values():
            gate.set()
        for name in entered:
            if entered[name].is_set():
                await asyncio.to_thread(finished[name].wait, 2)
        transport.stop()
        for reader in readers:
            await asyncio.to_thread(reader.join, 2)
        if blocker is not None:
            await asyncio.to_thread(blocker.join, 2)


@pytest.mark.asyncio
async def test_retired_reader_exit_cannot_fail_replacement_request(monkeypatch):
    transport = PersistentTransport("unused", timeout=2)
    old, old_lines, _old_writes = _callback_process()
    current, current_lines, current_writes = _callback_process()
    # Reaping the old process need not coincide with its reader unwinding.
    def reap_old(*, timeout):
        old.poll.return_value = 0
        return 0

    old.wait.side_effect = reap_old
    children = iter([old, current])
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", _spawner(children))
    request_written = threading.Event()

    def current_write(data):
        current_writes.append(json.loads(data))
        request_written.set()

    current.stdin.write.side_effect = current_write
    readers = []
    pending = None
    try:
        transport.start()
        readers.append(transport._reader_thread)
        transport.stop()
        transport.start()
        readers.append(transport._reader_thread)
        pending = asyncio.create_task(transport.send_async({
            "jsonrpc": "2.0", "id": "current-request", "method": "meta/status",
        }))
        assert await asyncio.to_thread(request_written.wait, 2)

        old_lines.put(b"")
        await asyncio.to_thread(readers[0].join, 2)
        assert not readers[0].is_alive()
        assert transport._reader_failure is None, "retired reader failed the replacement"
        current_lines.put(b'{"jsonrpc":"2.0","id":"current-request","result":{"current":true}}\n')
        assert await pending == {
            "jsonrpc": "2.0", "id": "current-request", "result": {"current": True},
        }
        assert transport._pending == {}
    finally:
        old_lines.put(b"")
        transport.stop()
        if pending is not None:
            await asyncio.gather(pending, return_exceptions=True)
        for reader in readers:
            await asyncio.to_thread(reader.join, 2)


@pytest.mark.asyncio
async def test_retired_stop_cannot_close_replacement_stderr(monkeypatch):
    transport = PersistentTransport("unused")
    old, old_lines, _old_writes = _callback_process()
    current, _current_lines, _current_writes = _callback_process()
    old_stderr, current_stderr = MagicMock(), MagicMock()
    reaping = threading.Event()
    release = threading.Event()

    def reap_old(*, timeout):
        old.poll.return_value = 0
        reaping.set()
        assert release.wait(2)
        old_lines.put(b"")
        return 0

    old.wait.side_effect = reap_old
    children = iter([old, current])
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", _spawner(children))
    transport.start()
    old_reader = transport._reader_thread
    transport._stderr_file = old_stderr
    stopping = asyncio.create_task(asyncio.to_thread(transport.stop))
    current_reader = None
    try:
        assert await asyncio.to_thread(reaping.wait, 2)
        transport.start()
        current_reader = transport._reader_thread
        transport._stderr_file = current_stderr
        release.set()
        await stopping
        assert transport._process is current
        assert transport._stderr_file is current_stderr
        current_stderr.close.assert_not_called()
        old_stderr.close.assert_called_once_with()
    finally:
        release.set()
        await stopping
        transport.stop()
        await asyncio.to_thread(old_reader.join, 2)
        if current_reader is not None:
            await asyncio.to_thread(current_reader.join, 2)


@pytest.mark.asyncio
async def test_callback_queued_on_loop_cannot_invoke_after_reconnect(monkeypatch):
    transport = PersistentTransport("unused")
    old, old_lines, old_writes = _callback_process()
    current, current_lines, current_writes = _callback_process()
    children = iter([old, current])
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", _spawner(children))
    scheduled, release = asyncio.Event(), asyncio.Event()
    finished = {name: threading.Event() for name in ("old", "current")}
    effects = []
    schedule = asyncio.run_coroutine_threadsafe

    def delayed_schedule(coroutine, loop):
        async def before_invocation():
            scheduled.set()
            await release.wait()
            return await coroutine

        return schedule(before_invocation(), loop)

    monkeypatch.setattr("meerkat_meta._transport.asyncio.run_coroutine_threadsafe", delayed_schedule)

    async def handler(_method, params):
        effects.append(params["invocation"])
        return {"invocation": params["invocation"]}

    dispatch = transport._dispatch_callback

    def observe_dispatch(msg, *args, **kwargs):
        try:
            dispatch(msg, *args, **kwargs)
        finally:
            finished[msg["params"]["invocation"]].set()

    monkeypatch.setattr(transport, "_dispatch_callback", observe_dispatch)
    transport.set_callback_handler(handler)
    readers = []
    try:
        transport.start()
        readers.append(transport._reader_thread)
        old_lines.put(b'{"jsonrpc":"2.0","id":"cb-1","method":"callback/call_tool","params":{"invocation":"old"}}\n')
        await asyncio.wait_for(scheduled.wait(), 2)
        transport.stop()
        await asyncio.to_thread(readers[0].join, 2)
        transport.start()
        readers.append(transport._reader_thread)
        release.set()
        assert await asyncio.to_thread(finished["old"].wait, 2)
        assert effects == [], "queued old callback invoked its handler after reconnect"
        assert old_writes == []
        assert current_writes == []

        current_lines.put(b'{"jsonrpc":"2.0","id":"cb-1","method":"callback/call_tool","params":{"invocation":"current"}}\n')
        assert await asyncio.to_thread(finished["current"].wait, 2)
        assert effects == ["current"]
        assert current_writes == [{
            "jsonrpc": "2.0", "id": "cb-1", "result": {"invocation": "current"},
        }]
    finally:
        release.set()
        await asyncio.to_thread(finished["old"].wait, 2)
        transport.stop()
        for reader in readers:
            await asyncio.to_thread(reader.join, 2)


@pytest.mark.asyncio
@pytest.mark.parametrize("retirement", ["exited", "stopped"])
@pytest.mark.parametrize("received_reply", [None, "result", "error"])
async def test_restart_detaches_old_requests_before_reusing_ids(monkeypatch, retirement, received_reply):
    transport = PersistentTransport("unused", timeout=2)
    old, old_lines, _old_writes = _callback_process()
    current, current_lines, current_writes = _callback_process()
    children = iter([old, current])
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", _spawner(children))
    old_written, release_old, current_written = threading.Event(), threading.Event(), threading.Event()
    write = transport._write_line

    def delayed_return(obj, **kwargs):
        write(obj, **kwargs)
        if obj.get("params", {}).get("owner") == "old":
            old_written.set()
            assert release_old.wait(2)
        else:
            current_written.set()

    monkeypatch.setattr(transport, "_write_line", delayed_return)
    readers = []
    requests = []
    old_reply = None
    try:
        transport.start()
        readers.append(transport._reader_thread)
        old_request = asyncio.create_task(transport.send_async({
            "jsonrpc": "2.0", "id": "reused-id", "method": "meta/init", "params": {"owner": "old"},
        }))
        requests.append(old_request)
        assert await asyncio.to_thread(old_written.wait, 2)
        old_event = transport._pending["reused-id"]
        if received_reply is not None:
            old_reply = {"jsonrpc": "2.0", "id": "reused-id", received_reply: {
                "old": True, "stdio_shutdown_handshake": True, "stdio_shutdown_horizon_ms": 123_000,
            }}
            old_lines.put(json.dumps(old_reply).encode() + b"\n")
            assert await asyncio.to_thread(old_event.wait, 2)
        if retirement == "stopped":
            transport.stop()
        else:
            old.poll.return_value = 0
        transport.start()
        readers.append(transport._reader_thread)
        old_lines.put(b"")
        await asyncio.to_thread(readers[0].join, 2)
        assert not readers[0].is_alive()
        assert old_event.is_set(), "replacement left the retired request waiting for its timeout"

        current_request = asyncio.create_task(transport.send_async({
            "jsonrpc": "2.0", "id": "reused-id", "method": "meta/status", "params": {"owner": "current"},
        }))
        requests.append(current_request)
        assert await asyncio.to_thread(current_written.wait, 0.5), "old request still owns the replacement request id"
        current_event = transport._pending["reused-id"]
        assert current_event is not old_event

        # Complete old cleanup only after the new child owns the reused ID.
        release_old.set()
        if old_reply is None:
            with pytest.raises(TransportReaderFailedError):
                await old_request
        else:
            assert await old_request == old_reply
        assert transport._supports_shutdown_handshake is False, "old init changed the replacement's capabilities"
        assert transport._pending["reused-id"] is current_event
        assert not current_event.is_set()
        assert len(current_writes) == 1
        current_reply = {"jsonrpc": "2.0", "id": "reused-id", "result": {"current": True}}
        current_lines.put(json.dumps(current_reply).encode() + b"\n")
        assert await current_request == current_reply
        assert transport._pending == {}
        assert transport._results == {}
    finally:
        release_old.set()
        old_lines.put(b"")
        transport._supports_shutdown_handshake = False
        transport.stop()
        await asyncio.gather(*requests, return_exceptions=True)
        for reader in readers:
            await asyncio.to_thread(reader.join, 2)


@pytest.mark.asyncio
async def test_restart_fails_only_old_init_watch(monkeypatch):
    transport = PersistentTransport("unused")
    old, old_lines, _old_writes = _callback_process()
    current, current_lines, _current_writes = _callback_process()
    children = iter([old, current])
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", _spawner(children))
    readers = []
    try:
        transport.start()
        readers.append(transport._reader_thread)
        old_watch = transport.open_init_watch("reused-init")
        old.poll.return_value = 0
        transport.start()
        readers.append(transport._reader_thread)
        current_watch = transport.open_init_watch("reused-init")
        old_lines.put(b"")
        await asyncio.to_thread(readers[0].join, 2)
        with pytest.raises(TransportReaderFailedError):
            await old_watch.settlement(deadline=0.25)
        transport.close_init_watch(old_watch)
        assert transport._init_watch is current_watch
        settled = {"init_id": "reused-init", "status": "settled"}
        current_lines.put(json.dumps({
            "jsonrpc": "2.0", "method": "meta/init_settled", "params": settled,
        }).encode() + b"\n")
        assert await current_watch.settlement(deadline=1) == settled
    finally:
        old_lines.put(b"")
        transport.stop()
        for reader in readers:
            await asyncio.to_thread(reader.join, 2)


@pytest.mark.asyncio
async def test_first_start_keeps_preinstalled_init_watch(monkeypatch):
    transport = PersistentTransport("unused")
    child, lines, _writes = _callback_process()
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", _spawner(iter([child])))
    watch = transport.open_init_watch("first-init")
    settled = {"init_id": "first-init", "status": "settled"}

    def answer_init(data):
        request = json.loads(data)
        lines.put(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": {"accepted": True}}).encode() + b"\n")
        lines.put(json.dumps({"jsonrpc": "2.0", "method": "meta/init_settled", "params": settled}).encode() + b"\n")

    child.stdin.write.side_effect = answer_init
    try:
        await transport.send_async({"jsonrpc": "2.0", "id": "first-init", "method": "meta/init"})
        assert await watch.settlement(deadline=1) == settled
        assert transport._init_watch is watch
    finally:
        transport.stop()
        await asyncio.to_thread(transport._reader_thread.join, 2)


@contextlib.contextmanager
def _loop_exception_reports():
    """Record what asyncio reports to the running loop's exception handler.

    A future whose exception nobody retrieved is reported there when it is
    collected ("Future exception was never retrieved"); Python 3.14 also
    reports a failure that lands in a future after its shield was cancelled
    ("... exception in shielded future"). Only the message and the
    exception's repr are kept, so a report never keeps its future alive.
    """
    loop = asyncio.get_running_loop()
    previous = loop.get_exception_handler()
    reports: list[tuple[object, str]] = []
    loop.set_exception_handler(
        lambda _loop, context: reports.append(
            (context.get("message"), repr(context.get("exception")))
        )
    )
    try:
        yield reports
    finally:
        loop.set_exception_handler(previous)


async def _assert_settlements_retired(
    settlements: list[weakref.ref], reports: list[tuple[object, str]]
) -> None:
    """Run what is queued on the loop, collect garbage, then check that no
    settlement future was reported and that each one was really collected
    (otherwise the empty report list would prove nothing)."""
    for _ in range(5):
        await asyncio.sleep(0)
    gc.collect()
    await asyncio.sleep(0)
    assert reports == []
    assert settlements, "no init settlement future was observed"
    assert all(ref() is None for ref in settlements), (
        "a settlement future is still referenced"
    )


def _fail_as_the_reader(watch: InitWatch) -> None:
    """Fail ``watch`` as the reader thread does at EOF: from another thread,
    with a delivery queued on the loop. The loop is blocked meanwhile, so the
    delivery is still queued when this returns."""
    reader = threading.Thread(
        target=watch._fail,
        args=(TransportReaderFailedError("the gateway closed its stdout"),),
    )
    reader.start()
    reader.join(5)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "order",
    [
        "delivered_before_close",
        "queued_behind_close",
        "delivered_after_a_timed_out_wait",
        "delivered_after_a_cancelled_wait",
        "closed_off_loop",
        "closed_off_loop_behind_a_queued_delivery",
    ],
)
async def test_closing_an_init_watch_retires_its_settlement(order):
    """#251: at EOF the reader fails the init watch with a delivery queued on
    the host loop. Whatever the order of that delivery, the caller's wait and
    the close, closing the watch retires its settlement future on that loop,
    so asyncio never reports the failure as unretrieved."""
    transport = PersistentTransport("unused")
    with _loop_exception_reports() as reports:
        watch = transport.open_init_watch("init-retired")
        settlements = [weakref.ref(watch._settlement)]
        if order == "delivered_after_a_timed_out_wait":
            with pytest.raises(asyncio.TimeoutError):
                await watch.settlement(deadline=0.01)
        elif order == "delivered_after_a_cancelled_wait":
            waiter = asyncio.ensure_future(watch.settlement())
            await asyncio.sleep(0)
            waiter.cancel()
            with pytest.raises(asyncio.CancelledError):
                await waiter
            del waiter
        _fail_as_the_reader(watch)
        if order not in (
            "queued_behind_close",
            "closed_off_loop_behind_a_queued_delivery",
        ):
            await asyncio.sleep(0)
            assert watch._settlement.done(), "the queued delivery did not run"
        if order.startswith("closed_off_loop"):
            closer = threading.Thread(target=transport.close_init_watch, args=(watch,))
            closer.start()
            closer.join(5)
        else:
            transport.close_init_watch(watch)
        assert transport._init_watch is None
        del watch
        await _assert_settlements_retired(settlements, reports)


@pytest.mark.asyncio
async def test_a_timed_out_settlement_wait_raises_asyncio_timeout_and_keeps_the_settlement():
    transport = PersistentTransport("unused")
    with _loop_exception_reports() as reports:
        watch = transport.open_init_watch("init-deadline")
        settlements = [weakref.ref(watch._settlement)]
        with pytest.raises(asyncio.TimeoutError) as raised:
            await watch.settlement(deadline=0.01)
        assert type(raised.value) is asyncio.TimeoutError
        del raised
        assert not watch._settlement.done(), "the deadline ended the settlement itself"
        settled = {"init_id": "init-deadline", "outcome": "ready"}
        watch._deliver(_INIT_SETTLED_METHOD, settled)
        assert await watch.settlement(deadline=1) == settled
        transport.close_init_watch(watch)
        del watch
        await _assert_settlements_retired(settlements, reports)


@pytest.mark.asyncio
@pytest.mark.parametrize("deadline", [None, 30.0])
async def test_a_cancelled_settlement_waiter_leaves_the_settlement_pending(deadline):
    transport = PersistentTransport("unused")
    with _loop_exception_reports() as reports:
        watch = transport.open_init_watch("init-cancelled")
        settlements = [weakref.ref(watch._settlement)]
        waiter = asyncio.ensure_future(watch.settlement(deadline))
        await asyncio.sleep(0)
        waiter.cancel()
        with pytest.raises(asyncio.CancelledError):
            await waiter
        del waiter
        assert not watch._settlement.done(), (
            "cancelling the caller ended the settlement itself"
        )
        settled = {"init_id": "init-cancelled", "outcome": "ready"}
        watch._deliver(_INIT_SETTLED_METHOD, settled)
        assert await watch.settlement(deadline=1) == settled
        transport.close_init_watch(watch)
        del watch
        await _assert_settlements_retired(settlements, reports)


@pytest.mark.asyncio
async def test_a_stop_during_a_replacement_start_publishes_no_child(monkeypatch):
    """A stop recorded while a replacement is being created wins: the start
    abandons and reaps its child, closes that child's stderr, and publishes
    nothing; the stop retires the old child."""
    transport = PersistentTransport("unused", env={"META_GATEWAY_STDERR_FILE": "unused.stderr"})
    old, old_lines, _old_writes = _callback_process()
    current, _current_lines, _current_writes = _callback_process()
    old_stderr, current_stderr = MagicMock(), MagicMock()
    children = iter([old, current])
    stderr_files = iter([old_stderr, current_stderr])
    preparing, publish = threading.Event(), threading.Event()
    monkeypatch.setattr("builtins.open", lambda *args, **kwargs: next(stderr_files))

    spawned_stderr = []
    published = []

    def spawn(*args, **kwargs):
        # Keep only what the test checks: the env holds the whole process
        # environment and must never reach a traceback.
        spawned_stderr.append(kwargs.get("stderr"))
        del args, kwargs
        child = next(children)
        if child is current:
            preparing.set()
            published.append(publish.wait(2))
        return child

    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", spawn)
    transport.start()
    old_reader = transport._reader_thread
    old.poll.return_value = 0
    starting = asyncio.create_task(asyncio.to_thread(transport.start))
    stopping = None
    try:
        assert await asyncio.to_thread(preparing.wait, 2)
        stopping = asyncio.create_task(asyncio.to_thread(transport.stop))
        assert await asyncio.to_thread(_wait_until, lambda: transport._closed), (
            "stop did not record its intent"
        )
        publish.set()
        with pytest.raises(TransportError):
            await starting
        await stopping
        assert spawned_stderr == [old_stderr, current_stderr]
        assert published == [True]
        assert transport._process is None, "a start published a child after stop"
        assert not transport.is_running()
        assert current.poll() is not None, "the abandoned child was left running"
        current_stderr.close.assert_called_once_with()
        old_stderr.close.assert_called_once_with()
    finally:
        publish.set()
        await asyncio.gather(starting, *(t for t in [stopping] if t), return_exceptions=True)
        transport.stop()
        old_lines.put(b"")
        await asyncio.to_thread(old_reader.join, 2)

# --- Child lifetime: no implicit replacement after a child existed (#111) ---
#
# A request must never start an uninitialized replacement for a child that
# already existed. The first send of a never-started, open transport still
# starts it. Explicit start() remains the caller's replacement decision.


class _CountingSpawner:
    """Popen stand-in that counts spawns and drops its arguments first."""

    def __init__(self, *children, delay: float = 0.0):
        self._children = iter(children)
        self._delay = delay
        self._lock = threading.Lock()
        self.count = 0

    def __call__(self, *args, **kwargs):
        del args, kwargs
        with self._lock:
            self.count += 1
        if self._delay:
            time.sleep(self._delay)
        child = next(self._children, None)
        if child is None:
            raise AssertionError("an unexpected gateway child was started")
        return child


class _HeldSpawner:
    """Popen stand-in that acknowledges entry and waits to be released."""

    def __init__(self, child):
        self._child = child
        self.entered = threading.Event()
        self.release = threading.Event()
        self.count = 0

    def __call__(self, *args, **kwargs):
        del args, kwargs
        self.count += 1
        if self.count > 1:
            raise AssertionError("an unexpected gateway child was started")
        self.entered.set()
        assert self.release.wait(5)
        return self._child


def _acknowledge_claims(monkeypatch, transport):
    """Count arrivals at the start claim, so a test can wait for a caller."""
    claim = transport._claim_start
    arrivals = []
    arrived = threading.Condition()

    def counted_claim(*args, **kwargs):
        with arrived:
            arrivals.append(threading.current_thread().name)
            arrived.notify_all()
        return claim(*args, **kwargs)

    def wait_for(count, timeout=2.0):
        with arrived:
            return arrived.wait_for(lambda: len(arrivals) >= count, timeout)

    monkeypatch.setattr(transport, "_claim_start", counted_claim)
    return wait_for


def _wait_until(predicate, timeout=2.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.01)
    return predicate()


def _answering_process():
    """A child that answers every request with its own id."""
    child, lines, writes = _callback_process()

    def answer(data):
        request = json.loads(data)
        writes.append(request)
        if "id" in request and "method" in request:
            lines.put(json.dumps(
                {"jsonrpc": "2.0", "id": request["id"], "result": {"ok": True}}
            ).encode() + b"\n")

    child.stdin.write.side_effect = answer
    return child, lines, writes


def _end_child(child, lines, transport, *, exit_code):
    """The child exits (when ``exit_code`` is set) and its stdout closes."""
    reader = transport._reader_thread
    if exit_code is not None:
        child.poll.return_value = exit_code
    lines.put(b"")
    reader.join(2)
    assert not reader.is_alive()


def _send(transport, mode, request_id):
    request = {"jsonrpc": "2.0", "id": request_id, "method": "meta/status"}
    if mode == "sync":
        return transport.send_sync(request)
    return asyncio.run(transport.send_async(request))


@pytest.mark.parametrize("mode", ["sync", "async"])
def test_ended_child_is_not_replaced_by_a_request(monkeypatch, mode):
    child, lines, _ = _callback_process()
    spawner = _CountingSpawner(child)
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", spawner)
    transport = PersistentTransport("unused", timeout=2)
    try:
        transport.start()
        _end_child(child, lines, transport, exit_code=137)

        with pytest.raises(TransportReaderFailedError) as ended:
            _send(transport, mode, "after-exit")
        assert spawner.count == 1, "a request replaced the ended gateway child"
        assert type(ended.value).__name__ == "GatewayChildEndedError"
        assert getattr(ended.value, "exit_code", "missing") == 137
        assert not transport.is_running()
    finally:
        transport.stop()


def test_reader_failure_while_the_child_lives_is_not_an_exit(monkeypatch):
    child, lines, _ = _callback_process()
    spawner = _CountingSpawner(child)
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", spawner)
    transport = PersistentTransport("unused", timeout=2)
    try:
        transport.start()
        _end_child(child, lines, transport, exit_code=None)
        assert child.poll() is None

        with pytest.raises(TransportReaderFailedError) as ended:
            _send(transport, "sync", "after-eof")
        assert spawner.count == 1
        assert getattr(ended.value, "exit_code", "missing") is None
        assert getattr(ended.value, "stop_requested", "missing") is False
        assert not transport.is_running(), "a child with a failed channel is not usable"
        # The live child still owns the transport: no replacement beside it.
        with pytest.raises(TransportError):
            transport.start()
        assert spawner.count == 1
    finally:
        transport.stop()


def test_stop_before_the_first_send_closes_admission(monkeypatch):
    spawner = _CountingSpawner()
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", spawner)
    transport = PersistentTransport("unused", timeout=2)
    transport.stop()

    with pytest.raises(TransportReaderFailedError) as ended:
        _send(transport, "sync", "after-stop")
    assert spawner.count == 0, "a stopped transport started a gateway child"
    assert getattr(ended.value, "stop_requested", "missing") is True


def test_concurrent_first_sends_start_one_child(monkeypatch):
    child, _lines, _ = _answering_process()
    spawner = _HeldSpawner(child)
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", spawner)
    transport = PersistentTransport("unused", timeout=2)
    outcomes = {}

    def send(request_id):
        try:
            outcomes[request_id] = _send(transport, "sync", request_id)
        except Exception as exc:  # noqa: BLE001 - recorded for the assertion
            outcomes[request_id] = exc

    senders = [threading.Thread(target=send, args=(f"first-{n}",)) for n in range(2)]
    arrivals = _acknowledge_claims(monkeypatch, transport)
    try:
        senders[0].start()
        assert spawner.entered.wait(2), "the first send did not start the child"
        senders[1].start()
        assert arrivals(2), "the second send did not reach the held start"
        spawner.release.set()
        for sender in senders:
            sender.join(5)
            assert not sender.is_alive()
        assert spawner.count == 1, "concurrent first sends started two children"
        assert outcomes == {
            request_id: {"jsonrpc": "2.0", "id": request_id, "result": {"ok": True}}
            for request_id in ("first-0", "first-1")
        }
    finally:
        spawner.release.set()
        transport.stop()


def test_a_waiter_keeps_its_own_child_record_across_replacement(monkeypatch):
    old, old_lines, _ = _callback_process()
    current, current_lines, _ = _callback_process()
    monkeypatch.setattr(
        "meerkat_meta._transport.subprocess.Popen", _CountingSpawner(old, current)
    )
    transport = PersistentTransport("unused", timeout=2)
    observed = []
    try:
        transport.start()
        captured = threading.Event()
        selected = transport._lifetime
        select_and_wait = selected.wait

        def acknowledged_wait(timeout=None):
            captured.set()
            return select_and_wait(timeout)

        selected.wait = acknowledged_wait
        waiter = threading.Thread(
            target=lambda: observed.append(transport.wait_child_ended(timeout=2))
        )
        waiter.start()
        assert captured.wait(2), "the waiter did not select the old child"
        old.poll.return_value = 3
        transport.start()
        old_lines.put(b"")
        waiter.join(3)
        assert not waiter.is_alive()
        assert len(observed) == 1 and observed[0] is not None
        assert observed[0].exit_code == 3
        assert transport.child_ended is None, "the replacement inherited the old child's end"
    finally:
        old_lines.put(b"")
        transport.stop()


def test_a_retired_reader_does_not_end_its_successor(monkeypatch):
    old, old_lines, _ = _callback_process()
    current, current_lines, _ = _answering_process()
    monkeypatch.setattr(
        "meerkat_meta._transport.subprocess.Popen", _CountingSpawner(old, current)
    )
    transport = PersistentTransport("unused", timeout=2)
    try:
        transport.start()
        old_reader = transport._reader_thread
        old.poll.return_value = 0
        transport.start()
        old_lines.put(b"")
        old_reader.join(2)
        assert not old_reader.is_alive()

        assert transport.child_ended is None
        assert transport.is_running()
        assert _send(transport, "sync", "on-successor")["result"] == {"ok": True}
    finally:
        transport.stop()


def test_stop_after_a_failure_keeps_the_failure_as_the_cause(monkeypatch):
    child, lines, _ = _callback_process()
    monkeypatch.setattr(
        "meerkat_meta._transport.subprocess.Popen", _CountingSpawner(child)
    )
    transport = PersistentTransport("unused", timeout=2)
    transport.start()
    _end_child(child, lines, transport, exit_code=9)
    transport.stop()

    ended = transport.child_ended
    assert ended is not None
    assert ended.exit_code == 9
    assert ended.stop_requested is True
    assert "stdout" in ended.reason, "stop relabelled the earlier failure"


def test_a_stop_during_a_lazy_first_start_keeps_admission_closed(monkeypatch):
    child, _lines, _ = _answering_process()
    spawner = _HeldSpawner(child)
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", spawner)
    transport = PersistentTransport("unused", timeout=2)
    outcome = {}

    def first_send():
        try:
            outcome["first"] = _send(transport, "sync", "first")
        except Exception as exc:  # noqa: BLE001 - recorded for the assertion
            outcome["first"] = exc

    sender = threading.Thread(target=first_send)
    stopper = threading.Thread(target=transport.stop)
    try:
        sender.start()
        assert spawner.entered.wait(2)
        stopper.start()
        assert _wait_until(lambda: transport._closed), "stop did not record its intent"
        spawner.release.set()
        sender.join(5)
        stopper.join(5)
        assert not sender.is_alive() and not stopper.is_alive()
        assert isinstance(outcome["first"], TransportReaderFailedError), (
            "a stopped transport admitted the first send"
        )
        assert getattr(outcome["first"], "stop_requested", None) is True
        with pytest.raises(TransportReaderFailedError):
            _send(transport, "sync", "after-stop")
        assert spawner.count == 1
        assert not transport.is_running()
        assert transport._process is None, "a completed stop left a published child"
        assert child.poll() is not None, "a completed stop left a live child"
    finally:
        spawner.release.set()
        transport.stop()


def test_a_stop_during_an_explicit_start_leaves_no_live_child(monkeypatch):
    child, _lines, _ = _answering_process()
    spawner = _HeldSpawner(child)
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", spawner)
    transport = PersistentTransport("unused", timeout=2)
    outcome = {}

    def explicit_start():
        try:
            transport.start()
            outcome["start"] = "published"
        except Exception as exc:  # noqa: BLE001 - recorded for the assertion
            outcome["start"] = exc

    starter = threading.Thread(target=explicit_start)
    stopper = threading.Thread(target=transport.stop)
    try:
        starter.start()
        assert spawner.entered.wait(2)
        stopper.start()
        assert _wait_until(lambda: transport._closed), "stop did not record its intent"
        spawner.release.set()
        starter.join(5)
        stopper.join(5)
        assert not starter.is_alive() and not stopper.is_alive()
        assert isinstance(outcome["start"], TransportError), (
            "a start published its child after stop"
        )
        assert transport._process is None
        assert not transport.is_running()
        assert child.poll() is not None, "a completed stop left a live child"
    finally:
        spawner.release.set()
        transport.stop()


def test_concurrent_explicit_starts_start_one_child(monkeypatch):
    child, _lines, _ = _answering_process()
    spawner = _HeldSpawner(child)
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", spawner)
    transport = PersistentTransport("unused", timeout=2)
    errors = []

    def explicit_start():
        try:
            transport.start()
        except Exception as exc:  # noqa: BLE001 - recorded for the assertion
            errors.append(exc)

    starters = [threading.Thread(target=explicit_start) for _ in range(2)]
    arrivals = _acknowledge_claims(monkeypatch, transport)
    try:
        starters[0].start()
        assert spawner.entered.wait(2)
        starters[1].start()
        assert arrivals(2), "the second start did not reach the held start"
        spawner.release.set()
        for starter in starters:
            starter.join(5)
            assert not starter.is_alive()
        assert errors == []
        assert spawner.count == 1, "concurrent starts created two children"
        assert transport.is_running()
    finally:
        spawner.release.set()
        transport.stop()


def test_a_retired_child_is_released(monkeypatch):
    import gc
    import weakref

    old, old_lines, _ = _callback_process()
    current, _current_lines, _ = _callback_process()
    monkeypatch.setattr(
        "meerkat_meta._transport.subprocess.Popen", _CountingSpawner(old, current)
    )
    transport = PersistentTransport("unused", timeout=2)
    try:
        transport.start()
        old_reader = transport._reader_thread
        old.poll.return_value = 0
        transport.start()
        old_lines.put(b"")
        old_reader.join(2)
        assert not old_reader.is_alive()
        retired = weakref.ref(old)
        del old, old_reader
        monkeypatch.undo()
        gc.collect()
        assert retired() is None, "the transport keeps a retired child alive"
        assert len(transport._lifetimes) == 1
    finally:
        transport.stop()


def test_an_unreapable_abandoned_child_stays_owned_until_reaped(monkeypatch):
    """A child abandoned because stop() won, but that cannot be reaped, stays
    in the transport's custody: stop() reports the incomplete cleanup and a
    replacement is refused until a later stop() reaps it."""
    child = MagicMock()
    child.poll.return_value = None
    child.wait.side_effect = subprocess.TimeoutExpired("gateway", 0)
    spawner = _HeldSpawner(child)
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", spawner)
    transport = PersistentTransport("unused", timeout=2)
    outcome = {}

    def record(name, action):
        try:
            action()
            outcome[name] = "returned"
        except Exception as exc:  # noqa: BLE001 - recorded for the assertion
            outcome[name] = exc

    starter = threading.Thread(target=record, args=("start", transport.start))
    stopper = threading.Thread(target=record, args=("stop", transport.stop))
    try:
        starter.start()
        assert spawner.entered.wait(2)
        stopper.start()
        assert _wait_until(lambda: transport._closed), "stop did not record its intent"
        spawner.release.set()
        starter.join(5)
        stopper.join(5)
        assert not starter.is_alive() and not stopper.is_alive()
        assert isinstance(outcome["start"], TransportError)
        assert isinstance(outcome["stop"], RuntimeError), (
            "stop() reported success while the abandoned child still ran"
        )
        with pytest.raises(TransportError):
            transport.start()
        assert spawner.count == 1, "a replacement started beside the unreaped child"
        assert transport._unreaped == [child]
        # Owned and still running after an attempted kill: not an exit.
        assert transport.last_termination == TransportTermination(
            "unknown", None, None, "sigkill", "not_negotiated"
        )

        child.wait.side_effect = None
        child.wait.return_value = 0
        child.poll.return_value = 0
        transport.stop()
        assert transport._unreaped == []
        assert transport.last_termination == TransportTermination(
            "exited", 0, None, "sigkill", "not_negotiated"
        )

        replacement, _replacement_lines, _ = _answering_process()
        monkeypatch.setattr(
            "meerkat_meta._transport.subprocess.Popen", _CountingSpawner(replacement)
        )
        transport.start()
        assert transport.is_running(), "the reaped child still blocked a replacement"
        assert transport.last_termination is None, "a new child inherited a record"
    finally:
        spawner.release.set()
        transport.stop()


def test_a_lazy_send_paused_before_its_claim_cannot_reopen_a_completed_stop(monkeypatch):
    child, _lines, _ = _answering_process()
    spawner = _CountingSpawner(child)
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", spawner)
    transport = PersistentTransport("unused", timeout=2)
    claim = transport._claim_start
    paused, release = threading.Event(), threading.Event()

    def paused_claim(*args, **kwargs):
        paused.set()
        assert release.wait(5)
        return claim(*args, **kwargs)

    monkeypatch.setattr(transport, "_claim_start", paused_claim)
    outcome = {}

    def first_send():
        try:
            outcome["first"] = _send(transport, "sync", "first")
        except Exception as exc:  # noqa: BLE001 - recorded for the assertion
            outcome["first"] = exc

    sender = threading.Thread(target=first_send)
    try:
        sender.start()
        assert paused.wait(2), "the first send did not reach its start claim"
        transport.stop()
        release.set()
        sender.join(5)
        assert not sender.is_alive()
        assert isinstance(outcome["first"], TransportReaderFailedError), (
            "a send admitted before stop() started a child after it"
        )
        assert getattr(outcome["first"], "stop_requested", None) is True
        assert spawner.count == 0
        assert not transport.is_running()
    finally:
        release.set()
        transport.stop()


def test_a_failing_stderr_close_does_not_drop_an_unreapable_child(monkeypatch):
    """Closing the abandoned child's stderr file can fail; custody of the
    unreaped child must already be recorded, so stop() still reports it."""
    child = MagicMock()
    child.poll.return_value = None
    child.wait.side_effect = subprocess.TimeoutExpired("gateway", 0)
    stderr_file = MagicMock()
    stderr_file.close.side_effect = OSError("stderr close failed")
    monkeypatch.setattr("builtins.open", lambda *args, **kwargs: stderr_file)
    spawner = _HeldSpawner(child)
    monkeypatch.setattr("meerkat_meta._transport.subprocess.Popen", spawner)
    transport = PersistentTransport(
        "unused", timeout=2, env={"META_GATEWAY_STDERR_FILE": "unused.stderr"}
    )
    outcome = {}

    def record(name, action):
        try:
            action()
            outcome[name] = "returned"
        except Exception as exc:  # noqa: BLE001 - recorded for the assertion
            outcome[name] = exc

    starter = threading.Thread(target=record, args=("start", transport.start))
    stopper = threading.Thread(target=record, args=("stop", transport.stop))
    try:
        starter.start()
        assert spawner.entered.wait(2)
        stopper.start()
        assert _wait_until(lambda: transport._closed), "stop did not record its intent"
        spawner.release.set()
        starter.join(5)
        stopper.join(5)
        assert not starter.is_alive() and not stopper.is_alive()
        assert isinstance(outcome["start"], TransportError)
        assert isinstance(outcome["stop"], RuntimeError), (
            "a failed stderr close let stop() report success beside a live child"
        )
        assert transport._unreaped == [child]
        assert transport.last_termination == TransportTermination(
            "unknown", None, None, "sigkill", "not_negotiated"
        )
        with pytest.raises(TransportError):
            transport.start()
        assert spawner.count == 1

        child.wait.side_effect = None
        child.wait.return_value = 0
        child.poll.return_value = 0
        transport.stop()
        assert transport._unreaped == []
        assert transport.last_termination == TransportTermination(
            "exited", 0, None, "sigkill", "not_negotiated"
        )
    finally:
        spawner.release.set()


# --- A gateway that exits while a process it started holds its stdout ---


def test_waiters_fail_when_the_gateway_exits_while_a_descendant_holds_stdout(tmp_path):
    """The gateway exits by itself while a process it started holds its
    stdout, so the reader never reaches EOF. The waiting request fails with
    the child's GatewayChildEndedError about a second after the exit instead
    of waiting out its whole timeout."""
    holder_pid = tmp_path / "holder.pid"
    gateway = tmp_path / "gateway"
    gateway.write_text(
        f"#!{sys.executable}\n"
        + textwrap.dedent(
            f"""\
            import json
            import subprocess
            import sys

            for line in sys.stdin:
                if json.loads(line).get("method") != "test/exit":
                    continue
                holder = subprocess.Popen(
                    [sys.executable, "-c", "import time; time.sleep(30)"],
                    stdin=subprocess.DEVNULL,
                    stdout=sys.stdout,
                    stderr=subprocess.DEVNULL,
                )
                with open({str(holder_pid)!r}, "w") as pid_file:
                    pid_file.write(str(holder.pid))
                sys.exit(7)
            """
        )
    )
    gateway.chmod(0o755)
    transport = PersistentTransport(str(gateway), timeout=20.0)
    transport.start()
    try:
        started = time.monotonic()
        with pytest.raises(GatewayChildEndedError) as excinfo:
            transport.send_sync(
                {"jsonrpc": "2.0", "id": "exit-request", "method": "test/exit", "params": {}}
            )
        elapsed = time.monotonic() - started
    finally:
        if holder_pid.exists():
            try:
                os.kill(int(holder_pid.read_text()), signal.SIGKILL)
            except ProcessLookupError:
                pass
        transport.stop()
    # Failed by the exit after its grace, not by the 20 s request timeout.
    assert elapsed < 10, f"the waiter failed {elapsed:.1f} s after the request"
    assert excinfo.value.exit_code == 7
