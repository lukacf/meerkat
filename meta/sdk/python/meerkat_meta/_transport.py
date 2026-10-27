"""Persistent subprocess transport for Meta JSON-RPC."""
from __future__ import annotations

import asyncio
from concurrent.futures import TimeoutError as FutureTimeoutError
from dataclasses import dataclass
import json
import logging
import math
import os
from signal import Signals
import subprocess
import threading
import time
from typing import Any, Callable, Literal
from uuid import uuid4
import weakref

from .errors import GatewayChildEndedError, TransportError, TransportReaderFailedError

_log = logging.getLogger("meerkat_meta")
_CURRENT_TRANSPORT = object()

# Provider operations are publicly required to finish within 120 seconds.
# Python gives their event-loop coroutine another five seconds of host
# completion margin before cancellation; the stock Rust gateway owns the
# final 130-second wire deadline.
_PROVIDER_CALLBACK_COMPLETION_SECONDS = 125.0

# The shutdown wait when no valid horizon was advertised. The stock gateway
# advertises its own stdio_shutdown_horizon_ms (432_000 ms in 0.9.5) in its
# `accepted` answer to meta/init, before any bootstrap, and the transport
# waits that value instead. This fallback governs only a stop before that
# answer (nothing durable has run yet), an older or custom gateway without
# the shutdown handshake (the stdin EOF protocol), and a handshake gateway
# that advertises no valid horizon. For those gateways it covers two
# 130-second callback windows, runtime event/mob drains, bounded
# RPC/HTTP/stdout phases, and response-delivery/process-reap margin.
# MetaRuntime.gateway_shutdown_horizon_ms is then None. A supervisor that
# sizes its stop from it falls back to its own unreported horizon: Meta
# Forge uses the stock 432_000 ms and widens an earlier systemd unit to fit.
_GATEWAY_SHUTDOWN_GRACE_SECONDS = 337.0
_MAX_GATEWAY_SHUTDOWN_HORIZON_MS = 2_147_483_647
_PROCESS_TERMINATE_GRACE_SECONDS = 5.0
_PROCESS_KILL_GRACE_SECONDS = 5.0
# How long a gateway that exited gets for its stdout to close before its
# waiters fail anyway: a process it started can hold that pipe open.
_CHILD_EXIT_STDOUT_GRACE_SECONDS = 1.0
# The exit watcher blocks in ``Popen.wait()``, so it watches only real
# child processes.
_Popen = subprocess.Popen
_GATEWAY_SHUTDOWN_METHOD = "meta/shutdown"

# meta/init: accepted, then settled (#550). The SDK opts in with these
# params; the gateway answers `accepted` at once, then sends progress
# notifications and exactly one settlement carrying the same `init_id`.
INIT_PROTOCOL_ACCEPTED_THEN_SETTLED = "accepted_then_settled"
_INIT_PROGRESS_METHOD = "meta/init_progress"
_INIT_SETTLED_METHOD = "meta/init_settled"


class InitWatch:
    """Correlates one accepted-then-settled ``meta/init``.

    Registered before the init request is written, so a progress or
    settlement notification can never arrive unclaimed. The reader thread
    delivers into an asyncio future on the host loop, so waiting for the
    settlement is cancellable and holds no thread. Closing the watch retires
    that future, so a failure nobody awaited is never reported by asyncio.
    """

    def __init__(self, init_id: str, loop: asyncio.AbstractEventLoop):
        self.init_id = init_id
        #: The last ``meta/init_progress`` phase seen, for diagnostics.
        self.last_phase: str | None = None
        self._loop = loop
        self._settlement: asyncio.Future[dict[str, Any]] = loop.create_future()

    def _resolve(self, settle: Callable[[asyncio.Future[dict[str, Any]]], None]) -> None:
        def apply() -> None:
            if not self._settlement.done():
                settle(self._settlement)

        try:
            self._loop.call_soon_threadsafe(apply)
        except RuntimeError:
            # The host loop is closed; nobody is waiting any more.
            pass

    def _deliver(self, method: str, params: dict[str, Any]) -> None:
        if method == _INIT_PROGRESS_METHOD:
            phase = params.get("phase")
            if isinstance(phase, str):
                self.last_phase = phase
        elif method == _INIT_SETTLED_METHOD:
            self._resolve(lambda future: future.set_result(params))

    def _fail(self, error: BaseException) -> None:
        self._resolve(lambda future: future.set_exception(error))

    async def settlement(self, deadline: float | None = None) -> dict[str, Any]:
        """Wait for ``meta/init_settled``. ``deadline`` is the caller's own
        cap in seconds (``None``: wait until the gateway settles, exits, or
        the reader fails); exceeding it raises ``asyncio.TimeoutError``.
        Neither the deadline nor a cancelled caller ends the settlement."""
        # asyncio.wait only watches the future. A cancelled shield detaches
        # from it instead, which left a later failure unretrieved (or, on
        # Python 3.14, reported as an exception in a shielded future) before
        # close_init_watch() could retire it.
        waiting = asyncio.wait((self._settlement,))
        if deadline is None:
            await waiting
        else:
            await asyncio.wait_for(waiting, deadline)
        return self._settlement.result()

    def _retire(self) -> None:
        """Retire the settlement future on its owning loop.

        A failure the reader already delivered is read, so asyncio counts it
        as retrieved; a pending future is cancelled, so a delivery still
        queued (the reader's EOF) finds it done and does nothing. Called off
        the loop, it is queued behind every delivery already queued there.
        """
        try:
            on_loop = asyncio.get_running_loop() is self._loop
        except RuntimeError:
            on_loop = False
        if on_loop:
            self._drain()
            return
        try:
            self._loop.call_soon_threadsafe(self._drain)
        except RuntimeError:
            # The loop is closed: no delivery runs any more, and asyncio never
            # reports a pending future. Read a failure already delivered.
            if self._settlement.done() and not self._settlement.cancelled():
                self._settlement.exception()

    def _drain(self) -> None:
        settlement = self._settlement
        if not settlement.done():
            settlement.cancel()
        elif not settlement.cancelled():
            settlement.exception()


