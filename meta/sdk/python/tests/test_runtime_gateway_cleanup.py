"""Runtime ownership of gateway cleanup.

A transport whose stop failed stays with its runtime until a later stop
succeeds, a reconnect settles it before making another transport, and a
failed startup whose stop failed reaches the same runtime through a typed
error carrying the exact startup error and the exact stop failure. Each test
drives stand-in gateway scripts, so the real process path runs without a
gateway binary.
"""
from __future__ import annotations

import asyncio
from contextlib import suppress
import json
import os
from pathlib import Path
import signal
import sys
import textwrap
import threading
from typing import Callable

import pytest

from meerkat_meta import (
    GatewayStartupCleanupError,
    Meta,
    NotConnectedError,
    TransportTermination,
)
import meerkat_meta._transport as transport_module
from meerkat_meta._transport import PersistentTransport
from meerkat_meta.runtime import MetaRuntime

pytestmark = pytest.mark.skipif(
    os.name != "posix", reason="the stand-in gateways rely on POSIX signals"
)

# Answers every request with the configured result (``meta/shutdown`` with an
# error when ``shutdown_error`` is set); a silent one answers nothing. A
# stubborn one ignores stdin EOF and SIGTERM, so only SIGKILL ends it.
_GATEWAY = """
import json, pathlib, signal, sys, time
config = json.loads(pathlib.Path(sys.argv[0]).with_suffix(".json").read_text())
if config["stubborn"]:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
for line in sys.stdin:
    request = json.loads(line)
    if "method" not in request or config["silent"]:
        continue
    answer = {"jsonrpc": "2.0", "id": request["id"]}
    if request["method"] == "meta/shutdown" and config["shutdown_error"]:
        answer["error"] = {"code": -32000, "message": config["shutdown_error"]}
    else:
        answer["result"] = config["result"]
    sys.stdout.write(json.dumps(answer) + "\\n")
    sys.stdout.flush()
while config["stubborn"]:
    time.sleep(1)
"""


def _write_gateway(
    directory: Path,
    name: str,
    *,
    stubborn: bool = False,
    silent: bool = False,
    result: dict | None = None,
    shutdown_error: str | None = None,
) -> str:
    script = directory / f"{name}.py"
    script.write_text(f"#!{sys.executable}\n{textwrap.dedent(_GATEWAY)}")
    script.chmod(0o755)
    script.with_suffix(".json").write_text(
        json.dumps(
            {
                "stubborn": stubborn,
                "silent": silent,
                "result": result or {},
                "shutdown_error": shutdown_error,
            }
        )
    )
    return str(script)


class StubbornTransport(PersistentTransport):
    """Its gateway survives bounded cleanup while ``allow_kill`` is false: the
    transport's kill attempts reach the real process only when allowed."""

    def __init__(
        self, gateway_bin: str, *, fail_startup: BaseException | None = None
    ):
        super().__init__(gateway_bin)
        self.allow_kill = False
        self.fail_startup = fail_startup

    def start(self) -> None:
        super().start()
        process = self._process
        kill = process.kill
        process.kill = lambda: kill() if self.allow_kill else None
        if self.fail_startup is not None:
            raise self.fail_startup


def _make_transports(
    monkeypatch, make: Callable[[int, str], PersistentTransport]
) -> list[PersistentTransport]:
    """Have the runtime make its transports with ``make(index, gateway_bin)``."""
    made: list[PersistentTransport] = []

    def factory(gateway_bin: str) -> PersistentTransport:
        transport = make(len(made), gateway_bin)
        made.append(transport)
        return transport

    monkeypatch.setattr("meerkat_meta.runtime.PersistentTransport", factory)
    return made


@pytest.fixture
def quick_cleanup(monkeypatch):
    """Scale the stop's bounded waits down for stand-ins that ignore them."""
    for name in (
        "_GATEWAY_SHUTDOWN_GRACE_SECONDS",
        "_PROCESS_TERMINATE_GRACE_SECONDS",
        "_PROCESS_KILL_GRACE_SECONDS",
    ):
        monkeypatch.setattr(transport_module, name, 0.2)


