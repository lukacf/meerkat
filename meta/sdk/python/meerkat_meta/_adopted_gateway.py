"""Adopt a gateway child that the host started and keeps.

A host that supervises the gateway process itself (it spawns the process and
owns its lifetime, credentials and cleanup) hands the SDK that exact child:
its stdin and stdout pipes, a non-blocking retirement request, and a bounded
wait for the child's own exit. ``AdoptedGatewayTransport`` speaks the stock
stdio protocol over those pipes. It never spawns, signals, reconnects to or
replays a gateway, and it never awaits the host's own cleanup.
"""
from __future__ import annotations

import asyncio
import concurrent.futures
from dataclasses import dataclass
import io
import logging
import math
import subprocess
import threading
import time
from typing import Any, BinaryIO, Callable, Literal

from ._transport import (
    _CHILD_EXIT_STDOUT_GRACE_SECONDS,
    _CURRENT_TRANSPORT,
    _NOT_STARTED,
    _PROCESS_KILL_GRACE_SECONDS,
    _PROCESS_TERMINATE_GRACE_SECONDS,
    PersistentTransport,
    _termination_of,
)
from .errors import (
    GatewayChildEndedError,
    GatewayChildPendingError,
    GatewayTransportConflictError,
    TransportError,
    TransportReaderFailedError,
)

_log = logging.getLogger("meerkat_meta")

# ``wait_process`` takes a finite timeout of 0 to 60 seconds.
_WAIT_PROCESS_MAX_SECONDS = 60.0
# A stop that finds the exit still pending after an earlier stop used its
# whole bound observes it again for this long: the stock stop's TERM and KILL
# graces together.
_PENDING_RETRY_SECONDS = _PROCESS_TERMINATE_GRACE_SECONDS + _PROCESS_KILL_GRACE_SECONDS
# A ``wait_process`` that reports "pending" before its timeout ran out is
# called again no sooner than this, so a stop never spins on the host.
_EARLY_PENDING_PAUSE_SECONDS = 0.05
# What ``wait_process`` raises while the exit is still pending. Python 3.10
# keeps the asyncio and concurrent.futures timeouts apart from TimeoutError,
# and a host that wraps ``Popen.wait`` raises ``TimeoutExpired``.
_EXIT_PENDING = (
    TimeoutError,
    asyncio.TimeoutError,
    concurrent.futures.TimeoutError,
    subprocess.TimeoutExpired,
)


@dataclass(frozen=True)
class AdoptedGatewayChild:
    """A gateway child process the host started and keeps, for the SDK to adopt.

    The host spawned the gateway (``rpc_gateway --persistent``) with piped
    stdin and stdout, and it keeps that process: its lifetime, credentials
    and cleanup. It hands the SDK exactly this child:

    - ``input``: a binary stream that writes the child's stdin: a raw one,
      such as ``os.fdopen(fd, "wb", buffering=0)`` over the write end of the
      child's stdin pipe, or a buffered one, such as the host's
      ``Popen.stdin``.
    - ``output``: a binary stream that reads the child's stdout: a raw one,
      such as ``os.fdopen(fd, "rb", buffering=0)`` over the read end of the
      child's stdout pipe, or a buffered one, such as ``Popen.stdout``.
    - ``request_retirement()``: schedules the host's retirement of the child
      on the host's own event loop and returns at once. It is thread-safe,
      repeat calls refer to the same retirement, and it never calls back into
      the SDK.
    - ``wait_process(timeout)``: waits at most ``timeout`` seconds (finite,
      from 0 to 60) for the direct child to exit and returns its exit code (a
      negative code names a signal, as ``subprocess.Popen.returncode`` does).
      It raises ``TimeoutError`` while the exit is still pending, which is
      not a failure. The SDK calls it only on its own worker threads, never
      on an event-loop thread, and never from two threads at once. It never
      waits for the SDK's reader, the child's descendants or the host's own
      cleanup.

    Construction checks these shapes only: it reads, writes and closes
    nothing. The pipes are handed over when the transport starts (see
    ``AdoptedGatewayTransport``), which buffers a raw stream itself: every
    byte of each line is written, lines are read in blocks, and interrupted
    or partial reads and writes are retried. The transport closes each end
    once, through the object it was given; the host must not ``os.close()``
    a descriptor it handed over inside such an object, as the descriptor's
    number could already belong to another file by then.
    """

    input: BinaryIO
    output: BinaryIO
    request_retirement: Callable[[], None]
    wait_process: Callable[[float], int]

    def __post_init__(self) -> None:
        _check_stream("input", self.input, ("write", "flush", "close"), "writable")
        _check_stream("output", self.output, ("readline", "close"), "readable")
        if self.input is self.output:
            raise ValueError(
                "input and output must be the child's two pipes, not one stream"
            )
        for name in ("request_retirement", "wait_process"):
            if not callable(getattr(self, name)):
                raise TypeError(f"{name} must be callable")


