"""Adopting a gateway child the host started and keeps.

A host that supervises the gateway process hands the SDK that exact child
(``AdoptedGatewayChild``): its stdin and stdout pipes, a non-blocking
``request_retirement`` and a bounded ``wait_process``. These tests drive an
in-process stand-in: a thread plays the gateway child over two real pipes
made as the host makes them, and a fake host retires it on its own event
loop, on its own thread. No test starts a process, and every test fails if
``subprocess.Popen`` or ``os.kill`` is called.

One contract each:
- the builder, and a constructor-given transport, adopt the child and never
  spawn, even with ``gateway()`` set;
- the carrier as the host builds it (``os.pipe2(O_CLOEXEC)`` ends behind
  unbuffered ``os.fdopen(..., buffering=0)`` objects) carries lines larger
  than the pipe buffer both ways, and interrupted, short and partial raw
  reads and writes, a handled signal during a blocked write included, lose
  nothing; Popen-style buffered ends work as they are;
- only ``start()`` hands the child over, once: a constructor-given adopted
  transport starts with ``meta/init``, a second ``start()`` does nothing, and
  one after a stop is refused;
- one exit watcher is the only caller of ``wait_process``: an exit while a
  descendant holds the child's stdout fails a settling init and requests in
  flight promptly;
- init settles ready over the adopted pipes and yields the HTTP paths;
- a callback handler registered before start answers during init;
- sync and async RPC round trips, and request timeouts;
- a stop closes the input, then requests the host's retirement once however
  often it runs, then sees the exit; it waits through the shutdown horizon
  and no longer, a writer stuck on a full pipe cannot hold back the
  retirement, it never kills, and it refuses an event-loop thread;
- ``shutdown()``, a failed ``build()`` and the cleanup it hands its caller
  run that stop off the event loop;
- a pending exit is a typed outcome and the observation is kept;
- a failed start requests the retirement and observes the child the same
  way, and a cancelled start keeps that observation and its uncertainty;
- refusals before the hand-off leave the pipes untouched;
- the SDK never runs or awaits the host's own cleanup, and the reader never
  calls the host (no circular join);
- the native terminal failure, the child's exit and the SDK reader's
  completion stay apart.
"""
from __future__ import annotations

import asyncio
import errno
import gc
import io
import json
import os
import select
import signal
import subprocess
import threading
import time
from typing import Any, Callable

import pytest

import meerkat_meta._adopted_gateway as adopted_module
from meerkat_meta import (
    AdoptedGatewayChild,
    AdoptedGatewayTransport,
    GatewayChildEndedError,
    GatewayChildExit,
    GatewayChildPendingError,
    GatewayStartFailure,
    GatewayStartupCleanupError,
    GatewayTransportConflictError,
    InitOutcomeUnknownError,
    RpcError,
    StorageResolutionError,
    TransportError,
    TransportTermination,
)
from meerkat_meta.builder import Meta
from meerkat_meta.runtime import MetaRuntime

# The longest any single expectation may take.
_WAIT = 5.0
# The host's own event-loop thread.
_HOST_LOOP = "host-loop"


@pytest.fixture(autouse=True)
def refuse_processes(monkeypatch):
    """No test may start, signal or kill a process."""

    def refuse_spawn(*args: Any, **kwargs: Any) -> Any:
        del args, kwargs  # the call carries the whole environment
        raise AssertionError("an adopted gateway must never be spawned")

    def refuse_signal(*args: Any, **kwargs: Any) -> Any:
        raise AssertionError("an adopted gateway must never be signalled")

    monkeypatch.setattr(subprocess, "Popen", refuse_spawn)
    monkeypatch.setattr(os, "kill", refuse_signal)
    if hasattr(os, "killpg"):
        monkeypatch.setattr(os, "killpg", refuse_signal)


def host_pipe() -> tuple[int, int]:
    """A pipe made as the host makes it: close-on-exec from the start."""
    if hasattr(os, "pipe2"):
        return os.pipe2(os.O_CLOEXEC)
    return os.pipe()  # close-on-exec as well on platforms without pipe2


class FakeGatewayChild:
    """A thread that plays the gateway child over two real pipes.

    ``script(child)`` runs on the thread and returns the exit code. Returning
    is the child's exit: both of its pipe ends close, as a process's do.

    The ends handed to the SDK are made as the host makes them: raw,
    unbuffered ``io.FileIO`` endpoints from ``os.fdopen(fd, ..., buffering=0)``.
    ``endpoints="buffered"`` gives Popen-style buffered ones instead.
    """

    def __init__(
        self,
        script: Callable[[FakeGatewayChild], int],
        *,
        endpoints: str = "raw",
    ):
        stdin_read, stdin_write = host_pipe()
        stdout_read, stdout_write = host_pipe()
        # The child's own ends.
        self.stdin = os.fdopen(stdin_read, "rb")
        self.stdout = os.fdopen(stdout_write, "wb")
        # The host's ends, handed to the SDK.
        buffering = 0 if endpoints == "raw" else -1
        self.input = os.fdopen(stdin_write, "wb", buffering=buffering)
        self.output = os.fdopen(stdout_read, "rb", buffering=buffering)
        self.received: list[dict[str, Any]] = []
        self.exit_code: int | None = None
        self.exited = threading.Event()
        # Set by the host's retirement (or by a test standing in for it).
        self.retire = threading.Event()
        self.saw_end_of_input = threading.Event()
        self._script = script
        self._thread = threading.Thread(target=self._run, daemon=True, name="fake-gateway")

    def start(self) -> FakeGatewayChild:
        self._thread.start()
        return self

    def _run(self) -> None:
        code = 70
        try:
            code = self._script(self)
        finally:
            for end in (self.stdout, self.stdin):
                try:
                    end.close()
                except OSError:
                    pass
            self.exit_code = code
            self.exited.set()

    def read(self) -> dict[str, Any] | None:
        """The next line the SDK wrote, or ``None`` at the end of input."""
        line = self.stdin.readline()
        if not line:
            self.saw_end_of_input.set()
            return None
        message = json.loads(line)
        self.received.append(message)
        return message

    def emit(self, message: dict[str, Any]) -> None:
        self.stdout.write((json.dumps(message) + "\n").encode("utf-8"))
        self.stdout.flush()

    def methods(self) -> list[Any]:
        return [message.get("method") for message in self.received]

    def input_untouched(self) -> bool:
        """Nothing was written to the child's stdin and it was not closed."""
        readable, _, _ = select.select([self.stdin], [], [], 0)
        return not readable and not self.input.closed and not self.output.closed