async def _release(runtime: MetaRuntime, made: list[PersistentTransport]) -> None:
    for transport in made:
        if isinstance(transport, StubbornTransport):
            transport.allow_kill = True
    with suppress(Exception):
        await runtime.shutdown()
    # A failed assertion can leave a child the runtime no longer reaches; a
    # stubborn stand-in would otherwise outlive the test.
    for transport in made:
        with suppress(Exception):
            await asyncio.to_thread(transport.stop)


_UNSETTLED = TransportTermination("unknown", None, None, "sigkill", "not_negotiated")
_KILLED = TransportTermination("exited", None, "SIGKILL", "sigkill", "not_negotiated")


@pytest.mark.asyncio
async def test_a_startup_failure_whose_cleanup_does_not_settle_keeps_the_gateway(
    tmp_path, monkeypatch, quick_cleanup
):
    stubborn_gateway = _write_gateway(tmp_path, "stubborn", stubborn=True)
    plain_gateway = _write_gateway(tmp_path, "plain")
    refused = RuntimeError("host bootstrap refused")

    def make(index: int, gateway_bin: str) -> PersistentTransport:
        if index == 0:
            return StubbornTransport(gateway_bin, fail_startup=refused)
        return PersistentTransport(plain_gateway)

    made = _make_transports(monkeypatch, make)
    runtime = MetaRuntime(Meta.builder().gateway(stubborn_gateway)._config)
    try:
        with pytest.raises(GatewayStartupCleanupError) as raised:
            await runtime.connect()
        error = raised.value
        assert error.startup_error is refused
        assert error.__cause__ is refused
        assert "did not terminate after bounded cleanup" in str(error.cleanup_error)
        assert error.cleanup.last_termination == _UNSETTLED
        stubborn = made[0]
        child = stubborn._process
        assert child is not None and child.poll() is None, "the live child stays owned"
        assert runtime._transport is stubborn
        assert not runtime.is_running

        # A reconnect settles the retained transport first and, while it
        # cannot, makes no other transport.
        with pytest.raises(RuntimeError, match="did not terminate after bounded cleanup"):
            await runtime.connect()
        assert len(made) == 1
        assert child.poll() is None

        # The cleanup rejoins the stop of that same child.
        stubborn.allow_kill = True
        await error.cleanup.shutdown()
        assert child.returncode == -signal.SIGKILL
        assert runtime._transport is None
        assert runtime.last_gateway_termination == _KILLED
        assert error.cleanup.last_termination == _KILLED

        await runtime.connect()
        assert len(made) == 2, "once settled, a reconnect makes a new transport"
        assert runtime.is_running
    finally:
        await _release(runtime, made)


@pytest.mark.asyncio
async def test_a_shutdown_whose_stop_does_not_settle_keeps_the_transport(
    tmp_path, monkeypatch, quick_cleanup
):
    gateway = _write_gateway(tmp_path, "stubborn", stubborn=True)
    made = _make_transports(
        monkeypatch, lambda index, gateway_bin: StubbornTransport(gateway_bin)
    )
    runtime = MetaRuntime(Meta.builder().gateway(gateway)._config)
    try:
        await runtime.connect()
        (transport,) = made
        child = transport._process
        with pytest.raises(RuntimeError, match="did not terminate after bounded cleanup"):
            await runtime.shutdown()
        assert not runtime.is_running, "public admission closed"
        with pytest.raises(NotConnectedError):
            await runtime._rpc("meta/status", {})
        assert runtime._transport is transport
        assert transport._process is child, "the transport kept the exact live child"
        assert child.poll() is None
        assert runtime.last_gateway_termination == _UNSETTLED

        # The second shutdown rejoins the stop of that same child.
        transport.allow_kill = True
        await runtime.shutdown()
        assert child.returncode == -signal.SIGKILL, "the same original child ended"
        assert runtime._transport is None
        assert runtime.last_gateway_termination == _KILLED
        await runtime.shutdown()
        assert runtime.last_gateway_termination == _KILLED
    finally:
        await _release(runtime, made)