def _check_stream(
    name: str, stream: Any, methods: tuple[str, ...], mode: str
) -> None:
    """Refuse a stream the transport cannot use, without using it."""
    if isinstance(stream, io.TextIOBase):
        raise TypeError(f"{name} must be a binary stream, not a text stream")
    for method in methods:
        if not callable(getattr(stream, method, None)):
            raise TypeError(f"{name} must be a binary stream with {method}()")
    if getattr(stream, "closed", False) is True:
        raise ValueError(f"{name} is already closed")
    check = getattr(stream, mode, None)
    if callable(check) and check() is False:
        raise ValueError(f"{name} is not {mode}")


@dataclass(frozen=True)
class GatewayStartFailure:
    """The native terminal failure of a start that failed after the hand-off.

    It stays apart from the direct child's exit
    (``AdoptedGatewayTransport.child_exit``) and the SDK reader's completion
    (``AdoptedGatewayTransport.reader_completed``): a failed init proves
    neither, and neither proves how init ended.
    """

    #: The startup error exactly as raised to the caller: the typed
    #: ``RpcError`` of a refused or failed init, ``InitOutcomeUnknownError``,
    #: a transport error, or the ``CancelledError`` of a cancelled caller.
    error: BaseException
    #: What is known of ``meta/init``: ``"not_sent"`` (the start failed
    #: before the request was handed to the transport), ``"refused"`` (the
    #: gateway answered it with an error), ``"failed"`` (it was accepted and
    #: then settled failed; the error's ``data["durable_effects"]`` says what
    #: may have changed), ``"unknown"`` (the request was handed over and no
    #: outcome was learned, for example a lost answer, a deadline or a
    #: cancelled caller: native state may have changed), or ``"ready"`` (it
    #: settled ready and a later step of the start failed).
    init_outcome: Literal["not_sent", "refused", "failed", "unknown", "ready"]


@dataclass(frozen=True)
class GatewayChildExit:
    """The adopted direct child's own exit, as ``wait_process`` reported it."""

    #: ``"exited"`` once ``wait_process`` returned an exit code; ``"pending"``
    #: when a stop's bounded wait ended first, or ``wait_process`` failed.
    state: Literal["exited", "pending"]
    #: The exit code ``wait_process`` returned; ``None`` while pending.
    exit_code: int | None


class _AdoptedProcess:
    """The process view the shared transport code drives for an adopted child.

    ``poll()`` and ``returncode`` report only an exit the transport observed
    through ``wait_process`` (its exit watcher, or a stop): they never call
    the host and never block. It has no ``wait()``, ``terminate()`` or
    ``kill()``: the transport never waits on, signals or kills a process it
    does not own.
    """

    def __init__(self, stdin: Any, stdout: Any) -> None:
        self.stdin = stdin
        self.stdout = stdout
        self._exit_code: int | None = None

    def poll(self) -> int | None:
        return self._exit_code

    @property
    def returncode(self) -> int | None:
        return self._exit_code


def _handed_over_streams(child: AdoptedGatewayChild) -> tuple[Any, Any]:
    """The streams the transport writes and reads the adopted child through.

    A raw stream (``os.fdopen(fd, "wb", buffering=0)`` and ``"rb"``) is
    wrapped. A raw write can take fewer bytes than it was offered, and a raw
    ``readline()`` reads one byte per call: a ``BufferedWriter`` writes every
    byte of a line before ``flush()`` returns, a ``BufferedReader`` reads
    lines in blocks, and both retry interrupted and partial reads and writes.
    Closing a wrapper closes its raw stream, once, at the transport's own
    close points (``input`` at stop, ``output`` when the reader ends). A
    buffered stream, such as ``Popen.stdin`` or ``Popen.stdout``, is used as
    it is.

    Called only once the hand-off is decided. If the second wrapper cannot
    be made, the first is detached, never closed: nothing was handed over.
    """
    stdin = child.input
    if isinstance(stdin, io.RawIOBase):
        stdin = io.BufferedWriter(stdin)
    try:
        stdout = child.output
        if isinstance(stdout, io.RawIOBase):
            stdout = io.BufferedReader(stdout)
    except BaseException:
        if stdin is not child.input:
            stdin.detach()
        raise
    return stdin, stdout