class FakeHost:
    """The host's side: it keeps the child and retires it on its own loop.

    Its retirement is the host's own cleanup. The SDK may only request it;
    the cleanup runs on the host's loop thread, and a test can hold it open
    (``cleanup_gate``) or make it join the SDK's reader first.
    """

    def __init__(self, child: FakeGatewayChild, *, retire_ends_child: bool = True):
        self.child = child
        self.retire_ends_child = retire_ends_child
        self.loop = asyncio.new_event_loop()
        self._loop_thread = threading.Thread(
            target=self.loop.run_forever, daemon=True, name=_HOST_LOOP
        )
        self._loop_thread.start()
        self._tasks: list[asyncio.Task[None]] = []
        self.retirement_threads: list[str] = []
        self.wait_calls: list[tuple[float, str, bool]] = []
        self.cleanup_threads: list[str] = []
        self.cleanup_done = threading.Event()
        self.cleanup_gate: threading.Event | None = None
        self.cleanup_joins_reader_of: AdoptedGatewayTransport | None = None
        self.retirement_error: Exception | None = None
        self._waits_lock = threading.Lock()
        self._active_waits = 0
        self.most_concurrent_waits = 0
        # What wait_process raises while pending, and whether it answers
        # "pending" at once instead of waiting out its timeout.
        self.pending_error: Callable[[], BaseException] = TimeoutError
        self.answers_pending_at_once = False
        # A failure the next wait_process call raises, once.
        self.wait_error: Exception | None = None
        # Every call into the host, in order, with what the child saw.
        self.events: list[str] = []

    def carrier(self) -> AdoptedGatewayChild:
        return AdoptedGatewayChild(
            input=self.child.input,
            output=self.child.output,
            request_retirement=self.request_retirement,
            wait_process=self.wait_process,
        )

    def request_retirement(self) -> None:
        self.retirement_threads.append(threading.current_thread().name)
        self.events.append("retirement requested")
        if self.retirement_error is not None:
            error, self.retirement_error = self.retirement_error, None
            raise error
        # Schedule the retirement on the host's own loop and return at once.
        self.loop.call_soon_threadsafe(self._start_retirement)

    def _start_retirement(self) -> None:
        self._tasks.append(self.loop.create_task(self._retire()))

    async def _retire(self) -> None:
        """The host's own cleanup: never run or awaited by the SDK."""
        self.cleanup_threads.append(threading.current_thread().name)
        if self.retire_ends_child:
            self.child.retire.set()
        joins = self.cleanup_joins_reader_of
        if joins is not None:
            # Joins the SDK's reader: a reader that waited for this cleanup
            # would never end.
            while not joins.reader_completed:
                await asyncio.sleep(0.01)
        gate = self.cleanup_gate
        if gate is not None:
            while not gate.is_set():
                await asyncio.sleep(0.01)
        self.cleanup_done.set()

    def wait_process(self, timeout: float) -> int:
        try:
            asyncio.get_running_loop()
            on_event_loop = True
        except RuntimeError:
            on_event_loop = False
        self.wait_calls.append((timeout, threading.current_thread().name, on_event_loop))
        with self._waits_lock:
            self._active_waits += 1
            self.most_concurrent_waits = max(self.most_concurrent_waits, self._active_waits)
        try:
            if self.wait_error is not None:
                error, self.wait_error = self.wait_error, None
                raise error
            if self.answers_pending_at_once and not self.child.exited.is_set():
                raise self.pending_error()
            if self.child.exited.wait(timeout):
                assert self.child.exit_code is not None
                self.events.append(f"exit {self.child.exit_code}")
                return self.child.exit_code
            raise self.pending_error()
        finally:
            with self._waits_lock:
                self._active_waits -= 1

    def close(self) -> None:
        self.loop.call_soon_threadsafe(self.loop.stop)
        self._loop_thread.join(_WAIT)
        self.loop.close()


@pytest.fixture
def adopt():
    """Make a stand-in child and its host; release both after the test."""
    made: list[tuple[FakeGatewayChild, FakeHost]] = []

    def make(
        script: Callable[[FakeGatewayChild], int],
        *,
        retire_ends_child: bool = True,
        endpoints: str = "raw",
    ) -> tuple[FakeGatewayChild, FakeHost]:
        child = FakeGatewayChild(script, endpoints=endpoints).start()
        host = FakeHost(child, retire_ends_child=retire_ends_child)
        made.append((child, host))
        return child, host

    yield make
    for child, host in made:
        if host.cleanup_gate is not None:
            host.cleanup_gate.set()
        child.retire.set()
        for stream in (child.input, child.output):
            try:
                stream.close()
            except OSError:
                pass
        child.exited.wait(_WAIT)
        host.close()


def accept_init(child: FakeGatewayChild, *, horizon_ms: int = 5_000) -> dict[str, Any]:
    init = child.read()
    assert init is not None and init["method"] == "meta/init"
    child.emit(
        {
            "jsonrpc": "2.0",
            "id": init["id"],
            "result": {
                "init_state": "accepted",
                "init_id": init["params"]["init_id"],
                "stdio_shutdown_handshake": True,
                "stdio_shutdown_horizon_ms": horizon_ms,
            },
        }
    )
    return init


def progress(child: FakeGatewayChild, init: dict[str, Any], phase: str) -> None:
    child.emit(
        {
            "jsonrpc": "2.0",
            "method": "meta/init_progress",
            "params": {"init_id": init["params"]["init_id"], "phase": phase},
        }
    )


def settle(child: FakeGatewayChild, init: dict[str, Any], **fields: Any) -> None:
    child.emit(
        {
            "jsonrpc": "2.0",
            "method": "meta/init_settled",
            "params": {"init_id": init["params"]["init_id"], **fields},
        }
    )


def serve(
    child: FakeGatewayChild, *, exit_code: int = 0, until_retired: bool = False
) -> int:
    """Answer requests until the input ends, then exit.

    With ``until_retired`` the child outlives its input until the host
    retires it.
    """
    while True:
        message = child.read()
        if message is None:
            break
        method = message.get("method")
        if method is None or method == "test/never_answered":
            continue
        if method == "meta/shutdown":
            result: dict[str, Any] = {"shutdown": True, "runtime_cleanup_completed": True}
        else:
            result = {"method": method, "params": message.get("params")}
        child.emit({"jsonrpc": "2.0", "id": message["id"], "result": result})
    if until_retired:
        child.retire.wait(30)
    return exit_code


def ready_gateway(
    *, exit_code: int = 0, until_retired: bool = False, horizon_ms: int = 5_000
) -> Callable[[FakeGatewayChild], int]:
    def script(child: FakeGatewayChild) -> int:
        init = accept_init(child, horizon_ms=horizon_ms)
        progress(child, init, "storage")
        settle(
            child,
            init,
            outcome="ready",
            http_base_url="http://127.0.0.1:9",
            http_public_base_url="https://gateway.example.test",
        )
        return serve(child, exit_code=exit_code, until_retired=until_retired)

    return script


def fails_closed(child: FakeGatewayChild) -> int:
    """Accepts init, settles it failed, and exits at once."""
    init = accept_init(child)
    settle(
        child,
        init,
        outcome="failed",
        code=-32014,
        message="refused for the test",
        durable_effects="possible",
    )
    return 0


def refuses_init(child: FakeGatewayChild) -> int:
    """Answers init with an error, then exits when its input ends."""
    init = child.read()
    assert init is not None
    child.emit(
        {
            "jsonrpc": "2.0",
            "id": init["id"],
            "error": {"code": -32602, "message": "refused for the test"},
        }
    )
    return serve(child)


async def wait_until(predicate: Callable[[], Any], timeout: float = _WAIT) -> None:
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while not predicate():
        assert loop.time() < deadline, "the expected state never came"
        await asyncio.sleep(0.01)


def start_build(
    transport: AdoptedGatewayTransport,
) -> tuple[asyncio.Task[MetaRuntime], dict[str, BaseException]]:
    """Run build() in a task that keeps the first exception it observes, as a
    caller without a runtime must (Python 3.10 re-raises a fresh
    CancelledError from a cancelled task)."""
    observed: dict[str, BaseException] = {}

    async def build() -> MetaRuntime:
        try:
            return await Meta.builder().gateway_transport(transport).build()
        except BaseException as error:
            observed["error"] = error
            raise

    return asyncio.create_task(build()), observed


def off_loop(host: FakeHost) -> bool:
    """Every wait_process call ran on a worker thread with a 0-60 s timeout,
    and never beside another one."""
    return (
        bool(host.wait_calls)
        and host.most_concurrent_waits <= 1
        and all(
            not on_event_loop and 0 <= timeout <= 60 and not thread.startswith(_HOST_LOOP)
            for timeout, thread, on_event_loop in host.wait_calls
        )
    )