@pytest.mark.asyncio
async def test_a_rejoined_shutdown_keeps_the_gateways_one_shutdown_answer(
    tmp_path, monkeypatch, quick_cleanup
):
    # One result answers both meta/init (negotiating the handshake) and
    # meta/shutdown (acknowledging the runtime cleanup).
    gateway = _write_gateway(
        tmp_path,
        "stubborn",
        stubborn=True,
        result={
            "stdio_shutdown_handshake": True,
            "stdio_shutdown_horizon_ms": 300,
            "shutdown": True,
            "runtime_cleanup_completed": True,
        },
    )
    made = _make_transports(
        monkeypatch, lambda index, gateway_bin: StubbornTransport(gateway_bin)
    )
    runtime = MetaRuntime(Meta.builder().gateway(gateway)._config)
    try:
        await runtime.connect()
        (transport,) = made
        with pytest.raises(RuntimeError, match="did not terminate after bounded cleanup"):
            await runtime.shutdown()
        assert runtime.last_gateway_termination == TransportTermination(
            "unknown", None, None, "sigkill", "acknowledged"
        )

        # The rejoin finishes the same child without a second shutdown
        # request on the stdin the first stop closed: the answer stands and
        # the rejoin succeeds.
        transport.allow_kill = True
        await runtime.shutdown()
        assert runtime.last_gateway_termination == TransportTermination(
            "exited", None, "SIGKILL", "sigkill", "acknowledged"
        )
    finally:
        await _release(runtime, made)


@pytest.mark.asyncio
async def test_builder_build_keeps_a_settled_failure_and_reaches_an_unsettled_one(
    tmp_path, monkeypatch, quick_cleanup
):
    plain_gateway = _write_gateway(tmp_path, "plain")
    stubborn_gateway = _write_gateway(tmp_path, "stubborn", stubborn=True)
    made = _make_transports(
        monkeypatch,
        lambda index, gateway_bin: StubbornTransport(
            gateway_bin, fail_startup=RuntimeError("host bootstrap refused")
        ),
    )
    try:
        # The cleanup settles: the original startup error, unchanged.
        with pytest.raises(RuntimeError, match="host bootstrap refused") as settled:
            await Meta.builder().gateway(plain_gateway).build()
        assert settled.value is made[0].fail_startup
        assert made[0].last_termination == TransportTermination(
            "exited", 0, None, "none", "not_negotiated"
        )

        # The cleanup does not settle: no runtime is returned, but the typed
        # error reaches the same runtime's cleanup.
        with pytest.raises(GatewayStartupCleanupError) as unsettled:
            await Meta.builder().gateway(stubborn_gateway).build()
        cleanup = unsettled.value.cleanup
        stubborn = made[1]
        assert unsettled.value.startup_error is stubborn.fail_startup
        assert cleanup.last_termination == _UNSETTLED
        stubborn.allow_kill = True
        await cleanup.shutdown()
        assert cleanup.last_termination == _KILLED
        assert stubborn._process is None
    finally:
        for transport in made:
            transport.allow_kill = True
            with suppress(Exception):
                await asyncio.to_thread(transport.stop)


def _cancelled() -> BaseException:
    return asyncio.CancelledError()


def _cancellation_group() -> BaseException:
    # Not itself a CancelledError, and not an ordinary Exception either.
    return BaseExceptionGroup("startup interrupted", [asyncio.CancelledError()])