class AdoptedGatewayTransport(PersistentTransport):
    """The stock stdio gateway transport over a child the host started and keeps.

    It speaks the stock protocol over the adopted pipes: the ``meta/init``
    exchange with its accepted-then-settled readiness, host callbacks (a
    handler set before ``start()`` answers the first one), sync and async
    RPC with their timeouts, correlated init watches, and the
    ``meta/shutdown`` handshake. It never spawns, signals, reconnects to or
    replays a gateway.

    Ownership:

    - Construction validates and keeps the ``AdoptedGatewayChild``. Only
      ``start()`` hands the child over (a request before it is refused): from
      then on the transport writes ``input`` and reads ``output``. A refusal
      before that (a wrong carrier, a transport already started or stopped,
      a stop that came first) leaves both pipes untouched with the host.
    - Call ``start()`` on the event loop that dispatches host callbacks, as
      for the stock transport. A second ``start()`` while the child serves
      does nothing; after a stop it is refused.
    - From the hand-off, one exit watcher thread is the only caller of
      ``wait_process``, in waits of at most 60 seconds, until the child
      exits or a stop returns. Once the child exited, a reader still blocked
      on a stdout that a descendant holds open fails its waiters about a
      second later, as the stock transport does. A watcher whose
      ``wait_process`` fails logs it and ends; the next stop observes the
      child itself, and until then an exit fails no waiter early.
    - ``stop()`` is synchronous and blocking. Run it on a worker thread, for
      example ``await asyncio.to_thread(transport.stop)``; on an event-loop
      thread it raises at once and does nothing. It makes the stock protocol
      shutdown (the one ``meta/shutdown`` request when the gateway negotiated
      it, then the end of ``input``) on a helper thread, bounded by the
      shutdown horizon, so a writer stuck on a full pipe cannot hold it; if
      that phase times out, the helper and the stuck writer stay blocked
      until the child's stdin closes, normally through the retirement.
      Then, whatever that phase did, it calls ``request_retirement()``, once
      per child however often it runs, and waits for the child's exit as
      the watcher observes it (after a stop that returned pending, later
      stops call ``wait_process`` themselves). It waits for the exit before
      it joins the SDK's reader, so a descendant that holds the child's
      stdout cannot use up that bound; once the child exited, the reader
      gets a short grace to reach the end of ``output``. It never awaits,
      models or reports the host's own cleanup.
    - ``MetaRuntime`` stops a start that fails after the hand-off the same
      way, on a runtime-owned worker task the caller's cancellation does not
      end, after it records the failure as ``start_failure``.
    - The SDK's reader closes ``output`` when it ends; ``stop()`` closes
      ``input``. For a raw stream that is its buffering wrapper, which closes
      the raw stream once; nothing else of the host is closed. The reader
      never calls into the host.
    - Always stop the transport (``MetaRuntime.shutdown()``). A transport
      dropped without a stop ends its child's input and calls
      ``request_retirement()`` from its finalizer, on whatever thread frees
      it (the reader's included) or in the collection at interpreter exit.

    Three outcomes stay apart: the native terminal failure
    (``start_failure``, and ``last_termination.native_shutdown`` for the
    answer to the shutdown request), the direct child's exit (``child_exit``,
    also recorded in ``last_termination``), and the SDK reader's completion
    (``reader_completed``).
    """

    def __init__(self, child: AdoptedGatewayChild, *, timeout: float = 60.0):
        if not isinstance(child, AdoptedGatewayChild):
            raise TypeError(
                "AdoptedGatewayTransport takes an AdoptedGatewayChild, got "
                f"{type(child).__name__}"
            )
        if (
            isinstance(timeout, bool)
            or not isinstance(timeout, (int, float))
            or not math.isfinite(timeout)
            or timeout <= 0
            or timeout > threading.TIMEOUT_MAX
        ):
            raise ValueError("timeout must be a positive finite number of seconds")
        super().__init__("", timeout=timeout)
        self._child = child
        # Set once, at the hand-off: the transport never takes another child.
        self._handed_over = False
        # Serializes stops: a later stop finds what an earlier one did.
        self._stop_lock = threading.Lock()
        self._retirement_lock = threading.Lock()
        self._retirement_requested = False
        self._start_failure: GatewayStartFailure | None = None
        # The last stop's record of a pending exit; the exit code itself, once
        # observed, is ``_exit_code``.
        self._child_exit: GatewayChildExit | None = None
        # The child's exit as observed, and who observes it: the exit watcher
        # while it runs, a stop otherwise. Never two callers of wait_process.
        self._observation = threading.Condition()
        self._exit_code: int | None = None
        self._watcher_running = False
        self._stop_returned = False
        # When the first stop began: its bounds run from then.
        self._first_stop_at: float | None = None
        # The one protocol shutdown of this child, and what it found.
        self._protocol_phase: threading.Thread | None = None
        self._protocol_outcome: dict[str, Any] = {}
        self._reader_done = threading.Event()

    @property
    def child(self) -> AdoptedGatewayChild:
        """The adopted child this transport was given."""
        return self._child

    @property
    def start_failure(self) -> GatewayStartFailure | None:
        """The native terminal failure of a start that failed after the hand-off.

        ``None`` while no start failed after the hand-off; a refusal before
        it leaves the child with the host and records nothing. Set by
        ``MetaRuntime`` before it stops the transport; the first failure is
        kept.
        """
        return self._start_failure

    @property
    def child_exit(self) -> GatewayChildExit | None:
        """The direct child's exit as ``wait_process`` reported it.

        ``"exited"`` with its code once the exit watcher or a stop observed
        it; ``"pending"`` when a stop's bound ended first; ``None`` before
        either. It is the child's own exit only: it says nothing about its
        descendants or the host's own cleanup.
        """
        with self._observation:
            if self._exit_code is not None:
                return GatewayChildExit("exited", self._exit_code)
            return self._child_exit

    @property
    def reader_completed(self) -> bool:
        """Whether the SDK's reader of ``output`` has ended.

        It ends at the end of the child's stdout or on a read failure, and
        closes ``output``. ``False`` before the hand-off and while a
        descendant of the child still holds that stdout open.
        """
        return self._reader_done.is_set()

    def _record_start_failure(self, failure: GatewayStartFailure) -> None:
        with self._pending_lock:
            if self._start_failure is None:
                self._start_failure = failure

    def _ensure_running(self) -> None:
        """Admit a request only once ``start()`` handed the child over.

        The stock transport starts its own child on a first request. An
        adopted child is handed over by ``start()`` alone, on the event loop
        that dispatches its callbacks, before ``meta/init``; a request never
        takes it.
        """
        with self._lifecycle_lock:
            if not self._handed_over and not self._closed:
                raise TransportError(
                    "an adopted gateway child is handed over by start() (connect()), "
                    "never by a request"
                )
        super()._ensure_running()

    def _claim_start(
        self, expected_stop_epoch: int | None = None
    ) -> tuple[int, threading.Event] | None:
        """Take the one hand-off, or ``None`` when the adopted child is in use."""
        while True:
            with self._lifecycle_lock:
                if expected_stop_epoch is not None and (
                    self._closed or self._stop_epoch != expected_stop_epoch
                ):
                    raise GatewayChildEndedError(
                        "the transport was stopped", stop_requested=True
                    )
                pending = self._start_claim
                if pending is None:
                    if self._handed_over:
                        current = self._process
                        if current is not None and not self._closed:
                            lifetime = self._lifetime_of(current)
                            if lifetime is None or lifetime.error is None:
                                return None
                        raise GatewayTransportConflictError(
                            "an adopted gateway transport takes its child once: it "
                            "never starts, reconnects to or replays another gateway "
                            "child"
                        )
                    if self._closed:
                        raise GatewayTransportConflictError(
                            "this adopted gateway transport was stopped before its "
                            "child was handed over; the host keeps the child and its "
                            "pipes"
                        )
                    done = threading.Event()
                    self._start_claim = done
                    return self._stop_epoch, done
            pending.wait()

    def _start_claimed(self, stop_epoch: int) -> None:
        # Capture event loop for async callback dispatch.
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            loop = None
        child = self._child
        with self._lifecycle_lock:
            if self._closed or self._stop_epoch != stop_epoch:
                # stop() came first: nothing was handed over, and the host
                # keeps the child and its pipes untouched.
                raise GatewayChildEndedError(
                    "the transport was stopped before its adopted gateway child "
                    "was handed over",
                    stop_requested=True,
                )
            for name in ("input", "output"):
                if getattr(getattr(child, name), "closed", False) is True:
                    raise TransportError(
                        f"the adopted gateway child's {name} was closed before the "
                        "hand-off"
                    )
            # The hand-off: from here the transport writes the child's stdin
            # and reads its stdout. The first and only child, so nothing is
            # retired and no waiter is failed. The exit watcher counts as
            # running before the child is published, so a stop that finds the
            # child never calls wait_process beside it.
            process = _AdoptedProcess(*_handed_over_streams(child))
            self._handed_over = True
            with self._observation:
                self._watcher_running = True
            self._publish_child(
                process,
                None,
                loop,
                TransportReaderFailedError("the gateway process was replaced"),
            )
        watcher_started = False
        try:
            reader = threading.Thread(
                target=self._reader_loop,
                args=(process,),
                daemon=True,
                name="meta-reader",
            )
            reader.start()
            self._reader_thread = reader
            watcher = threading.Thread(
                target=self._watch_adopted_child_exit,
                args=(process, reader),
                daemon=True,
                name="meta-exit-watch",
            )
            watcher.start()
            watcher_started = True
        except BaseException:
            if not watcher_started:
                # No watcher: a stop observes the child itself, and one
                # already waiting for the watcher wakes now.
                with self._observation:
                    self._watcher_running = False
                    self._observation.notify_all()
            # The child was handed over: its host still retires it.
            self._request_retirement_once()
            raise

    def _watch_adopted_child_exit(
        self, process: _AdoptedProcess, reader: threading.Thread
    ) -> None:
        """Observe the child's exit from the hand-off: the one wait_process caller.

        Waits of at most 60 seconds each, until the child exits or a stop
        returns; after that a stop calls ``wait_process`` itself. Once the
        child exited, its waiters fail about a second later if the reader is
        still blocked on a stdout that a descendant holds open, as the stock
        transport's watcher does.
        """
        exited = False
        try:
            while True:
                with self._observation:
                    if self._exit_code is not None:
                        exited = True
                        break
                    if self._stop_returned:
                        break
                try:
                    exit_code = self._wait_process_once(_WAIT_PROCESS_MAX_SECONDS)
                except Exception as exc:  # noqa: BLE001 - a stop observes again
                    _log.warning("transport: wait_process() failed: %s", exc)
                    break
                if exit_code is not None:
                    self._record_child_exit(process, exit_code)
                    exited = True
                    break
        finally:
            with self._observation:
                self._watcher_running = False
                self._observation.notify_all()
        if exited:
            self._fail_waiters_after_exit(process, reader)

    def _reader_loop(self, process: Any = _CURRENT_TRANSPORT) -> None:
        if process is _CURRENT_TRANSPORT:
            process = self._process
        try:
            super()._reader_loop(process)
        finally:
            # The reader is the only user of the child's stdout: it releases
            # it and reports its own completion. It never calls the host.
            stdout = getattr(process, "stdout", None)
            if stdout is not None:
                try:
                    stdout.close()
                except (OSError, ValueError):
                    pass
            self._reader_done.set()

    def stop(self) -> None:
        """End the protocol with the adopted child and hand the child back.

        Synchronous and blocking: run it on a worker thread. On an event-loop
        thread it raises ``TransportError`` at once and does nothing, so it
        never holds the loop that dispatches host callbacks or runs the
        host's retirement.

        It closes ordinary admission and makes the stock protocol shutdown:
        the one ``meta/shutdown`` request when the gateway negotiated it, then
        the end of ``input``, on a helper thread that it waits for only until
        the shutdown horizon ends (from when the first stop began). Then,
        whatever that phase did, it calls ``request_retirement()``, once per
        child, and waits for the direct child's exit until that horizon plus
        the stock stop's TERM and KILL graces (a later stop: those graces
        again). While the exit watcher runs, it waits for what the watcher
        observes; after a stop returned, a later stop calls ``wait_process``
        itself, at most 60 seconds at a time. ``TimeoutError`` from
        ``wait_process`` means pending.

        Raises ``GatewayChildPendingError`` while the exit was not observed:
        the transport keeps the child's observation, and a later stop
        observes it again without requesting the retirement twice. Raises
        ``TransportError`` when ``request_retirement()`` failed: the
        transport keeps the child, and a later stop requests it again. Raises
        ``RuntimeError`` when the protocol shutdown failed or did not end in
        time, as the stock transport does for its shutdown request. A stop
        after the exit was observed and the retirement requested returns at
        once.
        """
        try:
            asyncio.get_running_loop()
        except RuntimeError:
            pass
        else:
            raise TransportError(
                "AdoptedGatewayTransport.stop() blocks while it observes the child "
                "through wait_process: run it on a worker thread (for example with "
                "asyncio.to_thread), never on an event-loop thread"
            )
        with self._stop_lock:
            self._stop_adopted_child()

    def _stop_adopted_child(self) -> None:
        # Close ordinary admission first; the shutdown request and callback
        # replies use the internal path and stay open.
        with self._lifecycle_lock:
            self._closed = True
            self._stop_epoch += 1
            process = self._process
            stopping = self._lifetime_of(process)
            if stopping is not None:
                stopping.end("the transport was stopped", stop_requested=True)
        if process is None:
            # Nothing was handed over, or an earlier stop observed the exit:
            # that stop's record stays.
            if self._last_termination is None:
                self._last_termination = _NOT_STARTED
            return
        with self._pending_lock:
            supports_shutdown_handshake = self._supports_shutdown_handshake
            shutdown_horizon = self._shutdown_horizon_seconds
        if self._first_stop_at is None:
            self._first_stop_at = time.monotonic()
        horizon_ends = self._first_stop_at + shutdown_horizon
        native_shutdown = "unanswered"
        protocol_error: BaseException | None = None
        retirement_error: Exception | None = None
        observation_error: Exception | None = None
        exit_code: int | None = None
        try:
            try:
                native_shutdown, protocol_error = self._shut_down_protocol(
                    process, supports_shutdown_handshake, shutdown_horizon, horizon_ends
                )
            finally:
                # Whatever the protocol phase did, the host retires the child;
                # its end also frees a writer stuck on the child's stdin.
                retirement_error = self._request_retirement_once()
            deadline = max(
                horizon_ends + _PENDING_RETRY_SECONDS,
                time.monotonic() + _PENDING_RETRY_SECONDS,
            )
            try:
                exit_code = self._observe_child_exit(process, deadline)
            except Exception as exc:
                observation_error = exc
            if exit_code is not None:
                self._join_reader(_CHILD_EXIT_STDOUT_GRACE_SECONDS)
        finally:
            with self._observation:
                # Once a stop returned, the exit watcher stops waiting, and a
                # later stop observes the child itself.
                self._stop_returned = True
                self._observation.notify_all()
            exited = exit_code is not None
            child_exit = GatewayChildExit(
                "exited" if exited else "pending", exit_code
            )
            with self._pending_lock:
                if self._process is process:
                    self._child_exit = child_exit
                    self._last_termination = _termination_of(
                        exit_code, exited, "none", native_shutdown
                    )
            if exited:
                # The child can answer nothing more.
                self._fail_pending_after_reader_exit(
                    "the gateway process stopped", process=process
                )
                # Released only once its retirement was requested: until
                # then a later stop requests it again.
                with self._pending_lock:
                    if self._process is process and self._retirement_requested:
                        self._process = None
        if not exited:
            raise GatewayChildPendingError(child_exit) from observation_error
        if retirement_error is not None:
            raise TransportError(
                "persistent transport: the host's request_retirement() failed; a "
                "later stop() requests it again"
            ) from retirement_error
        if protocol_error is not None:
            raise RuntimeError(
                "persistent transport: gateway shutdown failed after bounded cleanup"
            ) from protocol_error

    def _shut_down_protocol(
        self,
        process: _AdoptedProcess,
        supports_shutdown_handshake: bool,
        shutdown_horizon: float,
        horizon_ends: float,
    ) -> tuple[str, BaseException | None]:
        """The stock protocol shutdown of this child, bounded by its horizon.

        The one ``meta/shutdown`` request when the gateway negotiated it,
        then the end of ``input``, once per child, on a helper thread. A stop
        waits for it only until ``horizon_ends``: a writer stuck on the
        child's full stdin, which the request and the close of a buffered
        ``input`` queue behind, cannot hold the stop past it. Returns the
        gateway's answer to the request and the phase's failure, if any.
        """
        phase = self._protocol_phase
        outcome = self._protocol_outcome
        if phase is None:
            native_shutdown, shutdown_error = self._recorded_shutdown_answer(
                supports_shutdown_handshake
            )
            outcome["native_shutdown"] = native_shutdown or "unanswered"
            outcome["error"] = shutdown_error

            def shut_down() -> None:
                try:
                    if native_shutdown is None:
                        answer, error = self._ask_gateway_shutdown(
                            process, shutdown_horizon
                        )
                        outcome["native_shutdown"], outcome["error"] = answer, error
                    self._close_gateway_input(process)
                except BaseException as exc:  # noqa: BLE001 - the phase's own outcome
                    outcome["error"] = outcome["error"] or exc

            phase = threading.Thread(
                target=shut_down, daemon=True, name="meta-stop-protocol"
            )
            phase.start()
            self._protocol_phase = phase
        phase.join(max(0.0, horizon_ends - time.monotonic()))
        if phase.is_alive():
            return outcome["native_shutdown"], TimeoutError(
                "the protocol shutdown did not end within the shutdown horizon"
            )
        return outcome["native_shutdown"], outcome["error"]

    def _request_retirement_once(self) -> Exception | None:
        """Ask the host to retire the child, once per child. Never blocks."""
        with self._retirement_lock:
            if self._retirement_requested:
                return None
            try:
                self._child.request_retirement()
            except Exception as exc:
                # Not requested: a later stop asks again.
                _log.warning("transport: request_retirement() failed: %s", exc)
                return exc
            self._retirement_requested = True
            return None

    def _observe_child_exit(self, process: _AdoptedProcess, deadline: float) -> int | None:
        """The child's exit code by ``deadline``, or ``None`` while pending.

        While the exit watcher runs, waits for what it observes. Otherwise
        calls ``wait_process`` on this worker thread, at most 60 seconds at a
        time, until the deadline: never beside the watcher.
        """
        while True:
            with self._observation:
                while self._exit_code is None and self._watcher_running:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        return None
                    self._observation.wait(remaining)
                if self._exit_code is not None:
                    return self._exit_code
            remaining = max(0.0, deadline - time.monotonic())
            exit_code = self._wait_process_once(min(_WAIT_PROCESS_MAX_SECONDS, remaining))
            if exit_code is not None:
                self._record_child_exit(process, exit_code)
                return exit_code
            if time.monotonic() >= deadline:
                return None

    def _wait_process_once(self, timeout: float) -> int | None:
        """One ``wait_process`` call: the exit code, or ``None`` while pending.

        A failure other than a timeout is raised. A "pending" that came back
        before ``timeout`` ran out is followed by a short pause, so no caller
        spins on the host.
        """
        called = time.monotonic()
        try:
            exit_code = self._child.wait_process(timeout)
        except _EXIT_PENDING:
            early = timeout - (time.monotonic() - called)
            if early > 0:
                time.sleep(min(_EARLY_PENDING_PAUSE_SECONDS, early))
            return None
        if isinstance(exit_code, bool) or not isinstance(exit_code, int):
            raise TypeError(
                "wait_process must return the child's exit code as an int, got "
                f"{type(exit_code).__name__}"
            )
        return exit_code

    def _record_child_exit(self, process: _AdoptedProcess, exit_code: int) -> None:
        with self._observation:
            if self._exit_code is None:
                self._exit_code = exit_code
                process._exit_code = exit_code
            self._observation.notify_all()
        lifetime = self._lifetime_of(process)
        if lifetime is not None:
            lifetime.observe_exit()

    def _join_reader(self, timeout: float) -> None:
        reader = self._reader_thread
        if reader is not None and reader is not threading.current_thread():
            reader.join(timeout)

    def __del__(self) -> None:
        # A destructor never blocks or waits, and it may run on any thread,
        # the reader's included, or in the collection at interpreter exit,
        # where starting a thread can hang. A child that no stop handed back
        # gets the end of its input (the transport's own stream, flushed after
        # every line) and is handed back by calling request_retirement()
        # directly: the host keeps it thread-safe and non-blocking, and it
        # never calls back into the SDK.
        try:
            if not getattr(self, "_handed_over", False):
                return
            process = getattr(self, "_process", None)
            if process is not None:
                self._close_gateway_input(process)
            self._request_retirement_once()
        except Exception:
            pass