@pytest.mark.parametrize("endpoints", ["raw", "buffered"])
@pytest.mark.asyncio
async def test_the_builder_adopts_the_host_child_and_settles_ready(adopt, endpoints):
    child, host = adopt(ready_gateway(), endpoints=endpoints)
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    try:
        streams = transport._process
        if endpoints == "raw":
            # Buffered by the transport, over the host's own raw objects: one
            # owner of each descriptor, no second file object.
            assert isinstance(streams.stdin, io.BufferedWriter)
            assert isinstance(streams.stdout, io.BufferedReader)
            assert streams.stdin.raw is child.input
            assert streams.stdout.raw is child.output
        else:
            assert streams.stdin is child.input and streams.stdout is child.output
        assert runtime.is_running
        assert runtime.rust_http_base_url == "http://127.0.0.1:9"
        assert runtime.rust_http_public_base_url == "https://gateway.example.test"
        assert runtime.gateway_shutdown_horizon_ms == 5_000
        init_params = child.received[0]["params"]
        assert init_params["init_protocol"] == "accepted_then_settled"
        assert init_params["init_id"].startswith("init-")
        # While the child serves, only the exit watcher calls the host:
        # wait_process, off the loop; no retirement is requested.
        assert host.retirement_threads == []
        assert {thread for _, thread, _ in host.wait_calls} == {"meta-exit-watch"}
        assert off_loop(host)
    finally:
        await runtime.shutdown()
    # The stock protocol shutdown: one shutdown request, then end of input.
    assert child.methods()[-1] == "meta/shutdown"
    assert child.saw_end_of_input.is_set()
    assert len(host.retirement_threads) == 1
    assert off_loop(host)
    assert transport.child_exit == GatewayChildExit("exited", 0)
    assert runtime.last_gateway_termination == TransportTermination(
        "exited", 0, None, "none", "acknowledged"
    )
    assert transport.reader_completed
    assert transport.start_failure is None
    assert not runtime.is_running
    # Closed at the transport's own close points, through the host's objects.
    assert child.input.closed and child.output.closed


@pytest.mark.asyncio
async def test_a_callback_handler_set_before_start_answers_during_init(adopt):
    answers: list[dict[str, Any] | None] = []

    def script(child: FakeGatewayChild) -> int:
        init = accept_init(child)
        progress(child, init, "restore")
        child.emit(
            {
                "jsonrpc": "2.0",
                "id": "cb-1",
                "method": "callback/roster_provider/roster",
                "params": {"context": {}},
            }
        )
        answers.append(child.read())
        settle(child, init, outcome="ready", http_base_url="http://127.0.0.1:9")
        return serve(child)

    class Roster:
        def __init__(self) -> None:
            self.calls = 0

        async def roster(self, context: Any) -> list[Any]:
            self.calls += 1
            return []

    child, host = adopt(script)
    roster = Roster()
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().roster(roster).gateway_transport(transport).build()
    await runtime.shutdown()
    assert child.received[0]["params"]["has_roster_provider"] is True
    assert roster.calls == 1
    # Answered by the provider: no "callback_handler_unavailable" refusal.
    assert answers == [{"jsonrpc": "2.0", "id": "cb-1", "result": []}]


@pytest.mark.asyncio
async def test_sync_and_async_rpc_and_request_timeouts(adopt):
    child, host = adopt(ready_gateway())
    transport = AdoptedGatewayTransport(host.carrier(), timeout=5.0)
    runtime = await Meta.builder().gateway_transport(transport).build()
    try:
        assert transport.request_timeout == 5.0
        assert await runtime._rpc("test/echo", {"n": 1}) == {
            "method": "test/echo",
            "params": {"n": 1},
        }
        answer = await asyncio.to_thread(
            transport.send_sync,
            {"jsonrpc": "2.0", "id": "sync-1", "method": "test/echo", "params": {"n": 2}},
        )
        assert answer["result"] == {"method": "test/echo", "params": {"n": 2}}
        never = {"jsonrpc": "2.0", "method": "test/never_answered", "params": {}}
        with pytest.raises(RuntimeError, match="timeout after 0.2s"):
            await transport.send_async({**never, "id": "late-1"}, timeout=0.2)
        with pytest.raises(RuntimeError, match="timeout after 0.2s"):
            await asyncio.to_thread(
                transport.send_sync, {**never, "id": "late-2"}, timeout=0.2
            )
        # A timed-out request leaves the channel usable.
        assert await runtime._rpc("test/echo", {"n": 3}) == {
            "method": "test/echo",
            "params": {"n": 3},
        }
    finally:
        await runtime.shutdown()


def pipe_buffer_size(stream: Any) -> int:
    """The kernel buffer of the pipe behind ``stream`` (64 KiB when unknown)."""
    try:
        import fcntl

        return fcntl.fcntl(stream.fileno(), fcntl.F_GETPIPE_SZ)
    except (ImportError, AttributeError, OSError):
        return 65_536


@pytest.mark.asyncio
async def test_lines_larger_than_the_pipe_buffer_cross_raw_endpoints_both_ways(adopt):
    # The carrier exactly as the host builds it: pipe2(O_CLOEXEC) ends behind
    # unbuffered FileIO objects.
    child, host = adopt(ready_gateway())
    assert isinstance(child.input, io.FileIO) and isinstance(child.output, io.FileIO)
    assert not os.get_inheritable(child.input.fileno())
    payload = "".join(chr(0x41 + index % 26) for index in range(1 << 20))
    assert len(payload) > 4 * max(
        pipe_buffer_size(child.input), pipe_buffer_size(child.output)
    )
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    try:
        # SDK to child (the request) and child to SDK (its echo).
        assert await runtime._rpc("test/echo", {"payload": payload}) == {
            "method": "test/echo",
            "params": {"payload": payload},
        }
        assert child.received[-1]["params"]["payload"] == payload
        answer = await asyncio.to_thread(
            transport.send_sync,
            {
                "jsonrpc": "2.0",
                "id": "big-sync",
                "method": "test/echo",
                "params": {"payload": payload[::-1]},
            },
        )
        assert answer["result"]["params"]["payload"] == payload[::-1]
    finally:
        await runtime.shutdown()
    assert transport.child_exit == GatewayChildExit("exited", 0)
    assert child.input.closed and child.output.closed


class ChoppyRaw(io.RawIOBase):
    """A raw pipe end that is interrupted once, then moves at most ``step``
    bytes per call: the short writes and partial reads a raw stream allows."""

    def __init__(self, raw: io.FileIO, step: int):
        super().__init__()
        self._raw = raw
        self._step = step
        self.interrupted = False
        self.calls = 0

    def readable(self) -> bool:
        return self._raw.readable()

    def writable(self) -> bool:
        return self._raw.writable()

    def fileno(self) -> int:
        return self._raw.fileno()

    def _interrupt_once(self) -> None:
        self.calls += 1
        if not self.interrupted:
            self.interrupted = True
            raise InterruptedError(errno.EINTR, "interrupted by a signal")

    def readinto(self, buffer: Any) -> int | None:
        self._interrupt_once()
        return self._raw.readinto(memoryview(buffer)[: self._step])

    def write(self, data: Any) -> int | None:
        self._interrupt_once()
        return self._raw.write(bytes(memoryview(data)[: self._step]))

    def close(self) -> None:
        if not self.closed:
            self._raw.close()
        super().close()


@pytest.mark.asyncio
async def test_raw_endpoints_survive_interrupted_short_writes_and_partial_reads(adopt):
    child, host = adopt(ready_gateway())
    stdin = ChoppyRaw(child.input, 1_000)
    stdout = ChoppyRaw(child.output, 997)
    transport = AdoptedGatewayTransport(
        AdoptedGatewayChild(
            input=stdin,
            output=stdout,
            request_retirement=host.request_retirement,
            wait_process=host.wait_process,
        )
    )
    payload = "".join(chr(0x61 + index % 26) for index in range(200_000))
    runtime = await Meta.builder().gateway_transport(transport).build()
    try:
        assert await runtime._rpc("test/echo", {"payload": payload}) == {
            "method": "test/echo",
            "params": {"payload": payload},
        }
        assert child.received[-1]["params"]["payload"] == payload
    finally:
        await runtime.shutdown()
    # Each direction was interrupted once and moved in many short pieces.
    assert stdin.interrupted and stdout.interrupted
    assert stdin.calls > len(payload) // 1_000
    assert stdout.calls > len(payload) // 997
    assert transport.child_exit == GatewayChildExit("exited", 0)
    # Closing each wrapper closed its raw stream and the host's end under it.
    assert stdin.closed and stdout.closed
    assert child.input.closed and child.output.closed