_GROUPS = pytest.mark.skipif(
    sys.version_info < (3, 11), reason="exception groups are builtin from Python 3.11"
)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "interruption",
    [_cancelled, pytest.param(_cancellation_group, marks=_GROUPS)],
)
async def test_a_cancelled_startup_stays_cancelled_and_its_gateway_stays_owned(
    tmp_path, monkeypatch, quick_cleanup, interruption
):
    gateway = _write_gateway(tmp_path, "stubborn", stubborn=True)
    startup_error = interruption()
    made = _make_transports(
        monkeypatch,
        lambda index, gateway_bin: StubbornTransport(
            gateway_bin, fail_startup=startup_error
        ),
    )
    runtime = MetaRuntime(Meta.builder().gateway(gateway)._config)
    try:
        # Only an ordinary startup error is wrapped; a cancellation (or a group
        # holding one) propagates as itself, and the runtime still owns the
        # unsettled gateway.
        with pytest.raises(BaseException) as raised:
            await runtime.connect()
        assert raised.value is startup_error
        (transport,) = made
        assert runtime._transport is transport
        cleanup = startup_error.gateway_cleanup
        assert cleanup.last_termination == _UNSETTLED

        transport.allow_kill = True
        await cleanup.shutdown()
        assert runtime._transport is None
        assert runtime.last_gateway_termination == _KILLED
    finally:
        await _release(runtime, made)


@pytest.mark.asyncio
async def test_a_refused_shutdown_after_the_exit_stays_visible_and_the_next_shutdown_settles(
    tmp_path,
):
    gateway = _write_gateway(
        tmp_path,
        "refusing",
        result={"stdio_shutdown_handshake": True, "stdio_shutdown_horizon_ms": 2000},
        shutdown_error="cleanup refused",
    )
    runtime = MetaRuntime(Meta.builder().gateway(gateway)._config)
    try:
        await runtime.connect()
        with pytest.raises(
            RuntimeError, match="gateway shutdown failed after bounded cleanup"
        ) as failed:
            await runtime.shutdown()
        assert "cleanup refused" in str(failed.value.__cause__)
        refused = TransportTermination("exited", 0, None, "none", "refused")
        assert runtime.last_gateway_termination == refused

        # The child is already released: the retained stop succeeds at once.
        await runtime.shutdown()
        assert runtime._transport is None
        assert runtime.last_gateway_termination == refused
    finally:
        with suppress(Exception):
            await runtime.shutdown()


def _inert_gateway(directory: Path) -> str:
    """A gateway that exits at once: an inert child for startup failures."""
    script = directory / "inert.sh"
    script.write_text("#!/bin/sh\nexit 0\n")
    script.chmod(0o755)
    return str(script)


class PrivateCloseTransport(PersistentTransport):
    """A host-composed transport whose own private cleanup fails the first
    time: its stop lets the base transport stop the gateway, then closes a
    private channel. Its startup fails once its child exited or, with
    ``spawn`` false, before any child exists."""

    def __init__(
        self,
        gateway_bin: str,
        *,
        startup_error: BaseException,
        close_error: BaseException,
        spawn: bool = True,
    ):
        super().__init__(gateway_bin)
        self.startup_error = startup_error
        self.close_error = close_error
        self.spawn = spawn
        self.closes = 0

    def start(self) -> None:
        if self.spawn:
            super().start()
            self._process.wait(timeout=10)
        raise self.startup_error

    def stop(self) -> None:
        super().stop()
        self.closes += 1
        if self.closes == 1:
            raise self.close_error


@pytest.mark.asyncio
@pytest.mark.parametrize("spawn", [True, False])
async def test_builder_build_keeps_a_private_cleanup_failure_and_only_its_cleanup_retries(
    tmp_path, monkeypatch, spawn
):
    startup_error = RuntimeError("host bootstrap refused")
    close_error = OSError("private channel close failed")
    made = _make_transports(
        monkeypatch,
        lambda index, gateway_bin: PrivateCloseTransport(
            gateway_bin, startup_error=startup_error, close_error=close_error, spawn=spawn
        ),
    )
    with pytest.raises(GatewayStartupCleanupError) as raised:
        await Meta.builder().gateway(_inert_gateway(tmp_path)).build()
    # The process outcome does not attest the transport's own cleanup: its
    # failure reaches the caller with both exact errors.
    failure = raised.value
    assert failure.startup_error is startup_error
    assert failure.__cause__ is startup_error
    assert failure.cleanup_error is close_error
    assert failure.cleanup.last_termination == (
        TransportTermination("exited", 0, None, "none", "not_negotiated")
        if spawn
        else TransportTermination("not_started", None, None, "none", "not_negotiated")
    )
    (transport,) = made
    assert transport.closes == 1

    # Only the returned cleanup retries, and it settles that same owner.
    await failure.cleanup.shutdown()
    assert transport.closes == 2, "the retry ran the same private cleanup"
    await failure.cleanup.shutdown()
    assert transport.closes == 2, "a settled cleanup is not repeated"
    assert len(made) == 1, "no transport was made again"