class _ChildLifetime:
    """What is known about one gateway child, kept for that child alone.

    The first recorded cause ends the lifetime and later facts never relabel
    it. An observed exit status is recorded when ``poll()`` reports one; a
    channel failure alone leaves it unknown. A waiter selects one lifetime and
    receives that child's record even if a replacement starts meanwhile.
    """

    _EXIT_POLL_SECONDS = 0.05

    def __init__(self, process: Any):
        # Weak: the record outlives its child for waiters, but must not keep a
        # retired process (and its streams) alive once nothing else owns it.
        self._process = weakref.ref(process)
        self._lock = threading.Lock()
        self._ended = threading.Event()
        self._error: GatewayChildEndedError | None = None

    def end(self, reason: str, *, stop_requested: bool = False) -> None:
        with self._lock:
            if self._error is None:
                self._error = GatewayChildEndedError(reason)
            if stop_requested:
                self._error.stop_requested = True
        self._ended.set()

    def observe_exit(self) -> None:
        process = self._process()
        if process is None:
            return
        try:
            exit_code = process.poll()
        except OSError:
            return
        if exit_code is None:
            return
        with self._lock:
            if self._error is None:
                self._error = GatewayChildEndedError("the gateway process exited")
            if self._error.exit_code is None:
                self._error.exit_code = exit_code
        self._ended.set()

    @property
    def error(self) -> GatewayChildEndedError | None:
        self.observe_exit()
        with self._lock:
            return self._error

    def refusal(self) -> GatewayChildEndedError | None:
        """A fresh error for one refused request; the record stays shared."""
        recorded = self.error
        if recorded is None:
            return None
        return GatewayChildEndedError(
            recorded.reason,
            exit_code=recorded.exit_code,
            stop_requested=recorded.stop_requested,
        )

    def wait(self, timeout: float | None = None) -> GatewayChildEndedError | None:
        deadline = None if timeout is None else time.monotonic() + timeout
        while True:
            self.observe_exit()
            step = self._EXIT_POLL_SECONDS
            if deadline is not None:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return self.error
                step = min(step, remaining)
            if self._ended.wait(step):
                return self.error


@dataclass(frozen=True)
class TransportTermination:
    """What a ``stop()`` observed for a transport's gateway process.

    Read it with ``PersistentTransport.last_termination`` or
    ``MetaRuntime.last_gateway_termination``. ``"exited"`` with
    ``exit_code`` 0 and escalation ``"none"`` is the process outcome only: it
    does not prove the gateway's own cleanup (see ``native_shutdown``) or a
    host's private settlement.
    """

    #: ``"exited"`` when the stop observed the process exit, ``"unknown"``
    #: when bounded cleanup ended without an exit and the transport still
    #: owns the process, ``"not_started"`` when no process was started.
    state: Literal["exited", "not_started", "unknown"]
    #: The observed exit status; ``None`` for a signal exit or no observed exit.
    exit_code: int | None
    #: The signal that ended the process, such as ``"SIGKILL"``; ``None`` for
    #: a normal exit or no observed exit.
    signal: str | None
    #: The highest signal any stop attempted against the process. An attempt
    #: is not proof that it caused the exit, nor that the process ended.
    escalation: Literal["none", "sigterm", "sigkill"]
    #: The gateway's own answer to its one shutdown request, apart from the
    #: process outcome: ``"acknowledged"`` (runtime cleanup completed),
    #: ``"retained_until_exit"`` (cleanup completes only with the process
    #: exit), ``"refused"`` (an error or non-attesting answer),
    #: ``"unanswered"`` (no answer: a timeout, a failed reader or a gone
    #: process), or ``"not_negotiated"`` (the gateway offered no shutdown
    #: handshake, so the stop only closed its stdin).
    native_shutdown: Literal[
        "acknowledged", "retained_until_exit", "refused", "unanswered", "not_negotiated"
    ]


_NOT_STARTED = TransportTermination(
    state="not_started",
    exit_code=None,
    signal=None,
    escalation="none",
    native_shutdown="not_negotiated",
)


def _termination_of(
    exit_status: Any, reaped: bool, escalation: Any, native_shutdown: Any
) -> TransportTermination:
    """The record of a stopped process: its observed exit, or ``unknown``."""
    if not reaped:
        return TransportTermination("unknown", None, None, escalation, native_shutdown)
    exit_code: int | None = None
    signal: str | None = None
    if isinstance(exit_status, int) and not isinstance(exit_status, bool):
        if exit_status >= 0:
            exit_code = exit_status
        else:
            # Popen reports a signal exit as the negated signal number.
            try:
                signal = Signals(-exit_status).name
            except ValueError:
                signal = f"signal {-exit_status}"
    return TransportTermination("exited", exit_code, signal, escalation, native_shutdown)


def _polled_exit_status(process: Any) -> Any:
    try:
        return process.poll()
    except OSError:
        return None


class _GatewayShutdownRefusedError(RuntimeError):
    """The gateway answered its shutdown request without attesting cleanup."""


def _sanitize_for_json(obj: Any) -> Any:
    """Recursively sanitize a value so json.dumps won't fail.

    Non-serializable leaves (callables, custom objects) are converted to
    their string representation so the callback response always reaches Rust.
    """
    if obj is None or isinstance(obj, (bool, int, float, str)):
        return obj
    if isinstance(obj, dict):
        return {str(k): _sanitize_for_json(v) for k, v in obj.items()}
    if isinstance(obj, (list, tuple)):
        return [_sanitize_for_json(v) for v in obj]
    # Fall back to string repr for non-serializable objects (e.g. tool callables)
    try:
        json.dumps(obj)
        return obj
    except (TypeError, ValueError):
        return str(obj)


def _contains_non_finite(obj: Any) -> bool:
    if isinstance(obj, float):
        return not math.isfinite(obj)
    if isinstance(obj, dict):
        return any(_contains_non_finite(value) for value in obj.values())
    if isinstance(obj, list):
        return any(_contains_non_finite(value) for value in obj)
    return False