@pytest.mark.asyncio
async def test_endpoints_refused_at_the_hand_off_stay_with_the_host(adopt):
    def script(child: FakeGatewayChild) -> int:
        return serve(child)

    # The host closed one end after building the carrier.
    child, host = adopt(script)
    transport = AdoptedGatewayTransport(host.carrier())
    child.output.close()
    with pytest.raises(TransportError, match="closed before the hand-off"):
        transport.start()
    gc.collect()
    assert not child.input.closed
    assert not select.select([child.stdin], [], [], 0)[0], "nothing reached the child"

    # The output cannot be wrapped: the input's wrapper is detached, never
    # closed, so the host's input stays open.
    class ReadableOnce(io.RawIOBase):
        def __init__(self) -> None:
            super().__init__()
            self.asked = 0

        def readable(self) -> bool:
            self.asked += 1
            return self.asked == 1

        def readinto(self, buffer: Any) -> int:
            return 0

    child, host = adopt(script)
    transport = AdoptedGatewayTransport(
        AdoptedGatewayChild(
            input=child.input,
            output=ReadableOnce(),
            request_retirement=host.request_retirement,
            wait_process=host.wait_process,
        )
    )
    with pytest.raises(io.UnsupportedOperation):
        transport.start()
    gc.collect()
    assert not child.input.closed
    assert not select.select([child.stdin], [], [], 0)[0], "nothing reached the child"
    assert host.retirement_threads == [] and host.wait_calls == []


@pytest.mark.asyncio
async def test_repeated_stops_request_the_retirement_once_and_never_kill(adopt):
    child, host = adopt(ready_gateway())
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    await runtime.shutdown()
    await runtime.shutdown()
    await asyncio.to_thread(transport.stop)
    await asyncio.to_thread(transport.stop)
    assert len(host.retirement_threads) == 1
    assert child.methods().count("meta/shutdown") == 1
    # The transport's view of an adopted child cannot wait on, signal or kill it.
    for method in ("wait", "terminate", "kill", "send_signal"):
        assert not hasattr(adopted_module._AdoptedProcess, method)
    # The host's retirement ran on the host's own loop, never on an SDK thread.
    await asyncio.to_thread(host.cleanup_done.wait, _WAIT)
    assert host.cleanup_threads == [_HOST_LOOP]
    assert transport.child_exit == GatewayChildExit("exited", 0)


@pytest.mark.asyncio
async def test_stop_refuses_the_event_loop_thread_and_runs_on_a_worker_thread(adopt):
    release_exit = threading.Event()

    def script(child: FakeGatewayChild) -> int:
        code = ready_gateway()(child)
        # Outlives its input until the test lets it exit.
        release_exit.wait(30)
        return code

    child, host = adopt(script, retire_ends_child=False)
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    with pytest.raises(TransportError, match="worker thread"):
        transport.stop()
    # It did nothing: admission is open and nothing was asked or requested.
    assert runtime.is_running
    assert "meta/shutdown" not in child.methods()
    assert host.retirement_threads == []
    assert await runtime._rpc("test/echo", {}) == {"method": "test/echo", "params": {}}

    ticks = 0

    async def tick() -> None:
        nonlocal ticks
        while True:
            ticks += 1
            await asyncio.sleep(0.01)

    ticker = asyncio.create_task(tick())
    stopping = asyncio.create_task(asyncio.to_thread(transport.stop))
    await wait_until(lambda: host.wait_calls)
    before = ticks
    await asyncio.sleep(0.1)
    # The loop kept running while the stop observed the child.
    assert ticks > before
    assert not stopping.done()
    release_exit.set()
    await asyncio.wait_for(stopping, _WAIT)
    ticker.cancel()
    assert transport.child_exit == GatewayChildExit("exited", 0)
    assert off_loop(host)
    # The runtime finds its transport stopped.
    await runtime.shutdown()
    assert len(host.retirement_threads) == 1


@pytest.mark.asyncio
async def test_shutdown_and_failed_builds_run_the_stop_off_the_loop(adopt, monkeypatch):
    # The worker-only stop, reached through the runtime's own paths: a builder
    # configured before build(), shutdown(), a failed build(), and the cleanup
    # a failed build hands its caller.
    loop_thread = threading.current_thread().name
    stops: list[tuple[str, bool]] = []
    stock_stop = AdoptedGatewayTransport.stop

    def observed_stop(self: AdoptedGatewayTransport) -> None:
        try:
            asyncio.get_running_loop()
            on_event_loop = True
        except RuntimeError:
            on_event_loop = False
        stops.append((threading.current_thread().name, on_event_loop))
        stock_stop(self)

    monkeypatch.setattr(AdoptedGatewayTransport, "stop", observed_stop)

    def configure_builder(transport: AdoptedGatewayTransport) -> Any:
        return Meta.builder().gateway_transport(transport)

    _, host = adopt(ready_gateway())
    runtime = await configure_builder(AdoptedGatewayTransport(host.carrier())).build()
    await runtime.shutdown()

    _, refused_host = adopt(refuses_init)
    with pytest.raises(RpcError):
        await configure_builder(AdoptedGatewayTransport(refused_host.carrier())).build()

    _, failed_host = adopt(fails_closed)
    with pytest.raises(GatewayStartupCleanupError) as raised:
        await configure_builder(AdoptedGatewayTransport(failed_host.carrier())).build()
    await raised.value.cleanup.shutdown()

    assert len(stops) == 4
    assert all(not on_event_loop and thread != loop_thread for thread, on_event_loop in stops)
    for each in (host, refused_host, failed_host):
        assert len(each.retirement_threads) == 1
        assert off_loop(each)


@pytest.mark.asyncio
async def test_a_pending_exit_is_typed_and_its_observation_is_kept(adopt, monkeypatch):
    # Scale the bound: a 100 ms horizon, 200 ms more, in 50 ms waits.
    monkeypatch.setattr(adopted_module, "_PENDING_RETRY_SECONDS", 0.2)
    monkeypatch.setattr(adopted_module, "_WAIT_PROCESS_MAX_SECONDS", 0.05)
    child, host = adopt(
        ready_gateway(exit_code=3, until_retired=True, horizon_ms=100),
        retire_ends_child=False,
    )
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    with pytest.raises(GatewayChildPendingError) as raised:
        await runtime.shutdown()
    assert raised.value.child_exit == GatewayChildExit("pending", None)
    assert transport.child_exit == GatewayChildExit("pending", None)
    assert runtime.last_gateway_termination == TransportTermination(
        "unknown", None, None, "none", "acknowledged"
    )
    # Bounded retries, each a timeout of at most the cap, all off the loop.
    assert len(host.wait_calls) >= 3
    assert all(timeout <= 0.05 for timeout, _, _ in host.wait_calls)
    assert off_loop(host)
    assert len(host.retirement_threads) == 1
    # The runtime kept the transport: the next shutdown observes the same
    # child again, once the host has retired it.
    child.retire.set()
    await runtime.shutdown()
    assert transport.child_exit == GatewayChildExit("exited", 3)
    assert runtime.last_gateway_termination == TransportTermination(
        "exited", 3, None, "none", "acknowledged"
    )
    assert len(host.retirement_threads) == 1
    assert child.methods().count("meta/shutdown") == 1