def _dual_failure(close_error: BaseException) -> tuple[BaseException, BaseException]:
    """A composed stop's two failures, native stop first, as one group whose
    cause is the native failure."""
    native_error = RuntimeError("persistent transport: gateway process did not terminate")
    group = BaseExceptionGroup("gateway stop failed", [native_error, close_error])
    group.__cause__ = native_error
    return native_error, group


@_GROUPS
@pytest.mark.asyncio
@pytest.mark.parametrize(
    "close_error",
    [OSError("private channel close failed"), asyncio.CancelledError()],
    ids=["exception", "cancellation"],
)
async def test_a_failed_stop_group_is_kept_exactly_and_its_cleanup_rejoins(
    tmp_path, monkeypatch, close_error
):
    native_error, group = _dual_failure(close_error)
    startup_error = RuntimeError("host bootstrap refused")
    made = _make_transports(
        monkeypatch,
        lambda index, gateway_bin: PrivateCloseTransport(
            gateway_bin, startup_error=startup_error, close_error=group
        ),
    )
    runtime = MetaRuntime(Meta.builder().gateway(_inert_gateway(tmp_path))._config)
    # A group holding a cancellation is the stop's own failure, not this
    # caller's cancellation: it is kept, and nothing raises TypeError.
    with pytest.raises(GatewayStartupCleanupError) as raised:
        await runtime.connect()
    failure = raised.value
    assert failure.startup_error is startup_error
    assert failure.__cause__ is startup_error
    assert failure.cleanup_error is group
    assert group.exceptions[0] is native_error
    assert group.exceptions[1] is close_error
    assert len(group.exceptions) == 2
    assert group.__cause__ is native_error
    assert asyncio.current_task().cancelling() == 0, "the caller was not cancelled"

    # The same client is rejoined through the returned cleanup.
    (transport,) = made
    assert runtime._transport is transport
    await failure.cleanup.shutdown()
    assert transport.closes == 2
    assert runtime._transport is None
    assert len(made) == 1


class BlockingStopTransport(PersistentTransport):
    """Its startup fails before any child exists; its stop waits for
    ``release``, then settles or, the first time, fails with
    ``close_error``."""

    def __init__(
        self,
        gateway_bin: str,
        startup_error: BaseException,
        close_error: BaseException | None = None,
    ):
        super().__init__(gateway_bin)
        self.startup_error = startup_error
        self.close_error = close_error
        self.entered = threading.Event()
        self.release = threading.Event()
        self.stops = 0

    def start(self) -> None:
        raise self.startup_error

    def stop(self) -> None:
        self.stops += 1
        self.entered.set()
        if not self.release.wait(10):
            raise AssertionError("the test did not release the stop")
        super().stop()
        if self.stops == 1 and self.close_error is not None:
            raise self.close_error


def _start_build(gateway: str) -> tuple[asyncio.Task[MetaRuntime], dict[str, BaseException]]:
    """Run a public build() in a task that keeps the first exception it
    observes, as a caller without a runtime must."""
    observed: dict[str, BaseException] = {}

    async def build() -> MetaRuntime:
        try:
            return await Meta.builder().gateway(gateway).build()
        except BaseException as error:
            observed["error"] = error
            raise

    return asyncio.create_task(build()), observed


async def _until(predicate: Callable[[], bool], timeout: float = 5.0) -> None:
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while not predicate():
        assert loop.time() < deadline, "the condition was not reached"
        await asyncio.sleep(0.01)