class PersistentTransport:
    """Long-lived meta-rpc subprocess communicating over stdin/stdout JSON-RPC.

    Uses a background reader thread to multiplex responses and callbacks.
    Unlike the per-call subprocess transport, this keeps the process alive
    so mob state persists across calls.

    Gateway stderr (tracing lines, panic hooks, migration progress) is
    INHERITED by default so it reaches the host process's stderr. Override
    with either environment variable:

    - ``META_GATEWAY_STDERR_FILE=<path>``: append gateway stderr to a file.
    - ``META_GATEWAY_STDERR=devnull``: discard gateway stderr (the pre-0.8.9
      default). A production fleet lost a week of panic-hook lines to the old
      silent default; opt out only when the host genuinely cannot carry the
      child's stderr.
    """

    def __init__(
        self,
        gateway_bin: str,
        *,
        env: dict[str, str] | None = None,
        timeout: float = 60.0,
        inherited_fds: tuple[int, ...] = (),
        extra_args: tuple[str, ...] = (),
    ):
        if any(isinstance(fd, bool) or not isinstance(fd, int) or fd < 3 for fd in inherited_fds):
            raise ValueError("inherited_fds must contain open non-stdio descriptors")
        if len(set(inherited_fds)) != len(inherited_fds):
            raise ValueError("inherited_fds must not contain duplicates")
        if inherited_fds and os.name != "posix":
            raise ValueError("inherited descriptors require a POSIX host")
        if any(not isinstance(arg, str) or "\x00" in arg for arg in extra_args):
            raise ValueError("extra_args must be strings without NUL")
        # These are process-local startup capabilities, never serialized in RPC.
        # Their creator retains ownership and closes parent copies after start.
        self._inherited_fds = tuple(inherited_fds)
        self._extra_args = tuple(extra_args)
        self.gateway_bin = gateway_bin
        self._env = {**os.environ, **(env or {})}
        self._process: subprocess.Popen[bytes] | None = None
        self._timeout = timeout
        # Serializes stdin writes line by line. Callback responses take
        # priority: they wait only for the line being written, never behind
        # queued requests, so a gateway callback is not held up by host RPCs.
        self._write_cond = threading.Condition()
        self._writing = False
        self._priority_writers_waiting = 0
        self._pending_lock = threading.Lock()     # protects _pending and _results
        self._pending: dict[str, threading.Event] = {}
        self._results: dict[str, Any] = {}
        self._reader_thread: threading.Thread | None = None
        self._callback_handler: Callable | None = None
        self._loop: asyncio.AbstractEventLoop | None = None
        self._stderr_file = None
        self._supports_shutdown_handshake = False
        self._shutdown_horizon_seconds = _GATEWAY_SHUTDOWN_GRACE_SECONDS
        self._advertised_shutdown_horizon_ms: int | None = None
        # What stops observed for the current child (``last_termination``).
        # The escalation and the shutdown answer belong to that child: a later
        # stop of the same child keeps them and never asks it twice.
        self._last_termination: TransportTermination | None = None
        self._attempted_escalation = "none"
        self._native_shutdown: str | None = None
        self._native_shutdown_error: Exception | None = None
        # Set once the reader for the current gateway process has stopped;
        # every waiter and every later request fails with it.
        self._reader_failure: TransportReaderFailedError | None = None
        # The accepted-then-settled init in flight on this process, if any.
        self._init_watch: InitWatch | None = None
        # Orders the start decision, request admission and stop. Only a
        # never-started, open transport starts its child on demand; after a
        # child existed, replacing it is the caller's explicit start().
        self._lifecycle_lock = threading.Lock()
        self._lifetime: _ChildLifetime | None = None
        self._lifetimes: weakref.WeakKeyDictionary[Any, _ChildLifetime] = (
            weakref.WeakKeyDictionary()
        )
        self._closed = False
        # One start at a time: set while a start creates a child; a concurrent
        # start (explicit or a first send) waits for it. A start publishes its
        # child only if no stop() was recorded since it claimed the slot.
        self._start_claim: threading.Event | None = None
        self._stop_epoch = 0
        # Children a start abandoned (stop() won) but could not reap. They
        # stay owned here until a stop() reaps them; no replacement starts
        # beside them.
        self._unreaped: list[Any] = []

    def set_callback_handler(self, handler: Callable) -> None:
        self._callback_handler = handler

    def open_init_watch(self, init_id: str) -> InitWatch:
        """Register the watch for ``init_id`` before writing ``meta/init``."""
        loop = self._loop if self._loop is not None else asyncio.get_running_loop()
        watch = InitWatch(init_id, loop)
        with self._pending_lock:
            self._init_watch = watch
            failure = getattr(self, "_reader_failure", None)
        if failure is not None:
            watch._fail(failure)
        return watch

    def close_init_watch(self, watch: InitWatch) -> None:
        """Stop watching ``watch`` and retire its settlement future.

        The init caller may have stopped waiting first (its request failed,
        was refused, or timed out) while the reader still failed the watch.
        That exact watch is retired even if a restart already detached it.
        """
        with self._pending_lock:
            if self._init_watch is watch:
                self._init_watch = None
        watch._retire()

    @property
    def request_timeout(self) -> float:
        """Default timeout, in seconds, for one outbound RPC request."""
        return self._timeout

    def start(self) -> None:
        self._start(None)

    def _start(self, expected_stop_epoch: int | None) -> None:
        """Start a child. A first send passes the stop epoch it observed while
        the transport was open; a stop() recorded since refuses the start."""
        claim = self._claim_start(expected_stop_epoch)
        if claim is None:
            return
        stop_epoch, done = claim
        try:
            self._start_claimed(stop_epoch)
        finally:
            with self._lifecycle_lock:
                if self._start_claim is done:
                    self._start_claim = None
            done.set()

    def _claim_start(
        self, expected_stop_epoch: int | None = None
    ) -> tuple[int, threading.Event] | None:
        """Take the start slot, or ``None`` when a usable child already runs."""
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
                    self._unreaped = [
                        child for child in self._unreaped if child.poll() is None
                    ]
                    if self._unreaped:
                        raise TransportError(
                            "persistent transport: a gateway child abandoned during "
                            "stop() is still running; stop() reaps it before a "
                            "replacement starts"
                        )
                    current = self._process
                    if current is not None and current.poll() is None:
                        lifetime = self._lifetime_of(current)
                        if lifetime is None or lifetime.error is None:
                            return None
                        # Its channel ended while the process still runs. A
                        # replacement beside it would leave two children;
                        # stop() reaps it first.
                        raise TransportError(
                            "persistent transport: the gateway child is still running "
                            "after its channel ended; stop() it before starting a "
                            "replacement"
                        )
                    done = threading.Event()
                    self._start_claim = done
                    return self._stop_epoch, done
            pending.wait()

    def _start_claimed(self, stop_epoch: int) -> None:
        # Capture event loop for async callback dispatch
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            loop = None
        # Default: the child gateway INHERITS this process's stderr (None).
        # The old DEVNULL default silently discarded tracing, panic hooks,
        # and migration progress — a week of panic lines went to /dev/null in
        # one production fleet, and a supervisor aborted a deploy because a
        # working migration looked like a hang. Opt out explicitly with
        # META_GATEWAY_STDERR=devnull, or redirect to a file with
        # META_GATEWAY_STDERR_FILE=<path>.
        stderr_target: Any = None
        stderr_file = None
        stderr_path = self._env.get("META_GATEWAY_STDERR_FILE", "").strip()
        if stderr_path:
            stderr_file = open(stderr_path, "ab", buffering=0)
            stderr_target = stderr_file
        elif self._env.get("META_GATEWAY_STDERR", "").strip().lower() == "devnull":
            stderr_target = subprocess.DEVNULL

        try:
            process = subprocess.Popen(
                [self.gateway_bin, "--persistent", *self._extra_args],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=stderr_target,
                env=self._env,
                close_fds=True,
                pass_fds=self._inherited_fds,
            )
        except BaseException:
            # No gateway process was started: there is none to describe.
            self._last_termination = _NOT_STARTED
            raise
        failure = TransportReaderFailedError("the gateway process was replaced")
        events = []
        watch = None
        with self._lifecycle_lock:
            if self._stop_epoch != stop_epoch:
                # stop() was recorded while this child was created: it wins.
                publish = False
            else:
                publish = True
                retired, events, watch = self._publish_child(
                    process, stderr_file, loop, failure
                )
        if not publish:
            self._abandon_unpublished_child(process, stderr_file)
            raise GatewayChildEndedError(
                "the transport was stopped while its gateway child was starting",
                stop_requested=True,
            )
        for event in events:
            event.set()
        if watch is not None:
            watch._fail(failure)
        if retired is not None:
            retired.observe_exit()
            retired.end("the gateway process was replaced")
        reader = threading.Thread(
            target=self._reader_loop, args=(process,), daemon=True, name="meta-reader"
        )
        self._reader_thread = reader
        reader.start()
        if isinstance(process, _Popen):
            threading.Thread(
                target=self._watch_child_exit,
                args=(process, reader),
                daemon=True,
                name="meta-exit-watch",
            ).start()

    def _publish_child(self, process, stderr_file, loop, failure):
        """Make ``process`` the current child. Caller holds the lifecycle lock."""
        events = []
        watch = None
        with self._pending_lock:
            # A caller may restart an exited child before its reader sees EOF.
            # Finish old waiters against their own maps before publishing the
            # replacement. Already received replies keep their exact outcome.
            if self._process is not None or self._reader_failure is not None:
                for msg_id in self._pending:
                    self._results.setdefault(msg_id, failure)
                events = list(self._pending.values())
                watch = self._init_watch
                self._pending = {}
                self._results = {}
                self._init_watch = None
            retired = self._lifetime_of(self._process) if self._process is not None else None
            lifetime = _ChildLifetime(process)
            self._lifetimes[process] = lifetime
            self._lifetime = lifetime
            self._closed = False
            self._process = process
            self._stderr_file = stderr_file
            self._loop = loop
            self._reader_failure = None
            # Capabilities and streams belong to this exact child.
            self._supports_shutdown_handshake = False
            self._shutdown_horizon_seconds = _GATEWAY_SHUTDOWN_GRACE_SECONDS
            self._advertised_shutdown_horizon_ms = None
            # So do the stop record and progress: a new child starts without.
            self._last_termination = None
            self._attempted_escalation = "none"
            self._native_shutdown = None
            self._native_shutdown_error = None
        return retired, events, watch

    def _abandon_unpublished_child(self, process, stderr_file) -> None:
        """Reap a child that was created but never published (stop won).

        It never received a request; closing its stdin asks it to exit, and a
        bounded terminate/kill follows. A child that cannot be reaped stays
        reported, never silently dropped.
        """
        reaped = False
        try:
            reaped = self._reap_bounded(process)
        finally:
            if not reaped:
                # Keep custody before any fallible cleanup: stop() retries
                # and reports it, and no replacement starts beside it.
                with self._lifecycle_lock:
                    self._unreaped.append(process)
        close_error: Exception | None = None
        for stream in (process.stdout, stderr_file):
            if stream is None:
                continue
            try:
                stream.close()
            except (OSError, ValueError) as exc:
                close_error = close_error or exc
        if not reaped:
            raise TransportError(
                "persistent transport: a gateway child started during stop() did "
                "not terminate after bounded cleanup"
            ) from close_error
        if close_error is not None:
            _log.warning(
                "transport: closing an abandoned gateway child's streams failed: %s",
                close_error,
            )

    @staticmethod
    def _reap_bounded(process) -> bool:
        """Close stdin, then wait, terminate and kill with bounded graces."""
        if process.stdin:
            try:
                process.stdin.close()
            except OSError:
                pass
        for signal_child, grace in (
            (None, _PROCESS_TERMINATE_GRACE_SECONDS),
            (process.terminate, _PROCESS_TERMINATE_GRACE_SECONDS),
            (process.kill, _PROCESS_KILL_GRACE_SECONDS),
        ):
            if signal_child is not None:
                try:
                    signal_child()
                except OSError:
                    pass
            try:
                process.wait(timeout=grace)
                return True
            except (OSError, subprocess.TimeoutExpired):
                continue
        try:
            return process.poll() is not None
        except OSError:
            return False

    def _reader_loop(self, process=_CURRENT_TRANSPORT) -> None:
        if process is _CURRENT_TRANSPORT:
            process = self._process
        assert process is not None and process.stdout is not None
        reason = "the gateway closed its stdout"
        try:
            while True:
                line = process.stdout.readline()
                if not line or process is not self._process:
                    break
                self._read_line(line, process=process)
        except Exception as exc:  # noqa: BLE001 - every exit must fail waiters
            reason = f"reader failed: {exc!r}"
            _log.error("transport: reader stopped: %s", reason, exc_info=True)
        finally:
            self._fail_pending_after_reader_exit(reason, process=process)

    def _read_line(self, line: bytes, *, process=_CURRENT_TRANSPORT) -> None:
        """Handle one stdout line. A bad line is skipped, never fatal."""
        if process is _CURRENT_TRANSPORT:
            process = self._process
        if process is not self._process:
            return
        try:
            msg = json.loads(line.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError):
            _log.warning("transport: skipping non-JSON line from subprocess: %r", line[:200])
            return
        if not isinstance(msg, dict):
            _log.warning(
                "transport: skipping JSON line that is not an object: %r", line[:200]
            )
            return

        if "method" in msg:
            method = msg.get("method")
            if "id" not in msg and method in (_INIT_PROGRESS_METHOD, _INIT_SETTLED_METHOD):
                self._deliver_init_event(method, msg.get("params"), process=process)
                return
            # Callback or notification FROM Rust
            self._handle_callback(msg, process=process)
        elif "id" in msg:
            # Response to a pending request
            msg_id = str(msg["id"])
            with self._pending_lock:
                if process is not self._process:
                    return
                event = self._pending.get(msg_id)
                if event is not None:
                    self._results[msg_id] = msg
            if event is not None:
                event.set()
            else:
                # The caller may already have timed out and removed its
                # pending entry. Do not retain an unclaimable late result.
                _log.debug(
                    "transport: dropping response for non-pending id=%s",
                    msg_id,
                )
        else:
            _log.warning(
                "transport: unrecognized message (no id or method): %s",
                str(msg)[:200],
            )

    def _deliver_init_event(self, method: str, params: Any, *, process=_CURRENT_TRANSPORT) -> None:
        if process is _CURRENT_TRANSPORT:
            process = self._process
        if not isinstance(params, dict):
            _log.warning("transport: %s without object params; ignored", method)
            return
        with self._pending_lock:
            if process is not self._process:
                return
            watch = self._init_watch
        if watch is None or params.get("init_id") != watch.init_id:
            # Not the init this process is running (a late or foreign line).
            _log.debug("transport: %s for an init that is not in flight; ignored", method)
            return
        watch._deliver(method, params)

    def _watch_child_exit(self, process, reader: threading.Thread) -> None:
        """Fail the waiters of a child that exited while its stdout stays open.

        The reader fails them once stdout reaches EOF, which normally follows
        the exit at once. A process the gateway started can inherit its
        stdout and hold it open; its waiters would then wait out their
        timeouts, the shutdown handshake its whole horizon. After a short
        grace they fail with the child's ``GatewayChildEndedError`` instead.
        If the reader thread does not run for that whole second (a loaded
        host), a reply already in the pipe fails with the exit instead of
        being delivered: as with any reader failure, that request's outcome
        is then unknown.
        """
        try:
            process.wait()
        except Exception:  # noqa: BLE001 - the watcher must never raise
            return
        reader.join(_CHILD_EXIT_STDOUT_GRACE_SECONDS)
        if not reader.is_alive():
            return
        reason = "the gateway process exited while a process it started still held its stdout"
        lifetime = self._lifetime_of(process)
        failure = None
        if lifetime is not None:
            lifetime.end(reason)
            lifetime.observe_exit()
            failure = lifetime.refusal()
        self._fail_pending_after_reader_exit(
            reason,
            process=process,
            failure=failure or GatewayChildEndedError(reason, exit_code=process.returncode),
        )

    def _fail_pending_after_reader_exit(
        self,
        reason: str,
        *,
        process=_CURRENT_TRANSPORT,
        failure: TransportReaderFailedError | None = None,
    ) -> None:
        """No response can arrive any more: fail every waiter, typed.

        A response that arrived before the reader stopped (for example a
        fail-closed init error written just before the gateway exits) is
        kept for its waiter.
        """
        if process is _CURRENT_TRANSPORT:
            process = self._process
        # The reader's own child, even after a replacement: its end is that
        # child's record and never the successor's.
        lifetime = self._lifetime_of(process) if process is not None else None
        if lifetime is not None:
            lifetime.end(reason)
            lifetime.observe_exit()
        if failure is None:
            failure = TransportReaderFailedError(reason)
        with self._pending_lock:
            if process is not self._process:
                return
            self._reader_failure = failure
            for msg_id in self._pending:
                self._results.setdefault(msg_id, failure)
            events = list(self._pending.values())
            watch = getattr(self, "_init_watch", None)
        for event in events:
            event.set()
        if watch is not None:
            watch._fail(failure)

    def _handle_callback(self, msg: dict, *, process=_CURRENT_TRANSPORT) -> None:
        """Dispatch callback in a separate thread so the reader loop is not blocked."""
        if process is _CURRENT_TRANSPORT:
            process = self._process
        with self._pending_lock:
            if process is not self._process:
                return
            handler, loop = self._callback_handler, self._loop
        if handler is None:
            method = msg.get("method")
            _log.warning(
                "transport: received callback but no handler registered: %s",
                method,
            )
            callback_id = msg.get("id")
            if callback_id is not None:
                # Answer now: the gateway would otherwise wait out its full
                # callback deadline for a response that never comes.
                try:
                    self._write_line(
                        {
                            "jsonrpc": "2.0",
                            "id": callback_id,
                            "error": {
                                "code": -32000,
                                "message": f"no callback handler is registered for {method}",
                                "data": {
                                    "kind": "callback_handler_unavailable",
                                    "method": method,
                                },
                            },
                        },
                        callback_response=True,
                        process=process,
                    )
                except Exception:
                    _log.error(
                        "failed to send callback error response for id=%s", callback_id
                    )
            return
        # Dispatch in a daemon thread to avoid blocking the reader loop
        t = threading.Thread(
            target=self._dispatch_callback, args=(msg,), daemon=True,
            kwargs={"process": process, "handler": handler, "loop": loop},
            name="meta-callback",
        )
        t.start()

    def _dispatch_callback(
        self, msg: dict, *, process=_CURRENT_TRANSPORT,
        handler=_CURRENT_TRANSPORT, loop=_CURRENT_TRANSPORT,
    ) -> None:
        if process is _CURRENT_TRANSPORT:
            process = self._process
        if handler is _CURRENT_TRANSPORT:
            handler = self._callback_handler
        if loop is _CURRENT_TRANSPORT:
            loop = self._loop
        if process is not self._process:
            return
        method = msg.get("method", "")
        params = msg.get("params", {})
        callback_id = msg.get("id")  # None for notifications
        try:
            if loop is not None and loop.is_running():
                async def invoke_current_handler():
                    # Scheduling on the captured loop can outlive the child.
                    # Check again before any host handler body starts.
                    if process is not self._process:
                        return None
                    return await handler(method, params)

                future = asyncio.run_coroutine_threadsafe(
                    invoke_current_handler(), loop
                )
                try:
                    result = future.result(
                        timeout=_PROVIDER_CALLBACK_COMPLETION_SECONDS
                    )
                except FutureTimeoutError as exc:
                    # `Future.result()` also propagates a TimeoutError raised
                    # by the provider itself. A completed future therefore
                    # represents the provider's own failure, not exhaustion
                    # of the host wait, and must retain its original error.
                    if future.done():
                        raise
                    # The ordinary request timeout is intentionally unrelated
                    # to provider execution. If the dedicated host deadline is
                    # exhausted, cancel the event-loop coroutine before Rust's
                    # 130-second wire deadline so a timed-out provider cannot
                    # continue toward a late authority commit unnoticed.
                    future.cancel()
                    raise TimeoutError(
                        "provider callback exceeded its 125s host completion deadline"
                    ) from exc
            else:
                raise RuntimeError(
                    "PersistentTransport: no running event loop for callback dispatch"
                )
            # Notifications (no id) are fire-and-forget — no response sent
            if callback_id is None:
                return
            # Ensure result is JSON-serializable before building response.
            # Tools or other callback results may contain non-serializable objects;
            # sanitize them to strings to prevent json.dumps failures in _write_line.
            response = {"jsonrpc": "2.0", "id": callback_id, "result": _sanitize_for_json(result)}
            if _contains_non_finite(response["result"]):
                # JSON has no NaN or Infinity. Sending the token would make
                # the gateway drop the line and wait out its deadline;
                # sending null would change the provider's answer. Refuse it.
                self._write_line(
                    {
                        "jsonrpc": "2.0",
                        "id": callback_id,
                        "error": {
                            "code": -32000,
                            "message": (
                                f"{method} returned a non-finite number (NaN or "
                                "Infinity), which JSON cannot carry"
                            ),
                            "data": {"kind": "non_finite_result"},
                        },
                    },
                    callback_response=True,
                    process=process,
                )
                return
            self._write_line(response, callback_response=True, process=process)
        except Exception as exc:
            # Notifications: log only, don't try to send error response
            if callback_id is None:
                _log.warning("notification dispatch error (%s): %s", method, exc)
                return
            _log.warning("callback dispatch error: %s", exc)
            error_response = {
                "jsonrpc": "2.0",
                "id": callback_id,
                "error": {"code": -32000, "message": str(exc)},
            }
            try:
                self._write_line(error_response, callback_response=True, process=process)
            except Exception:
                _log.error("failed to send callback error response for id=%s", callback_id)

    def _write_line(
        self, obj: dict, *, callback_response: bool = False,
        process=_CURRENT_TRANSPORT,
    ) -> None:
        """Write one JSON line to the gateway's stdin.

        Lines never interleave. A callback response (``callback_response``)
        waits only for the line currently being written; queued requests wait
        until no callback response is waiting. One pipe cannot do better: a
        line already being written must finish first.
        """
        if process is _CURRENT_TRANSPORT:
            process = self._process
        # Strict JSON, encoded before taking a turn: the gateway cannot parse
        # NaN/Infinity tokens, and a refusal must not hold the pipe.
        data = (json.dumps(obj, allow_nan=False) + "\n").encode("utf-8")
        with self._write_cond:
            if callback_response:
                self._priority_writers_waiting += 1
            try:
                while self._writing or (
                    not callback_response and self._priority_writers_waiting
                ):
                    self._write_cond.wait()
            finally:
                if callback_response:
                    self._priority_writers_waiting -= 1
            self._writing = True
        try:
            # A callback may finish, or wait for the writer, across restart.
            # Its reply belongs only to the child that issued the invocation.
            if process is not self._process:
                return
            if process and process.stdin:
                process.stdin.write(data)
                process.stdin.flush()
        finally:
            with self._write_cond:
                self._writing = False
                self._write_cond.notify_all()

    def send_sync(
        self,
        request: dict[str, Any],
        *,
        timeout: float | None = None,
    ) -> Any:
        self._ensure_running()
        with self._pending_lock:
            process = self._process
        response = self._send_sync_running(request, timeout=timeout, process=process)
        if request.get("method") == "meta/init":
            result = response.get("result") if isinstance(response, dict) else None
            supports_shutdown_handshake = bool(
                isinstance(result, dict)
                and result.get("stdio_shutdown_handshake") is True
            )
            shutdown_horizon = _GATEWAY_SHUTDOWN_GRACE_SECONDS
            advertised_horizon_ms = None
            if supports_shutdown_handshake and isinstance(result, dict):
                horizon_ms = result.get("stdio_shutdown_horizon_ms")
                if (
                    isinstance(horizon_ms, int)
                    and not isinstance(horizon_ms, bool)
                    and 0 < horizon_ms <= _MAX_GATEWAY_SHUTDOWN_HORIZON_MS
                ):
                    shutdown_horizon = horizon_ms / 1000.0
                    advertised_horizon_ms = horizon_ms
            with self._pending_lock:
                if process is self._process:
                    self._supports_shutdown_handshake = supports_shutdown_handshake
                    self._shutdown_horizon_seconds = shutdown_horizon
                    self._advertised_shutdown_horizon_ms = advertised_horizon_ms
        return response

    def _send_sync_running(
        self,
        request: dict[str, Any],
        *,
        timeout: float | None = None,
        process=_CURRENT_TRANSPORT,
    ) -> Any:
        """Send on the current child without starting a replacement process."""
        request_timeout = self._timeout if timeout is None else timeout
        # ``Event.wait`` refuses a timeout past ``threading.TIMEOUT_MAX`` with
        # OverflowError, which would surface only after the request is written:
        # the gateway would run it with nobody waiting. Refuse it here instead.
        if (
            isinstance(request_timeout, bool)
            or not isinstance(request_timeout, (int, float))
            or not math.isfinite(request_timeout)
            or request_timeout <= 0
            or request_timeout > threading.TIMEOUT_MAX
        ):
            raise ValueError(
                "persistent transport: timeout must be a positive finite number "
                f"of seconds, at most threading.TIMEOUT_MAX ({threading.TIMEOUT_MAX:g})"
            )
        # Pre-fix, requests with no `id` (or two callers using the
        # same id) collided on `self._pending[""]`: the second
        # `_pending[msg_id] = event` clobbered the first caller's
        # Event, blocking it for the full timeout. Reject either
        # condition explicitly so the deadlock surfaces as a clear
        # ValueError at the call site.
        raw_id = request.get("id")
        if raw_id is None or (isinstance(raw_id, str) and not raw_id):
            raise ValueError(
                "persistent transport: request must carry a non-empty `id` "
                "(use uuid4 or similar) — empty/missing ids collide on the "
                "in-flight pending map and deadlock concurrent callers"
            )
        msg_id = str(raw_id)
        event = threading.Event()
        with self._pending_lock:
            if process is _CURRENT_TRANSPORT:
                process = self._process
            elif process is not self._process:
                raise TransportReaderFailedError("the gateway process was replaced")
            reader_failure = getattr(self, "_reader_failure", None)
            if reader_failure is not None:
                raise reader_failure
            pending, results = self._pending, self._results
            if msg_id in pending:
                raise ValueError(
                    f"persistent transport: request id {msg_id!r} is already "
                    f"in flight; concurrent callers must use distinct ids"
                )
            pending[msg_id] = event
        try:
            # Admission, write, response and cleanup belong to this child.
            self._write_line(request, process=process)
            if not event.wait(timeout=request_timeout):
                raise RuntimeError(
                    f"persistent transport: timeout after {request_timeout}s "
                    "waiting for response"
                )
            with self._pending_lock:
                result = results.get(msg_id)
        finally:
            with self._pending_lock:
                pending.pop(msg_id, None)
                results.pop(msg_id, None)
        if isinstance(result, TransportReaderFailedError):
            raise result
        if result is None:
            raise RuntimeError("persistent transport: subprocess closed stdout")
        return result

    def _request_gateway_shutdown(
        self,
        process: subprocess.Popen[bytes],
        *,
        timeout: float,
    ) -> str:
        """Wait for runtime cleanup while the current child's stdin stays open.

        Returns the gateway's attesting answer, ``"acknowledged"`` or
        ``"retained_until_exit"``. An answer that does not attest cleanup
        raises ``_GatewayShutdownRefusedError``; no answer raises otherwise.
        """
        if self._process is not process or process.poll() is not None:
            raise RuntimeError("gateway exited before shutdown handshake")
        response = self._send_sync_running(
            {
                "jsonrpc": "2.0",
                "id": f"meta-shutdown-{uuid4()}",
                "method": _GATEWAY_SHUTDOWN_METHOD,
                "params": {},
            },
            timeout=timeout,
            process=process,
        )
        if not isinstance(response, dict):
            raise _GatewayShutdownRefusedError(
                "gateway shutdown returned a malformed response"
            )
        error = response.get("error")
        if isinstance(error, dict):
            message = error.get("message", "unknown gateway error")
            raise _GatewayShutdownRefusedError(f"gateway shutdown failed: {message}")
        result = response.get("result")
        if not isinstance(result, dict):
            raise _GatewayShutdownRefusedError(
                "gateway shutdown did not complete runtime-owned cleanup"
            )
        if (
            result.get("shutdown") is True
            and result.get("runtime_cleanup_completed") is True
        ):
            return "acknowledged"
        if (
            result.get("runtime_cleanup_completed") is False
            and result.get("runtime_cleanup_retained_until_exit") is True
        ):
            # Cleanup is still incomplete, but the only cause is durable child
            # mobs held until the gateway process exits; reaping it (next,
            # with its own failure path) releases them. Not a failed phase.
            _log.info(
                "gateway shutdown: durable child mobs held until the gateway exits"
            )
            return "retained_until_exit"
        raise _GatewayShutdownRefusedError(
            "gateway shutdown did not complete runtime-owned cleanup"
        )

    async def send_async(
        self,
        request: dict[str, Any],
        *,
        timeout: float | None = None,
    ) -> Any:
        return await asyncio.to_thread(self.send_sync, request, timeout=timeout)

    def stop(self) -> None:
        lifecycle_lock = getattr(self, "_lifecycle_lock", None)
        if lifecycle_lock is not None:
            # Close ordinary admission first; the shutdown handshake and
            # callback replies below use the internal path and stay open.
            with lifecycle_lock:
                self._closed = True
                self._stop_epoch += 1
                start_claim = self._start_claim
                stopping = self._lifetime_of(getattr(self, "_process", None))
                if stopping is not None:
                    stopping.end("the transport was stopped", stop_requested=True)
            if start_claim is not None:
                # A start in flight sees this stop and reaps its own child.
                start_claim.wait()
        reaped_abandoned, unreaped_error = self._reap_abandoned_children()
        with self._pending_lock:
            process = getattr(self, "_process", None)
            stderr_file = getattr(self, "_stderr_file", None)
            supports_shutdown_handshake = getattr(self, "_supports_shutdown_handshake", False)
            shutdown_horizon = getattr(
                self,
                "_shutdown_horizon_seconds",
                _GATEWAY_SHUTDOWN_GRACE_SECONDS,
            )
        if process is None:
            # No current child. A child a start abandoned is described while
            # it still runs and once reaped (both after an attempted kill); a
            # real record of an earlier child otherwise stays.
            if unreaped_error is not None:
                self._last_termination = _termination_of(
                    None, False, "sigkill", "not_negotiated"
                )
                raise unreaped_error
            if reaped_abandoned:
                self._last_termination = _termination_of(
                    _polled_exit_status(reaped_abandoned[-1]),
                    True,
                    "sigkill",
                    "not_negotiated",
                )
            elif getattr(self, "_last_termination", None) is None:
                self._last_termination = _NOT_STARTED
            return
        shutdown_error: Exception | None = None
        process_reaped = False
        reap_error: Exception | None = None
        # What this stop observes of ``process``: its exit status, plus the
        # progress the child keeps across stops (the highest signal attempted
        # and the answer to its one shutdown request).
        exit_status: Any = None
        escalation = getattr(self, "_attempted_escalation", "none")
        # An earlier stop of this child already gave it the whole horizon and
        # escalated; a later stop goes straight to the signals.
        grace_spent = escalation != "none"
        native_shutdown = getattr(self, "_native_shutdown", None)
        if native_shutdown is not None:
            # A later stop of this child only finishes reaping it, and reports
            # that answer as the stop which reaped it would have.
            shutdown_error = getattr(self, "_native_shutdown_error", None)
        elif not supports_shutdown_handshake:
            native_shutdown = "not_negotiated"
        shutdown_started = time.monotonic()
        try:
            # The gateway may need to round-trip host callbacks while
            # UnifiedRuntime shuts down. Keep stdin open until a capable
            # gateway acknowledges that cleanup is complete. Older or custom
            # gateways stay on the EOF protocol below.
            if native_shutdown is None:
                native_shutdown = "unanswered"
                try:
                    native_shutdown = self._request_gateway_shutdown(
                        process,
                        timeout=shutdown_horizon,
                    )
                except Exception as exc:
                    if isinstance(exc, _GatewayShutdownRefusedError):
                        native_shutdown = "refused"
                    _log.debug("gateway shutdown handshake failed: %s", exc)
                    shutdown_error = exc
                with self._pending_lock:
                    if self._process is process:
                        self._native_shutdown = native_shutdown
                        self._native_shutdown_error = shutdown_error

            elapsed = time.monotonic() - shutdown_started
            remaining_grace = 0.0 if grace_spent else max(0.0, shutdown_horizon - elapsed)
            if process.stdin:
                try:
                    process.stdin.close()
                except OSError:
                    # A gateway that already closed its pipe still needs to be
                    # reaped below.
                    pass
            try:
                exit_status = process.wait(timeout=remaining_grace)
                process_reaped = True
            except (OSError, subprocess.TimeoutExpired):
                try:
                    exit_status = process.poll()
                    process_reaped = exit_status is not None
                except OSError:
                    process_reaped = False
                if not process_reaped:
                    if escalation == "none":
                        escalation = "sigterm"
                    try:
                        process.terminate()
                    except OSError:
                        # The child can exit between the timed wait and signal.
                        pass
                    try:
                        exit_status = process.wait(timeout=_PROCESS_TERMINATE_GRACE_SECONDS)
                        process_reaped = True
                    except (OSError, subprocess.TimeoutExpired):
                        try:
                            exit_status = process.poll()
                            process_reaped = exit_status is not None
                        except OSError:
                            process_reaped = False
                        if not process_reaped:
                            escalation = "sigkill"
                            try:
                                process.kill()
                            except OSError:
                                pass
                            try:
                                exit_status = process.wait(timeout=_PROCESS_KILL_GRACE_SECONDS)
                                process_reaped = True
                            except (OSError, subprocess.TimeoutExpired) as exc:
                                try:
                                    exit_status = process.poll()
                                    process_reaped = exit_status is not None
                                except OSError:
                                    process_reaped = False
                                if not process_reaped:
                                    # Keep teardown bounded, but never attest
                                    # success or discard ownership while the
                                    # child remains live.
                                    reap_error = exc
        finally:
            with self._pending_lock:
                if self._process is process:
                    # Read from this exact child before it is released or
                    # kept. A child a start abandoned that still runs leaves
                    # the outcome unknown as well.
                    self._attempted_escalation = escalation
                    if unreaped_error is None:
                        self._last_termination = _termination_of(
                            exit_status, process_reaped, escalation, native_shutdown
                        )
                    else:
                        self._last_termination = _termination_of(
                            None, False, "sigkill", native_shutdown
                        )
            if process_reaped:
                self._fail_pending_after_reader_exit(
                    "the gateway process stopped", process=process,
                )
                with self._pending_lock:
                    if self._process is process:
                        self._process = None
                    if self._stderr_file is stderr_file:
                        self._stderr_file = None
                if stderr_file is not None:
                    stderr_file.close()
        if reap_error is not None:
            raise RuntimeError(
                "persistent transport: gateway process did not terminate after bounded cleanup"
            ) from reap_error
        if unreaped_error is not None:
            raise unreaped_error
        if shutdown_error is not None:
            raise RuntimeError(
                "persistent transport: gateway shutdown failed after bounded cleanup"
            ) from shutdown_error

    def _reap_abandoned_children(self) -> tuple[list[Any], RuntimeError | None]:
        """Retry children a start abandoned but could not reap.

        Returns the children reaped now, and an error while any still runs.
        """
        lifecycle_lock = getattr(self, "_lifecycle_lock", None)
        if lifecycle_lock is None:
            return [], None
        with lifecycle_lock:
            abandoned = list(self._unreaped)
        survivors = [child for child in abandoned if not self._reap_bounded(child)]
        with lifecycle_lock:
            self._unreaped = [
                child
                for child in self._unreaped
                if child not in abandoned or child in survivors
            ]
        reaped = [child for child in abandoned if child not in survivors]
        if survivors:
            return reaped, RuntimeError(
                "persistent transport: a gateway child abandoned during stop() did "
                "not terminate after bounded cleanup"
            )
        return reaped, None

    def is_running(self) -> bool:
        """Whether the current child is alive and can still take requests."""
        process = self._process
        if process is None or process.poll() is not None:
            return False
        if getattr(self, "_closed", False):
            return False
        lifetime = self._lifetime_of(process)
        return lifetime is None or lifetime.error is None

    @property
    def last_termination(self) -> TransportTermination | None:
        """What the most recent completed ``stop()`` observed.

        Describes the current or last gateway process (see
        :class:`TransportTermination`): its exit, the escalation attempted
        and the gateway's own shutdown answer, read from that process before
        it is released or kept. ``None`` while the current process runs or
        its stop has not finished; a new process never inherits an earlier
        one's record, and a stop with nothing to stop never overwrites one.
        """
        return getattr(self, "_last_termination", None)

    @property
    def advertised_shutdown_horizon_ms(self) -> int | None:
        """The shutdown horizon the current gateway advertised and was accepted.

        Milliseconds, from the ``stdio_shutdown_horizon_ms`` of a ``meta/init``
        that also advertised the shutdown handshake. ``None`` before that
        ``meta/init``, or when the gateway advertised no valid horizon and
        this transport waits its own default. A new process never inherits
        an earlier one's value.
        """
        return getattr(self, "_advertised_shutdown_horizon_ms", None)

    @property
    def child_ended(self) -> GatewayChildEndedError | None:
        """The current child's end, or ``None`` while it can take requests.

        Refers to the most recently started child; ``None`` as well before
        any child started.
        """
        lifetime = getattr(self, "_lifetime", None)
        return None if lifetime is None else lifetime.error

    def wait_child_ended(
        self, timeout: float | None = None
    ) -> GatewayChildEndedError | None:
        """Wait until the child current at this call ends; return its record.

        The waiter keeps that child's record across a later replacement.
        Returns ``None`` on timeout or when no child has started.
        """
        lifetime = getattr(self, "_lifetime", None)
        return None if lifetime is None else lifetime.wait(timeout)

    def _lifetime_of(self, process: Any) -> _ChildLifetime | None:
        if process is None:
            return None
        lifetimes = getattr(self, "_lifetimes", None)
        if lifetimes is None:
            return None
        try:
            return lifetimes.get(process)
        except TypeError:
            return None

    def _ensure_running(self) -> None:
        """Admit an ordinary request; never replace a child that existed.

        A never-started, open transport starts its child on first use. The
        decision is made under the lifecycle lock; the start itself runs
        outside it, so stop() is never blocked behind process creation.
        """
        while True:
            with self._lifecycle_lock:
                lifetime = self._lifetime
                if self._closed:
                    ended = None if lifetime is None else lifetime.refusal()
                    raise ended or GatewayChildEndedError(
                        "the transport was stopped", stop_requested=True
                    )
                if lifetime is not None:
                    ended = lifetime.refusal()
                    if ended is not None:
                        raise ended
                    return
                if self._process is not None:
                    return
                observed_stop_epoch = self._stop_epoch
            # A never-started, open transport starts on first use. The start
            # serializes concurrent first sends and is refused if a stop()
            # is recorded after this decision, even before the claim.
            self._start(observed_stop_epoch)

    def __del__(self) -> None:
        try:
            self.stop()
        except Exception:
            # Explicit shutdown surfaces handshake failures. Destructors run
            # outside a caller-owned error channel and must stay best effort.
            pass


def create_persistent_transport(gateway_bin: str, **kwargs: Any) -> PersistentTransport:
    transport = PersistentTransport(gateway_bin, **kwargs)
    transport.start()
    return transport