@pytest.mark.asyncio
async def test_a_refused_init_retires_and_observes_the_child_without_a_spawn(adopt):
    child, host = adopt(refuses_init)
    transport = AdoptedGatewayTransport(host.carrier())
    with pytest.raises(RpcError) as raised:
        await (
            Meta.builder()
            .gateway("/nonexistent/rpc_gateway")
            .gateway_transport(transport)
            .build()
        )
    assert raised.value.code == -32602
    assert transport.start_failure == GatewayStartFailure(
        error=raised.value, init_outcome="refused"
    )
    assert transport.start_failure.error is raised.value
    assert len(host.retirement_threads) == 1
    # No shutdown request before acceptance: the stop only ends the input.
    assert "meta/shutdown" not in child.methods()
    assert child.saw_end_of_input.is_set()
    assert transport.child_exit == GatewayChildExit("exited", 0)
    assert transport.last_termination == TransportTermination(
        "exited", 0, None, "none", "not_negotiated"
    )
    assert off_loop(host)


@pytest.mark.asyncio
async def test_a_failed_settlement_keeps_the_native_failure_apart_from_the_exit(adopt):
    child, host = adopt(fails_closed)
    transport = AdoptedGatewayTransport(host.carrier())
    with pytest.raises(GatewayStartupCleanupError) as raised:
        await Meta.builder().gateway_transport(transport).build()
    failure = raised.value
    assert isinstance(failure.startup_error, StorageResolutionError)
    assert failure.startup_error.data["durable_effects"] == "possible"
    # The shutdown request found the gateway gone: the native shutdown answer
    # is its own outcome, apart from the observed exit.
    assert "gateway shutdown failed after bounded cleanup" in str(failure.cleanup_error)
    assert transport.start_failure == GatewayStartFailure(failure.startup_error, "failed")
    assert transport.child_exit == GatewayChildExit("exited", 0)
    record = TransportTermination("exited", 0, None, "none", "unanswered")
    assert failure.cleanup.last_termination == record
    # Nothing is left to stop: the exit was observed.
    await failure.cleanup.shutdown()
    assert len(host.retirement_threads) == 1


@pytest.mark.asyncio
async def test_a_cancelled_start_keeps_the_child_observation_and_its_uncertainty(adopt):
    init_seen = threading.Event()

    def script(child: FakeGatewayChild) -> int:
        init = child.read()
        assert init is not None
        init_seen.set()
        # Never answers: the caller gives up first.
        return serve(child)

    child, host = adopt(script)
    transport = AdoptedGatewayTransport(host.carrier())
    start, observed = start_build(transport)
    assert await asyncio.to_thread(init_seen.wait, _WAIT)
    start.cancel()
    with pytest.raises(asyncio.CancelledError):
        await start
    failure = transport.start_failure
    assert failure is not None
    assert type(failure.error) is asyncio.CancelledError
    assert failure.error is observed["error"]
    # The init request reached the gateway: its outcome stays unknown.
    assert failure.init_outcome == "unknown"
    assert len(host.retirement_threads) == 1
    assert transport.child_exit == GatewayChildExit("exited", 0)
    assert off_loop(host)


@pytest.mark.asyncio
async def test_a_second_cancellation_does_not_drop_the_child_observation(adopt):
    init_seen = threading.Event()

    def script(child: FakeGatewayChild) -> int:
        init = child.read()
        assert init is not None
        init_seen.set()
        # Never answers, and outlives its input until the host retires it.
        return serve(child, exit_code=5, until_retired=True)

    child, host = adopt(script, retire_ends_child=False)
    transport = AdoptedGatewayTransport(host.carrier())
    start, observed = start_build(transport)
    assert await asyncio.to_thread(init_seen.wait, _WAIT)
    start.cancel()
    # The failed start's stop requested the retirement and now waits for the
    # child's exit: cancel the caller again.
    await wait_until(lambda: host.retirement_threads)
    start.cancel()
    with pytest.raises(asyncio.CancelledError):
        await start
    error = observed["error"]
    assert type(error) is asyncio.CancelledError
    cleanup = error.gateway_cleanup
    assert transport.child_exit is None
    # The same runtime-owned stop keeps observing: the host retires the child,
    # and the wait already in progress sees the exit. No stop starts again.
    calls_before = len(host.wait_calls)
    child.retire.set()
    await cleanup.shutdown()
    assert transport.child_exit == GatewayChildExit("exited", 5)
    assert cleanup.last_termination == TransportTermination(
        "exited", 5, None, "none", "not_negotiated"
    )
    assert len(host.wait_calls) == calls_before
    assert len(host.retirement_threads) == 1
    assert transport.start_failure is not None
    assert transport.start_failure.init_outcome == "unknown"
    assert off_loop(host)


@pytest.mark.asyncio
async def test_refusals_before_the_hand_off_leave_the_pipes_untouched(adopt, tmp_path):
    def script(child: FakeGatewayChild) -> int:
        return serve(child)

    child, host = adopt(script)

    def carrier(**changes: Any) -> AdoptedGatewayChild:
        fields = {
            "input": child.input,
            "output": child.output,
            "request_retirement": host.request_retirement,
            "wait_process": host.wait_process,
            **changes,
        }
        return AdoptedGatewayChild(**fields)

    text = io.StringIO()
    closed = open(tmp_path / "closed", "wb")
    closed.close()
    with pytest.raises(TypeError):
        carrier(input=text)
    with pytest.raises(TypeError):
        carrier(output=object())
    with pytest.raises(ValueError):
        carrier(input=closed)
    with pytest.raises(ValueError):
        carrier(output=child.input)
    with pytest.raises(TypeError):
        carrier(request_retirement=None)
    with pytest.raises(TypeError):
        carrier(wait_process="not callable")
    with pytest.raises(TypeError):
        AdoptedGatewayTransport(object())  # type: ignore[arg-type]
    for timeout in (0, -1, float("nan"), float("inf"), True):
        with pytest.raises(ValueError):
            AdoptedGatewayTransport(carrier(), timeout=timeout)

    # A stop before the hand-off makes a start refuse, typed.
    stopped = AdoptedGatewayTransport(carrier())
    await asyncio.to_thread(stopped.stop)
    assert stopped.last_termination == TransportTermination(
        "not_started", None, None, "none", "not_negotiated"
    )
    with pytest.raises(GatewayTransportConflictError):
        stopped.start()
    with pytest.raises(GatewayTransportConflictError):
        await Meta.builder().gateway_transport(stopped).build()

    # Two host transports for one gateway.
    fresh = AdoptedGatewayTransport(carrier())
    config = Meta.builder().gateway_transport(fresh)._config
    with pytest.raises(GatewayTransportConflictError):
        await MetaRuntime(config, AdoptedGatewayTransport(carrier())).connect()

    # A factory that returns no transport.
    with pytest.raises(TypeError):
        await Meta.builder().gateway_transport(lambda: object()).build()
    with pytest.raises(TypeError):
        Meta.builder().gateway_transport("not a transport")  # type: ignore[arg-type]

    assert child.input_untouched()
    assert host.retirement_threads == [] and host.wait_calls == []
    assert child.received == []