async def _reap(made: list[PersistentTransport], build: asyncio.Task | None = None) -> None:
    for transport in made:
        if isinstance(transport, StubbornTransport):
            transport.allow_kill = True
        if isinstance(transport, BlockingStopTransport):
            transport.release.set()
    if build is not None:
        with suppress(BaseException):
            await build
    for transport in made:
        with suppress(Exception):
            await asyncio.to_thread(transport.stop)


class StartupInterrupted(BaseException):
    """A KeyboardInterrupt-like stand-in: neither an Exception nor a
    cancellation."""


@pytest.mark.asyncio
async def test_a_build_cancelled_during_startup_hands_its_cleanup_to_that_cancellation(
    tmp_path, monkeypatch, quick_cleanup
):
    gateway = _write_gateway(tmp_path, "silent", stubborn=True, silent=True)
    made = _make_transports(
        monkeypatch, lambda index, gateway_bin: StubbornTransport(gateway_bin)
    )
    build, observed = _start_build(gateway)
    try:
        await _until(lambda: bool(made) and made[0]._process is not None)
        await asyncio.sleep(0.2)  # meta/init is in flight; nothing answers it
        build.cancel()
        with pytest.raises(asyncio.CancelledError):
            await build
        error = observed["error"]
        assert type(error) is asyncio.CancelledError
        # The stop failed: the runtime keeps the transport and its live child.
        cleanup = error.gateway_cleanup
        assert cleanup.last_termination == _UNSETTLED
        (transport,) = made
        assert transport._process.poll() is None

        transport.allow_kill = True
        await cleanup.shutdown()
        assert cleanup.last_termination == _KILLED
        await cleanup.shutdown()
        assert len(made) == 1, "no gateway was started again"
    finally:
        await _reap(made, build)


@pytest.mark.asyncio
@pytest.mark.parametrize("stop_fails", [False, True], ids=["stop-succeeds", "stop-fails"])
async def test_a_build_cancelled_while_its_cleanup_runs_leaves_that_stop_owned(
    tmp_path, monkeypatch, stop_fails
):
    startup_error = RuntimeError("host bootstrap refused")
    close_error = OSError("private channel close failed")
    made = _make_transports(
        monkeypatch,
        lambda index, gateway_bin: BlockingStopTransport(
            gateway_bin, startup_error, close_error if stop_fails else None
        ),
    )
    build, observed = _start_build(_inert_gateway(tmp_path))
    try:
        await _until(lambda: bool(made) and made[0].entered.is_set())
        (transport,) = made

        # The caller's cancellation is not a stop outcome: it propagates as
        # itself, carrying the cleanup, and no cleanup failure is made up.
        build.cancel()
        with pytest.raises(asyncio.CancelledError):
            await build
        error = observed["error"]
        assert type(error) is asyncio.CancelledError
        assert error.__context__ is startup_error
        cleanup = error.gateway_cleanup
        assert cleanup.last_termination is None, "that stop is still running"

        # The cleanup joins that same pending stop and starts no other.
        transport.release.set()
        if stop_fails:
            with pytest.raises(OSError) as failed:
                await cleanup.shutdown()
            assert failed.value is close_error, "the stop's own failure"
            assert transport.stops == 1
            await cleanup.shutdown()
            assert transport.stops == 2, "the retry settled the same owner"
        else:
            await cleanup.shutdown()
            assert transport.stops == 1
        assert cleanup.last_termination == TransportTermination(
            "not_started", None, None, "none", "not_negotiated"
        )
        assert len(made) == 1
    finally:
        await _reap(made, build)