@pytest.mark.asyncio
async def test_a_host_transport_never_spawns_even_with_gateway_set(adopt):
    # Through the builder, with gateway() set as well.
    child, host = adopt(ready_gateway())
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await (
        Meta.builder()
        .gateway("/nonexistent/rpc_gateway")
        .gateway_transport(transport)
        .build()
    )
    assert runtime._transport is transport
    await runtime.shutdown()
    assert transport.child_exit == GatewayChildExit("exited", 0)

    # A transport given to the constructor, with a gateway binary configured,
    # is started and initialized in place of a spawned child.
    child, host = adopt(ready_gateway())
    transport = AdoptedGatewayTransport(host.carrier())
    config = Meta.builder().gateway("/nonexistent/rpc_gateway")._config
    runtime = MetaRuntime(config, transport)
    await runtime.connect()
    assert runtime.rust_http_base_url == "http://127.0.0.1:9"
    await runtime.shutdown()
    assert transport.child_exit == GatewayChildExit("exited", 0)
    # One child, one connect: it is never replaced, reconnected or replayed.
    with pytest.raises(GatewayTransportConflictError):
        await runtime.connect()

    # A factory serves each connect with the child the host started for it.
    hosts: list[FakeHost] = []

    def factory() -> AdoptedGatewayTransport:
        _, next_host = adopt(ready_gateway())
        hosts.append(next_host)
        return AdoptedGatewayTransport(next_host.carrier())

    runtime = await Meta.builder().gateway_transport(factory).build()
    await runtime.shutdown()
    await runtime.connect()
    await runtime.shutdown()
    assert len(hosts) == 2
    assert [len(each.retirement_threads) for each in hosts] == [1, 1]


@pytest.mark.asyncio
async def test_the_sdk_never_awaits_the_host_cleanup_and_the_reader_never_calls_it(adopt):
    # The host's cleanup joins the SDK's reader, then waits for a gate that
    # opens only after the SDK's stop returned. A reader that waited for that
    # cleanup, or a stop that awaited it, would never finish.
    child, host = adopt(ready_gateway())
    transport = AdoptedGatewayTransport(host.carrier())
    host.cleanup_joins_reader_of = transport
    host.cleanup_gate = threading.Event()
    runtime = await Meta.builder().gateway_transport(transport).build()
    await asyncio.wait_for(runtime.shutdown(), _WAIT)
    # The stop returned while the host's own cleanup is still open.
    assert not host.cleanup_done.is_set()
    assert transport.reader_completed
    host.cleanup_gate.set()
    assert await asyncio.to_thread(host.cleanup_done.wait, _WAIT)
    # The SDK only requested the retirement, from its stop's worker thread:
    # never from its reader, and never by running the host's cleanup.
    assert len(host.retirement_threads) == 1
    assert host.retirement_threads[0] != "meta-reader"
    assert host.cleanup_threads == [_HOST_LOOP]


@pytest.mark.asyncio
async def test_the_child_exit_and_the_reader_completion_stay_apart(adopt, monkeypatch):
    monkeypatch.setattr(adopted_module, "_PENDING_RETRY_SECONDS", 0.2)
    closed_stdout = threading.Event()

    # The child closes its stdout and keeps running until it is retired.
    def closes_stdout_first(child: FakeGatewayChild) -> int:
        init = accept_init(child, horizon_ms=100)
        settle(child, init, outcome="ready", http_base_url="http://127.0.0.1:9")
        child.stdout.close()
        closed_stdout.set()
        child.retire.wait(30)
        return 7

    child, host = adopt(closes_stdout_first, retire_ends_child=False)
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    assert await asyncio.to_thread(closed_stdout.wait, _WAIT)
    await wait_until(lambda: transport.reader_completed)
    # The reader ended and closed the host's output itself, before any stop.
    assert child.output.closed
    assert not child.input.closed
    # The reader ended; the child has not exited.
    with pytest.raises(GatewayChildPendingError):
        await runtime.shutdown()
    assert transport.reader_completed
    assert transport.child_exit == GatewayChildExit("pending", None)
    assert transport.start_failure is None
    # Its shutdown answer is a native outcome of its own.
    assert transport.last_termination.native_shutdown == "unanswered"
    child.retire.set()
    with pytest.raises(RuntimeError, match="gateway shutdown failed"):
        await runtime.shutdown()
    assert transport.child_exit == GatewayChildExit("exited", 7)
    await runtime.shutdown()

    # The child exits while a descendant still holds its stdout.
    descendant_stdout: list[int] = []

    def exits_while_stdout_is_held(child: FakeGatewayChild) -> int:
        code = ready_gateway()(child)
        descendant_stdout.append(os.dup(child.stdout.fileno()))
        return code

    child, host = adopt(exits_while_stdout_is_held)
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    await runtime.shutdown()
    assert transport.child_exit == GatewayChildExit("exited", 0)
    assert not transport.reader_completed
    os.close(descendant_stdout[0])
    await wait_until(lambda: transport.reader_completed)


@pytest.mark.asyncio
async def test_a_failed_retirement_request_is_typed_and_requested_again(adopt):
    child, host = adopt(ready_gateway())
    host.retirement_error = RuntimeError("host loop is busy")
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    with pytest.raises(TransportError, match="request_retirement") as raised:
        await runtime.shutdown()
    assert isinstance(raised.value.__cause__, RuntimeError)
    # The child exited on its own end of input; the retirement is still owed.
    assert transport.child_exit == GatewayChildExit("exited", 0)
    await runtime.shutdown()
    assert len(host.retirement_threads) == 2


class RecordingRaw(io.RawIOBase):
    """A raw pipe end that records when the transport closes it."""

    def __init__(self, raw: io.FileIO, events: list[str], name: str):
        super().__init__()
        self._raw = raw
        self._events = events
        self._name = name

    def readable(self) -> bool:
        return self._raw.readable()

    def writable(self) -> bool:
        return self._raw.writable()

    def fileno(self) -> int:
        return self._raw.fileno()

    def readinto(self, buffer: Any) -> int | None:
        return self._raw.readinto(buffer)

    def write(self, data: Any) -> int | None:
        return self._raw.write(data)

    def close(self) -> None:
        if not self.closed:
            self._events.append(f"{self._name} closed")
            self._raw.close()
        super().close()


@pytest.mark.asyncio
async def test_stop_closes_input_then_requests_retirement_then_sees_the_exit(
    adopt, monkeypatch
):
    # The child outlives its input and exits only when its host retires it,
    # within a short bound: a stop that requested the retirement late, or
    # closed the input after it, fails here.
    monkeypatch.setattr(adopted_module, "_PENDING_RETRY_SECONDS", 0.3)
    child, host = adopt(ready_gateway(exit_code=4, until_retired=True, horizon_ms=200))
    transport = AdoptedGatewayTransport(
        AdoptedGatewayChild(
            input=RecordingRaw(child.input, host.events, "input"),
            output=child.output,
            request_retirement=host.request_retirement,
            wait_process=host.wait_process,
        )
    )
    runtime = await Meta.builder().gateway_transport(transport).build()
    await runtime.shutdown()
    assert child.methods()[-1] == "meta/shutdown"
    events = host.events
    assert events.index("input closed") < events.index("retirement requested")
    assert events.index("retirement requested") < events.index("exit 4")
    assert transport.child_exit == GatewayChildExit("exited", 4)
    assert off_loop(host)


@pytest.mark.asyncio
async def test_start_does_nothing_while_the_child_serves_and_is_refused_after_a_stop(adopt):
    child, host = adopt(ready_gateway())
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    reader, process = transport._reader_thread, transport._process
    received = len(child.received)
    transport.start()
    # No second hand-off: the same reader on the same pipes, nothing written.
    assert transport._reader_thread is reader and transport._process is process
    assert len(child.received) == received
    assert await runtime._rpc("test/echo", {}) == {"method": "test/echo", "params": {}}
    await runtime.shutdown()
    with pytest.raises(GatewayTransportConflictError):
        transport.start()
    assert transport._reader_thread is reader


@pytest.mark.asyncio
async def test_the_first_stop_waits_through_the_shutdown_horizon_and_no_longer(
    adopt, monkeypatch
):
    monkeypatch.setattr(adopted_module, "_PENDING_RETRY_SECONDS", 0.1)
    loop = asyncio.get_running_loop()

    # Exits half a second after its input ends: within the 800 ms horizon the
    # gateway advertised, beyond the graces that follow it.
    def exits_late(child: FakeGatewayChild) -> int:
        code = ready_gateway(horizon_ms=800)(child)
        threading.Event().wait(0.5)
        return code

    child, host = adopt(exits_late, retire_ends_child=False)
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    await runtime.shutdown()
    assert transport.child_exit == GatewayChildExit("exited", 0)

    # A child that never exits: pending once the horizon and the graces end.
    child, host = adopt(
        ready_gateway(until_retired=True, horizon_ms=800), retire_ends_child=False
    )
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    started = loop.time()
    with pytest.raises(GatewayChildPendingError):
        await runtime.shutdown()
    assert 0.8 <= loop.time() - started < 3.0


@pytest.mark.asyncio
async def test_an_exit_while_stdout_is_held_fails_a_pending_init_promptly(adopt):
    held: list[int] = []

    # Accepts init, then exits while a process it started keeps its stdout.
    def exits_after_accepting(child: FakeGatewayChild) -> int:
        accept_init(child)
        held.append(os.dup(child.stdout.fileno()))
        return 9

    child, host = adopt(exits_after_accepting)
    transport = AdoptedGatewayTransport(host.carrier())
    loop = asyncio.get_running_loop()
    started = loop.time()
    try:
        with pytest.raises(GatewayStartupCleanupError) as raised:
            await asyncio.wait_for(Meta.builder().gateway_transport(transport).build(), _WAIT)
        # No init deadline was set: the exit, not a timer, ended the wait.
        assert loop.time() - started < 4.0
        assert isinstance(raised.value.startup_error, InitOutcomeUnknownError)
        assert transport.child_exit == GatewayChildExit("exited", 9)
        assert not transport.reader_completed
        assert transport.start_failure.init_outcome == "unknown"
    finally:
        for fd in held:
            os.close(fd)
    await wait_until(lambda: transport.reader_completed)


@pytest.mark.asyncio
async def test_an_exit_while_stdout_is_held_fails_requests_in_flight_promptly(adopt):
    held: list[int] = []

    def exits_on_request(child: FakeGatewayChild) -> int:
        init = accept_init(child)
        settle(child, init, outcome="ready", http_base_url="http://127.0.0.1:9")
        while True:
            message = child.read()
            if message is None or message.get("method") == "test/exit":
                held.append(os.dup(child.stdout.fileno()))
                return 6

    child, host = adopt(exits_on_request)
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    loop = asyncio.get_running_loop()
    started = loop.time()
    try:
        # Never answered: the exit fails it, long before its 60 s timeout.
        with pytest.raises(GatewayChildEndedError) as raised:
            await runtime._rpc("test/exit", {})
        assert loop.time() - started < 4.0
        assert raised.value.exit_code == 6
        assert (await runtime.wait_gateway_ended(_WAIT)).exit_code == 6
        assert not runtime.is_running
        assert transport.child_exit == GatewayChildExit("exited", 6)
        assert not transport.reader_completed
        # Nobody can answer the shutdown request; the exit is already known.
        with pytest.raises(RuntimeError, match="gateway shutdown failed"):
            await runtime.shutdown()
        await runtime.shutdown()
        assert len(host.retirement_threads) == 1
    finally:
        for fd in held:
            os.close(fd)
    await wait_until(lambda: transport.reader_completed)


@pytest.mark.asyncio
async def test_a_constructor_adopted_transport_starts_with_init_and_not_a_request(adopt):
    child, host = adopt(ready_gateway())
    transport = AdoptedGatewayTransport(host.carrier())
    # A request never takes the child: only start() hands it over.
    with pytest.raises(TransportError, match="handed over by start"):
        transport.send_sync({"jsonrpc": "2.0", "id": "early", "method": "test/echo"})
    assert child.input_untouched() and child.received == []
    # No gateway binary configured: the adopted transport is the gateway.
    runtime = MetaRuntime(Meta.builder()._config, transport)
    await runtime.connect()
    try:
        assert child.received[0]["method"] == "meta/init"
        assert runtime.rust_http_base_url == "http://127.0.0.1:9"
        assert await runtime._rpc("test/echo", {}) == {"method": "test/echo", "params": {}}
    finally:
        await runtime.shutdown()
    assert transport.child_exit == GatewayChildExit("exited", 0)


def pipe_bytes_waiting(stream: Any) -> int:
    """How many bytes wait unread in the pipe behind ``stream``."""
    import fcntl
    import struct
    import termios

    raw = fcntl.ioctl(stream.fileno(), termios.FIONREAD, b"\0\0\0\0")
    return struct.unpack("i", raw)[0]


def stalls_its_input(may_read: threading.Event) -> Callable[[FakeGatewayChild], int]:
    """Settles ready, then stops reading its stdin until ``may_read`` or its
    retirement; then it serves (after its retirement it exits at once)."""

    def script(child: FakeGatewayChild) -> int:
        init = accept_init(child, horizon_ms=300)
        settle(child, init, outcome="ready", http_base_url="http://127.0.0.1:9")
        while not (may_read.is_set() or child.retire.is_set()):
            threading.Event().wait(0.01)
        if child.retire.is_set():
            return 0
        return serve(child)

    return script


@pytest.mark.asyncio
async def test_a_writer_stuck_on_a_full_pipe_cannot_hold_back_the_retirement(adopt):
    child, host = adopt(stalls_its_input(threading.Event()))
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    loop = asyncio.get_running_loop()
    payload = "x" * (4 << 20)
    stuck = asyncio.create_task(
        asyncio.to_thread(
            transport.send_sync,
            {"jsonrpc": "2.0", "id": "big", "method": "test/echo", "params": {"p": payload}},
            timeout=_WAIT,
        )
    )
    # The writer fills the pipe the child no longer reads, and blocks.
    await wait_until(lambda: pipe_bytes_waiting(child.stdin) >= pipe_buffer_size(child.stdin))
    started = loop.time()
    stopping = asyncio.create_task(runtime.shutdown())
    await wait_until(lambda: host.retirement_threads)
    # Requested once the 300 ms horizon of the protocol phase ended, not later.
    assert loop.time() - started < 2.0
    # The retirement ended the child, which freed the stuck writer.
    with pytest.raises(RuntimeError, match="gateway shutdown failed"):
        await asyncio.wait_for(stopping, _WAIT)
    with pytest.raises(OSError):
        await asyncio.wait_for(stuck, _WAIT)
    assert transport.child_exit == GatewayChildExit("exited", 0)
    await runtime.shutdown()
    assert len(host.retirement_threads) == 1


@pytest.mark.skipif(
    not hasattr(signal, "pthread_kill") or not hasattr(signal, "SIGUSR1"),
    reason="needs POSIX thread-directed signals",
)
@pytest.mark.asyncio
async def test_a_handled_signal_during_a_blocked_raw_write_loses_no_bytes(adopt):
    may_read = threading.Event()
    child, host = adopt(stalls_its_input(may_read))
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    handled: list[int] = []
    previous = signal.signal(signal.SIGUSR1, lambda signum, frame: handled.append(signum))
    payload = "".join(chr(0x41 + index % 26) for index in range(4 << 20))
    writer: list[int] = []

    def send() -> Any:
        writer.append(threading.get_ident())
        return transport.send_sync(
            {"jsonrpc": "2.0", "id": "big", "method": "test/echo", "params": {"p": payload}},
            timeout=_WAIT * 2,
        )

    try:
        sending = asyncio.create_task(asyncio.to_thread(send))
        await wait_until(
            lambda: writer and pipe_bytes_waiting(child.stdin) >= pipe_buffer_size(child.stdin)
        )
        # The write is blocked on the full pipe: interrupt it with a signal
        # this process handles. The raw write returns short; nothing is lost.
        signal.pthread_kill(writer[0], signal.SIGUSR1)
        await wait_until(lambda: handled)
        may_read.set()
        answer = await asyncio.wait_for(sending, _WAIT * 2)
        assert answer["result"]["params"]["p"] == payload
        assert child.received[-1]["params"]["p"] == payload
    finally:
        signal.signal(signal.SIGUSR1, previous)
        may_read.set()
        await runtime.shutdown()