@pytest.mark.asyncio
async def test_an_interrupted_build_hands_its_cleanup_to_that_exception(
    tmp_path, monkeypatch, quick_cleanup
):
    gateway = _write_gateway(tmp_path, "stubborn", stubborn=True)
    interrupted = StartupInterrupted("the host interrupted the startup")
    made = _make_transports(
        monkeypatch,
        lambda index, gateway_bin: StubbornTransport(gateway_bin, fail_startup=interrupted),
    )
    try:
        with pytest.raises(StartupInterrupted) as raised:
            await Meta.builder().gateway(gateway).build()
        assert raised.value is interrupted
        cleanup = interrupted.gateway_cleanup
        assert cleanup.last_termination == _UNSETTLED

        (transport,) = made
        transport.allow_kill = True
        await cleanup.shutdown()
        assert cleanup.last_termination == _KILLED
        assert len(made) == 1
    finally:
        await _reap(made)


_TIMEOUTS = pytest.mark.skipif(
    sys.version_info < (3, 11),
    reason="the exact TimeoutError cause and asyncio.timeout() need Python 3.11",
)


@_TIMEOUTS
@pytest.mark.asyncio
@pytest.mark.parametrize("limit", ["timeout", "wait_for"])
async def test_a_timed_out_build_still_times_out_and_its_cause_carries_the_cleanup(
    tmp_path, monkeypatch, quick_cleanup, limit
):
    gateway = _write_gateway(tmp_path, "silent", stubborn=True, silent=True)
    made = _make_transports(
        monkeypatch, lambda index, gateway_bin: StubbornTransport(gateway_bin)
    )
    try:
        with pytest.raises(TimeoutError) as raised:
            if limit == "timeout":
                async with asyncio.timeout(0.5):
                    await Meta.builder().gateway(gateway).build()
            else:
                await asyncio.wait_for(Meta.builder().gateway(gateway).build(), 0.5)
        # The cancellation stays a cancellation, so the timeout converts it;
        # the exact cancellation that carried the cleanup is the cause.
        cause = raised.value.__cause__
        assert type(cause) is asyncio.CancelledError
        cleanup = cause.gateway_cleanup
        assert cleanup.last_termination == _UNSETTLED

        (transport,) = made
        transport.allow_kill = True
        await cleanup.shutdown()
        assert cleanup.last_termination == _KILLED
        assert len(made) == 1
    finally:
        await _reap(made)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "interruption",
    [_cancelled, lambda: StartupInterrupted("the host interrupted the startup")],
    ids=["cancelled", "interrupted"],
)
async def test_an_interrupted_build_whose_stop_succeeds_is_left_unchanged(
    tmp_path, monkeypatch, interruption
):
    startup_error = interruption()
    made = _make_transports(
        monkeypatch,
        lambda index, gateway_bin: StubbornTransport(gateway_bin, fail_startup=startup_error),
    )
    try:
        # The plain stand-in exits at stdin EOF: the stop succeeds.
        with pytest.raises(BaseException) as raised:
            await Meta.builder().gateway(_write_gateway(tmp_path, "plain")).build()
        assert raised.value is startup_error
        assert not hasattr(startup_error, "gateway_cleanup")
        (transport,) = made
        assert transport.last_termination == TransportTermination(
            "exited", 0, None, "none", "not_negotiated"
        )
        assert len(made) == 1
    finally:
        await _reap(made)


class Unprintable(Exception):
    """An exception whose ``str()`` raises."""

    def __str__(self) -> str:
        raise RuntimeError("unprintable")


@pytest.mark.asyncio
@pytest.mark.parametrize("unprintable", ["startup", "cleanup"])
async def test_builder_build_keeps_unprintable_failures_and_their_cleanup_settles(
    tmp_path, monkeypatch, unprintable
):
    startup_error = (
        Unprintable() if unprintable == "startup" else RuntimeError("host bootstrap refused")
    )
    close_error = (
        Unprintable() if unprintable == "cleanup" else OSError("private channel close failed")
    )
    made = _make_transports(
        monkeypatch,
        lambda index, gateway_bin: PrivateCloseTransport(
            gateway_bin, startup_error=startup_error, close_error=close_error
        ),
    )
    # An unprintable failure cannot make the typed error itself raise.
    with pytest.raises(GatewayStartupCleanupError) as raised:
        await Meta.builder().gateway(_inert_gateway(tmp_path)).build()
    failure = raised.value
    assert failure.startup_error is startup_error
    assert failure.__cause__ is startup_error
    assert failure.cleanup_error is close_error

    (transport,) = made
    await failure.cleanup.shutdown()
    assert transport.closes == 2, "the same cleanup settled"
    assert len(made) == 1, "no transport was made again"


class RefusesAttributes(BaseException):
    """A non-Exception startup failure whose attribute assignment raises."""

    def __setattr__(self, name: str, value: object) -> None:
        raise RuntimeError(f"{type(self).__name__} accepts no attributes")


@pytest.mark.asyncio
async def test_a_startup_exception_that_refuses_its_cleanup_still_propagates_unchanged(
    tmp_path, monkeypatch, quick_cleanup, caplog
):
    gateway = _write_gateway(tmp_path, "stubborn", stubborn=True)
    startup_error = RefusesAttributes("the host interrupted the startup")
    made = _make_transports(
        monkeypatch,
        lambda index, gateway_bin: StubbornTransport(gateway_bin, fail_startup=startup_error),
    )
    try:
        with pytest.raises(RefusesAttributes) as raised:
            await Meta.builder().gateway(gateway).build()
        # The refusal never replaces the actual startup exception, and nothing
        # claims the handoff happened.
        assert raised.value is startup_error
        assert not hasattr(startup_error, "gateway_cleanup")
        refusals = [
            record
            for record in caplog.records
            if "could not attach gateway_cleanup" in record.getMessage()
        ]
        assert len(refusals) == 1
        assert isinstance(refusals[0].exc_info[1], RuntimeError)
        (transport,) = made
        assert transport.last_termination == _UNSETTLED, "the stop failed; nothing settled"
    finally:
        await _reap(made)


@pytest.mark.asyncio
@pytest.mark.parametrize("stop_fails", [True, False], ids=["fail-then-cancel", "succeed-then-cancel"])
async def test_a_build_cancelled_once_its_stop_completed(
    tmp_path, monkeypatch, caplog, stop_fails
):
    startup_error = RuntimeError("host bootstrap refused")
    close_error = OSError("private channel close failed")

    def released(index: int, gateway_bin: str) -> BlockingStopTransport:
        transport = BlockingStopTransport(
            gateway_bin, startup_error, close_error if stop_fails else None
        )
        transport.release.set()
        return transport

    made = _make_transports(monkeypatch, released)
    tasks: dict[str, asyncio.Task[MetaRuntime]] = {}
    schedule = MetaRuntime._schedule_transport_stop

    def schedule_then_cancel_on_completion(runtime, transport):
        stop_task = schedule(runtime, transport)
        # The acknowledged completion boundary: as soon as the stop task has
        # completed, the build is cancelled, before it observes the outcome.
        stop_task.add_done_callback(lambda done: tasks["build"].cancel())
        return stop_task

    monkeypatch.setattr(
        MetaRuntime, "_schedule_transport_stop", schedule_then_cancel_on_completion
    )
    build, observed = _start_build(_inert_gateway(tmp_path))
    tasks["build"] = build
    try:
        with pytest.raises(asyncio.CancelledError):
            await build
        error = observed["error"]
        assert type(error) is asyncio.CancelledError
        (transport,) = made
        assert transport.stops == 1
        reported = [
            record
            for record in caplog.records
            if "gateway stop failed after its caller was cancelled" in record.getMessage()
        ]
        if stop_fails:
            # The failure that won is reported, not lost with the caller, and
            # the retained transport stays reachable.
            assert len(reported) == 1 and reported[0].exc_info[1] is close_error
            cleanup = error.gateway_cleanup
            await cleanup.shutdown()
            assert transport.stops == 2, "the retry settled the same owner"
            assert cleanup.last_termination == TransportTermination(
                "not_started", None, None, "none", "not_negotiated"
            )
        else:
            # The successful stop won: nothing is retained or handed over.
            assert reported == []
            assert not hasattr(error, "gateway_cleanup")
        assert len(made) == 1
    finally:
        await _reap(made, build)