@pytest.mark.asyncio
async def test_a_dropped_transport_ends_input_and_requests_retirement_from_its_finalizer(
    adopt,
):
    # The child exits: the reader and the exit watcher end, and with them the
    # transport's last references. Its finalizer may run on either of their
    # threads, or in a collection here.
    finish = threading.Event()

    def exits_on_signal(child: FakeGatewayChild) -> int:
        finish.wait(_WAIT)
        return 0

    child, host = adopt(exits_on_signal)
    transport = AdoptedGatewayTransport(host.carrier())
    transport.start()
    del transport
    gc.collect()
    finish.set()
    await wait_until(lambda: gc.collect() >= 0 and host.retirement_threads)
    # Requested once, directly, by the finalizer; no thread was started for it.
    assert len(host.retirement_threads) == 1
    assert "meta-retirement" not in {thread.name for thread in threading.enumerate()}
    # The transport's own input was ended, through the host's object.
    assert child.input.closed


@pytest.mark.asyncio
async def test_other_timeout_kinds_count_as_pending_and_early_answers_do_not_spin(
    adopt, monkeypatch
):
    monkeypatch.setattr(adopted_module, "_PENDING_RETRY_SECONDS", 0.3)
    child, host = adopt(
        ready_gateway(until_retired=True, horizon_ms=100), retire_ends_child=False
    )
    # A host that wraps Popen.wait, and answers "pending" without waiting.
    host.pending_error = lambda: subprocess.TimeoutExpired("gateway", 0)
    host.answers_pending_at_once = True
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    with pytest.raises(GatewayChildPendingError) as raised:
        await runtime.shutdown()
    # Pending, not a failed observation, and no busy loop on the host.
    assert raised.value.__cause__ is None
    assert len(host.wait_calls) < 100
    assert off_loop(host)
    child.retire.set()
    host.answers_pending_at_once = False
    await runtime.shutdown()
    assert transport.child_exit == GatewayChildExit("exited", 0)


def hold_thread_start(monkeypatch, name: str, *, then_fail: bool = False):
    """Hold the start of the SDK thread called ``name`` until released; with
    ``then_fail``, its start then fails as a host out of threads would."""
    held = threading.Event()
    release = threading.Event()
    stock_start = threading.Thread.start

    def start(thread: threading.Thread) -> None:
        if thread.name == name and not release.is_set():
            held.set()
            release.wait(_WAIT)
            if then_fail:
                raise RuntimeError("can't start new thread")
        stock_start(thread)

    monkeypatch.setattr(threading.Thread, "start", start)
    return held, release


def stop_on_a_thread(transport: AdoptedGatewayTransport) -> tuple[threading.Thread, dict]:
    outcome: dict[str, Any] = {}

    def stop() -> None:
        try:
            transport.stop()
        except BaseException as error:  # noqa: BLE001 - recorded for the test
            outcome["error"] = error
        outcome["returned"] = time.monotonic()

    thread = threading.Thread(target=stop, name="host-stop")
    thread.start()
    return thread, outcome


def test_a_stop_during_the_hand_off_never_calls_wait_process_beside_the_watcher(
    adopt, monkeypatch
):
    # The hand-off is held between publishing the child and starting its
    # threads; a stop that comes then must wait for the watcher, not call
    # wait_process itself.
    child, host = adopt(lambda child: serve(child))
    transport = AdoptedGatewayTransport(host.carrier())
    held, release = hold_thread_start(monkeypatch, "meta-reader")
    starting = threading.Thread(target=transport.start, name="host-start")
    starting.start()
    assert held.wait(_WAIT)
    stopping, outcome = stop_on_a_thread(transport)
    assert child.exited.wait(_WAIT)  # the stop ended the child's input
    threading.Event().wait(0.2)
    assert host.wait_calls == [], "the stop called wait_process beside the watcher"
    release.set()
    for thread in (starting, stopping):
        thread.join(_WAIT)
    assert "error" not in outcome
    assert {thread for _, thread, _ in host.wait_calls} == {"meta-exit-watch"}
    assert off_loop(host)
    assert transport.child_exit == GatewayChildExit("exited", 0)


def test_a_watcher_that_cannot_start_wakes_a_stop_that_waits_for_it(adopt, monkeypatch):
    # Short bounds, so a stop left asleep would end pending soon.
    monkeypatch.setattr("meerkat_meta._transport._GATEWAY_SHUTDOWN_GRACE_SECONDS", 2.0)
    monkeypatch.setattr(adopted_module, "_PENDING_RETRY_SECONDS", 0.5)
    child, host = adopt(lambda child: serve(child))
    transport = AdoptedGatewayTransport(host.carrier())
    held, release = hold_thread_start(monkeypatch, "meta-exit-watch", then_fail=True)
    started: dict[str, BaseException] = {}

    def start() -> None:
        try:
            transport.start()
        except BaseException as error:  # noqa: BLE001 - recorded for the test
            started["error"] = error

    starting = threading.Thread(target=start, name="host-start")
    starting.start()
    assert held.wait(_WAIT)
    stopping, outcome = stop_on_a_thread(transport)
    # The stop requested the retirement and now waits for the watcher.
    deadline = time.monotonic() + _WAIT
    while not host.retirement_threads:
        assert time.monotonic() < deadline
        threading.Event().wait(0.01)
    threading.Event().wait(0.2)
    released = time.monotonic()
    release.set()
    for thread in (starting, stopping):
        thread.join(_WAIT)
    assert isinstance(started["error"], RuntimeError)
    # Woken when the watcher failed to start, it observed the exit itself.
    assert "error" not in outcome
    assert outcome["returned"] - released < 1.0
    assert transport.child_exit == GatewayChildExit("exited", 0)
    assert {thread for _, thread, _ in host.wait_calls} == {"host-stop"}
    assert off_loop(host)
    assert len(host.retirement_threads) == 1


@pytest.mark.asyncio
async def test_the_watcher_ends_once_a_stop_returned(adopt, monkeypatch):
    monkeypatch.setattr(adopted_module, "_PENDING_RETRY_SECONDS", 0.2)
    monkeypatch.setattr(adopted_module, "_WAIT_PROCESS_MAX_SECONDS", 0.05)
    child, host = adopt(
        ready_gateway(exit_code=2, until_retired=True, horizon_ms=100),
        retire_ends_child=False,
    )
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    with pytest.raises(GatewayChildPendingError):
        await runtime.shutdown()

    def watcher_calls() -> int:
        return sum(thread == "meta-exit-watch" for _, thread, _ in host.wait_calls)

    # At most the wait in progress finishes; then the watcher is gone.
    after_stop = watcher_calls()
    await asyncio.sleep(0.3)
    assert watcher_calls() <= after_stop + 1
    assert not transport._watcher_running
    # A later stop observes the child itself.
    child.retire.set()
    await runtime.shutdown()
    assert transport.child_exit == GatewayChildExit("exited", 2)
    assert host.wait_calls[-1][1] != "meta-exit-watch"
    assert off_loop(host)


@pytest.mark.asyncio
async def test_a_watcher_whose_wait_process_fails_leaves_the_next_stop_to_observe(adopt):
    child, host = adopt(ready_gateway(exit_code=5))
    host.wait_error = RuntimeError("the host's process table is busy")
    transport = AdoptedGatewayTransport(host.carrier())
    runtime = await Meta.builder().gateway_transport(transport).build()
    await wait_until(lambda: not transport._watcher_running)
    assert [thread for _, thread, _ in host.wait_calls] == ["meta-exit-watch"]
    # The stop falls back and observes the exit on its own worker thread.
    await runtime.shutdown()
    assert transport.child_exit == GatewayChildExit("exited", 5)
    assert host.wait_calls[-1][1] != "meta-exit-watch"
    assert off_loop(host)
