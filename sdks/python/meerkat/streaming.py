"""Streaming API for the Meerkat Python SDK.

Provides :class:`_StdoutDispatcher` (internal) for multiplexing the rkat-rpc
stdout stream, and :class:`EventStream` (public) for consuming typed agent
events in real-time.
"""

from __future__ import annotations

import asyncio
import json
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, AsyncIterator, Callable

from .consent import CONSENT_NOTICE_NOTIFICATION
from .event_envelope import parse_agent_event_envelope
from .errors import (
    COMMS_SEND_ERROR_CODES,
    EventSubscriptionError,
    MeerkatError,
    meerkat_error_from_jsonrpc_code,
)
from .events import Event, StreamTruncated, TextDelta
from .tools import ToolCallDedupeKey

if TYPE_CHECKING:
    from .session import Session
    from .types import RunResult

RPC_STDOUT_LIMIT_BYTES = 64 * 1024 * 1024

# Bytes of the child's most recent stderr kept for the CONNECTION_CLOSED error.
STDERR_TAIL_LIMIT_BYTES = 16 * 1024

# How long the stdout read loop waits for the stderr pipe to reach EOF after
# stdout closed, so a startup refusal written just before exit lands in the
# tail. Both pipes close when the child exits, so this is normally instant.
STDERR_TAIL_SETTLE_SECS = 1.0

_STDERR_READ_CHUNK_BYTES = 4096


class _StderrTail:
    """Continuously drain a child's stderr into a bounded tail buffer.

    rkat-rpc is spawned with ``stderr=PIPE``. A pipe nobody reads fills at the
    OS limit (64 KB on Linux) and a chatty child under ``RUST_LOG`` then blocks
    on its next ``eprintln!``, so the drain must run for the child's whole
    lifetime. Only the last ``limit`` bytes are retained: enough for the
    startup refusal or panic that explains an unexpected exit, never unbounded.
    """

    def __init__(
        self,
        stderr: asyncio.StreamReader,
        limit: int = STDERR_TAIL_LIMIT_BYTES,
    ):
        self._stderr = stderr
        self._limit = limit
        self._buffer = bytearray()
        self._task: asyncio.Task[None] | None = None
        self._eof = asyncio.Event()

    def start(self) -> None:
        self._task = asyncio.get_running_loop().create_task(self._drain())

    async def stop(self) -> None:
        """Cancel and await the drain task so it never outlives the client."""
        if self._task is None:
            return
        task = self._task
        self._task = None
        if not task.done():
            task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass

    async def wait_closed(self, timeout: float) -> None:
        """Wait up to ``timeout`` seconds for stderr to reach EOF."""
        try:
            await asyncio.wait_for(self._eof.wait(), timeout)
        except asyncio.TimeoutError:
            pass

    def text(self) -> str:
        """The retained tail, decoded leniently (a chunk boundary may split a
        multi-byte character)."""
        return self._buffer.decode("utf-8", errors="replace")

    def _append(self, chunk: bytes) -> None:
        self._buffer.extend(chunk)
        overflow = len(self._buffer) - self._limit
        if overflow > 0:
            del self._buffer[:overflow]

    async def _drain(self) -> None:
        try:
            while True:
                chunk = await self._stderr.read(_STDERR_READ_CHUNK_BYTES)
                if not chunk:
                    return
                self._append(chunk)
        finally:
            self._eof.set()


@dataclass(frozen=True)
class _SubscriptionEvent:
    """Keep RPC delivery order separate from the canonical event envelope."""

    event: dict[str, Any]
    rpc_sequence: int | None


@dataclass(frozen=True)
class _SubscriptionEnd:
    terminal_outcome: dict[str, Any]


_SubscriptionItem = _SubscriptionEvent | _SubscriptionEnd | None


class _StdoutDispatcher:
    """Background reader that multiplexes stdout lines to response futures and event queues."""

    def __init__(self, stdout: asyncio.StreamReader):
        self._stdout = stdout
        self._stderr_tail: _StderrTail | None = None
        self._pending_responses: dict[int, asyncio.Future[dict[str, Any]]] = {}
        self._event_queues: dict[str, asyncio.Queue[dict[str, Any] | None]] = {}
        self._stream_queues: dict[str, asyncio.Queue[_SubscriptionItem]] = {}
        self._pending_stream_queues: dict[int, asyncio.Queue[dict[str, Any] | None]] = {}
        self._unmatched_buffer: dict[str, list[dict[str, Any]]] = {}
        self._unmatched_stream_buffer: dict[str, list[_SubscriptionEvent]] = {}
        self._stream_terminal: dict[str, dict[str, Any]] = {}
        self._unmatched_stream_end: set[str] = set()
        self._task: asyncio.Task[None] | None = None
        self._closed = False
        self._tool_handler: Any = None  # ToolRegistry, set lazily
        # Channel-scoped ``live/*`` notifications (no session_id) go here,
        # never into a session event queue.
        self._live_notification_sink: Callable[[str, Any], None] | None = None
        # ``approval/consent_notice`` (no session_id) goes here, never into a
        # session event queue.
        self._consent_notice_sink: Callable[[Any], None] | None = None
        self._stdin_writer: asyncio.StreamWriter | None = None  # set on connect
        # Permanent transport-failed state: once the read loop classifies the
        # stream as untrustworthy (corrupted frame) or closed, later requests
        # must fail immediately with this recorded typed fault — never register
        # a future that no read loop will ever resolve.
        self.transport_fault: MeerkatError | None = None

    def start(self) -> None:
        self._task = asyncio.get_running_loop().create_task(self._read_loop())

    async def wait_read_loop_done(self, timeout: float) -> bool:
        """Wait up to ``timeout`` seconds for the read loop to end; whether it did."""
        task = self._task
        if task is None:
            return True
        done, _ = await asyncio.wait({task}, timeout=timeout)
        return bool(done)

    async def stop(self) -> None:
        self._closed = True
        if self._task:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass
        self._fail_all("CLIENT_CLOSED", "client stopped")

    def expect_response(self, request_id: int) -> asyncio.Future[dict[str, Any]]:
        future: asyncio.Future[dict[str, Any]] = asyncio.get_running_loop().create_future()
        self._pending_responses[request_id] = future
        return future

    def subscribe_events(self, session_id: str) -> asyncio.Queue[dict[str, Any] | None]:
        queue: asyncio.Queue[dict[str, Any] | None] = asyncio.Queue()
        self._event_queues[session_id] = queue
        return queue

    def unsubscribe_events(self, session_id: str) -> None:
        self._event_queues.pop(session_id, None)

    def subscribe_stream(self, stream_id: str) -> asyncio.Queue[_SubscriptionItem]:
        queue: asyncio.Queue[_SubscriptionItem] = asyncio.Queue()
        if self.transport_fault is None:
            self._stream_queues[stream_id] = queue
        for event in self._unmatched_stream_buffer.pop(stream_id, []):
            queue.put_nowait(event)
        if stream_id in self._unmatched_stream_end:
            self._unmatched_stream_end.discard(stream_id)
            queue.put_nowait(_SubscriptionEnd(self._stream_terminal[stream_id]))
        elif self.transport_fault is not None:
            # An accepted open can resume after EOF. Drain any received prefix
            # and then end, just as an already-registered queue does on EOF.
            queue.put_nowait(None)
        return queue

    def unsubscribe_stream(self, stream_id: str) -> None:
        self._stream_queues.pop(stream_id, None)

    def stream_terminal(self, stream_id: str) -> dict[str, Any] | None:
        return self._stream_terminal.get(stream_id)

    def subscribe_pending_stream(self, request_id: int) -> asyncio.Queue[dict[str, Any] | None]:
        # Per-request_id subscription: concurrent streaming creates each get
        # their own pending queue keyed by request id, so a second create is
        # not rejected by a client-local singleton invariant. Admission and
        # rejection belong to the server.
        queue: asyncio.Queue[dict[str, Any] | None] = asyncio.Queue()
        self._pending_stream_queues[request_id] = queue
        return queue

    def unsubscribe_pending_stream(self, request_id: int) -> None:
        self._pending_stream_queues.pop(request_id, None)

    def set_live_notification_sink(self, sink: Callable[[str, Any], None]) -> None:
        """Route ``live/*`` notifications to ``sink(method, params)``."""
        self._live_notification_sink = sink

    def set_consent_notice_sink(self, sink: Callable[[Any], None]) -> None:
        """Route ``approval/consent_notice`` notifications to ``sink(params)``."""
        self._consent_notice_sink = sink

    def set_tool_handler(self, handler: Any) -> None:
        """Set the tool registry for handling callback tool requests."""
        self._tool_handler = handler

    def set_stdin_writer(self, process: Any) -> None:
        """Set the process for writing callback responses."""
        self._stdin_writer = process

    def set_stderr_tail(self, tail: _StderrTail | None) -> None:
        """Attach the child's stderr drain so an unexpected transport close
        can report why the child exited."""
        self._stderr_tail = tail

    async def _fail_connection_closed(self) -> None:
        """Fail every waiter with CONNECTION_CLOSED, carrying the child's
        stderr tail in both the message and ``details["stderr_tail"]``."""
        message = "rkat-rpc process closed"
        details: dict[str, Any] | None = None
        if self._stderr_tail is not None:
            await self._stderr_tail.wait_closed(STDERR_TAIL_SETTLE_SECS)
            tail = self._stderr_tail.text().strip()
            if tail:
                message = f"{message}; stderr tail:\n{tail}"
                details = {"stderr_tail": tail}
        self._fail_all("CONNECTION_CLOSED", message, details)

    async def _read_loop(self) -> None:
        while not self._closed:
            line = await self._stdout.readline()
            if not line:
                await self._fail_connection_closed()
                return
            try:
                data = json.loads(line)
            except json.JSONDecodeError:
                # A malformed frame corrupts the JSONL transport framing: we
                # cannot trust any subsequent bytes on this stream. Fail closed
                # with a typed protocol error to all pending callers instead of
                # silently dropping the frame (which would hang awaiters).
                self._fail_all(
                    "PROTOCOL_ERROR",
                    "rkat-rpc stream produced a malformed JSON frame",
                )
                return
            # Server→client callback request (has both id and method).
            # Spawn as a separate task to avoid blocking the read loop — if the
            # tool handler makes a nested SDK call, the response needs to flow
            # through this same _read_loop.
            if "id" in data and "method" in data:
                asyncio.get_running_loop().create_task(
                    self._handle_callback_request(data)
                )
                continue
            if "id" in data:
                request_id = data["id"]
                future = self._pending_responses.pop(request_id, None)
                if future and not future.done():
                    if data.get("error"):
                        err = data["error"]
                        raw_message = err.get("message", "Unknown error")
                        # The server's typed error projection is `error.data`
                        # ({code, message, details}). `error.message` is
                        # presentation text only — it is never parsed as JSON
                        # to recover typed fields, so SDK error semantics
                        # cannot depend on message-string folklore.
                        nested_code: str | None = None
                        nested_message = None
                        nested_details = err.get("data")
                        if isinstance(nested_details, dict):
                            raw_nested_code = nested_details.get("code")
                            if isinstance(raw_nested_code, str):
                                nested_code = raw_nested_code
                            nested_message = nested_details.get("message")
                            # Bare mob/lifecycle structured payloads (no
                            # nested "code"/"details" envelope) are never
                            # narrowed to a single "reason" string — that
                            # silently dropped sibling fields like
                            # `retryable`/`required_action`/`kind` for any
                            # payload that happened to also carry "reason"
                            # (e.g. `mob_member_reload_required`). The whole
                            # dict survives as `details` unless the server
                            # explicitly nests one under "details". A typed
                            # `comms/send` failure is such a bare payload: its
                            # own optional `details` field (a transport
                            # error's text) is part of it, not an envelope.
                            if nested_code not in COMMS_SEND_ERROR_CODES:
                                nested_details = nested_details.get(
                                    "details", nested_details
                                )
                        future.set_exception(
                            meerkat_error_from_jsonrpc_code(
                                err.get("code", "UNKNOWN"),
                                nested_code,
                                str(nested_message or raw_message),
                                nested_details,
                            )
                        )
                        # The server rejected this request: tear down only this
                        # request's pending stream so its consumer fails closed,
                        # leaving concurrent pending streams untouched.
                        rejected_queue = self._pending_stream_queues.pop(request_id, None)
                        if rejected_queue is not None:
                            rejected_queue.put_nowait(None)
                    else:
                        result = data.get("result", {})
                        future.set_result(result)
                        pending_queue = self._pending_stream_queues.pop(request_id, None)
                        if pending_queue is not None:
                            sid = result.get("session_id", "")
                            if sid:
                                # Drain only this session's pre-binding buffer —
                                # events are buffered by session_id, so
                                # concurrent pending streams do not
                                # cross-deliver.
                                for evt in self._unmatched_buffer.pop(sid, []):
                                    await pending_queue.put(evt)
                                self._event_queues[sid] = pending_queue
            elif "method" in data:
                method = str(data.get("method", ""))
                params = data.get("params", {})
                if method.startswith("live/"):
                    if self._live_notification_sink is not None:
                        self._live_notification_sink(method, params)
                    continue
                if method == CONSENT_NOTICE_NOTIFICATION:
                    if self._consent_notice_sink is not None:
                        self._consent_notice_sink(params)
                    continue
                if method in {"session/stream_event", "mob/stream_event"}:
                    stream_id = str(params.get("stream_id", ""))
                    raw_event = params.get("event") or params
                    # Preserve scope fields when present (delegated-branch / mob-member scoped events).
                    scope_id = params.get("scope_id")
                    scope_path = params.get("scope_path")
                    if scope_id is not None or scope_path is not None:
                        event = {
                            "event": raw_event,
                            "scope_id": scope_id,
                            "scope_path": scope_path,
                        }
                    else:
                        event = raw_event
                    sequence = params.get("sequence")
                    delivery = _SubscriptionEvent(
                        event=event,
                        rpc_sequence=(
                            sequence
                            if isinstance(sequence, int)
                            and not isinstance(sequence, bool)
                            and 0 <= sequence <= (1 << 64) - 1
                            else None
                        ),
                    )
                    queue = self._stream_queues.get(stream_id)
                    if queue is not None:
                        await queue.put(delivery)
                    elif stream_id:
                        self._unmatched_stream_buffer.setdefault(stream_id, []).append(delivery)
                    continue
                if method in {"session/stream_end", "mob/stream_end"}:
                    stream_id = str(params.get("stream_id", ""))
                    if stream_id:
                        self._stream_terminal[stream_id] = dict(params)
                    queue = self._stream_queues.get(stream_id)
                    if queue is not None:
                        await queue.put(_SubscriptionEnd(dict(params)))
                    elif stream_id:
                        self._unmatched_stream_end.add(stream_id)
                    continue
                session_id = params.get("session_id", "")
                event = params.get("event")
                if not isinstance(event, dict):
                    event = {"invalid_event_envelope": event}
                queue = self._event_queues.get(session_id)
                if queue is not None:
                    await queue.put(event)
                elif self._pending_stream_queues:
                    # A stream is pending but its session_id is not yet bound.
                    # Buffer by session_id; the response that binds this
                    # session_id drains exactly this buffer into the matching
                    # request's queue.
                    self._unmatched_buffer.setdefault(session_id, []).append(event)

    async def _handle_callback_request(self, data: dict[str, Any]) -> None:
        """Handle an incoming server→client tool/execute request."""
        request_id = data.get("id")
        method = data.get("method", "")
        params = data.get("params", {})

        if method == "tool/execute" and self._tool_handler is not None:
            tool_name = params.get("name", "")
            arguments = params.get("arguments", {})
            # Present when an agent-loop round dispatched the call; absent or
            # unparseable is no key, never a failed call.
            dedupe_key = ToolCallDedupeKey.from_wire(params.get("dedupe_key"))
            if dedupe_key is None:
                content, is_error = await self._tool_handler.handle(tool_name, arguments)
            else:
                content, is_error = await self._tool_handler.handle(
                    tool_name, arguments, dedupe_key=dedupe_key
                )
            response = {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {"content": content, "is_error": is_error},
            }
        else:
            response = {
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {
                    "code": -32601,
                    "message": f"Method not supported: {method}",
                },
            }

        self._write_response(response)

    def _write_response(self, response: dict[str, Any]) -> None:
        """Write a JSON-RPC response back to the server via stdin."""
        if self._stdin_writer is not None:
            try:
                data = (json.dumps(response) + "\n").encode()
                self._stdin_writer.write(data)
            except Exception:
                pass  # Best-effort — process may have died.

    def _fail_all(
        self,
        code: str,
        message: str,
        details: dict[str, Any] | None = None,
    ) -> None:
        if self.transport_fault is None:
            self.transport_fault = MeerkatError(code, message, details)
        for future in self._pending_responses.values():
            if not future.done():
                future.set_exception(MeerkatError(code, message, details))
        self._pending_responses.clear()
        for queue in self._event_queues.values():
            queue.put_nowait(None)
        self._event_queues.clear()
        for queue in self._stream_queues.values():
            queue.put_nowait(None)
        self._stream_queues.clear()
        # The open response may already be resolved while its caller has not
        # resumed to register a queue. Retain that received prefix and terminal
        # until subscribe_stream transfers custody. No producer remains after
        # the transport fault; other terminal records already live in queues.
        self._stream_terminal = {
            stream_id: terminal
            for stream_id, terminal in self._stream_terminal.items()
            if stream_id in self._unmatched_stream_end
        }
        for queue in self._pending_stream_queues.values():
            queue.put_nowait(None)
        self._pending_stream_queues.clear()
        self._unmatched_buffer.clear()


class EventStream:
    """Async iterable of **typed** events from a running turn.

    Use as an async context manager::

        async with session.stream("prompt") as events:
            async for event in events:
                match event:
                    case TextDelta(delta=chunk):
                        print(chunk, end="")
            result = events.result

    Or consume in one shot::

        result = await session.stream("prompt").collect()
    """

    def __init__(
        self,
        *,
        session_id: str,
        event_queue: asyncio.Queue[dict[str, Any] | None],
        response_future: asyncio.Future[dict[str, Any]],
        dispatcher: _StdoutDispatcher,
        parse_result: Callable[[dict[str, Any]], RunResult],
        pending_send: tuple[Any, bytes] | None = None,
        session: Session | None = None,
        pending_request_id: int | None = None,
    ):
        self._session_id = session_id
        self._event_queue = event_queue
        self._response_future = response_future
        self._dispatcher = dispatcher
        self._parse_result = parse_result
        self._result: RunResult | None = None
        self._pending_send = pending_send
        self._session = session
        self._pending_request_id = pending_request_id

    @property
    def session_id(self) -> str:
        return self._session_id

    @property
    def result(self) -> RunResult:
        """The final :class:`~meerkat.RunResult`.  Available after iteration completes."""
        if self._result is None:
            raise MeerkatError(
                "STREAM_NOT_CONSUMED",
                "Iterate the stream before accessing result",
            )
        return self._result

    async def __aenter__(self) -> EventStream:
        if self._pending_send is not None:
            stdin, data = self._pending_send
            self._pending_send = None
            stdin.write(data)
            await stdin.drain()
        return self

    async def __aexit__(self, *_exc: object) -> None:
        if self._session_id:
            self._dispatcher.unsubscribe_events(self._session_id)
        elif self._pending_request_id is not None:
            self._dispatcher.unsubscribe_pending_stream(self._pending_request_id)

    def __aiter__(self) -> AsyncIterator[Event]:
        return self._iter_events()

    async def _iter_events(self) -> AsyncIterator[Event]:
        """Yield typed events until the response future resolves."""
        queue_get: asyncio.Task[dict[str, Any] | None] | None = None
        response_task = asyncio.ensure_future(self._response_future)
        try:
            while True:
                if response_task.done():
                    while not self._event_queue.empty():
                        raw = self._event_queue.get_nowait()
                        if raw is None:
                            break
                        yield parse_agent_event_envelope(raw).payload
                    self._finalise(await response_task)
                    return

                if queue_get is None or queue_get.done():
                    queue_get = asyncio.ensure_future(self._event_queue.get())

                done, _ = await asyncio.wait(
                    [queue_get, response_task],
                    return_when=asyncio.FIRST_COMPLETED,
                )

                if queue_get in done:
                    raw = queue_get.result()
                    queue_get = None
                    if raw is None:
                        self._finalise(await response_task)
                        return
                    yield parse_agent_event_envelope(raw).payload
        finally:
            if queue_get is not None and not queue_get.done():
                queue_get.cancel()

    def _finalise(self, raw_result: dict[str, Any]) -> None:
        self._result = self._parse_result(raw_result)
        if not self._session_id:
            self._session_id = self._result.session_id
        if self._session is not None:
            self._session._last_result = self._result  # noqa: SLF001

    async def collect(self) -> RunResult:
        """Consume all events silently and return the final result."""
        async with self:
            async for _ in self:
                pass
        return self.result

    async def collect_text(self) -> tuple[str, RunResult]:
        """Consume events, accumulate text deltas, return ``(full_text, result)``."""
        parts: list[str] = []
        async with self:
            async for event in self:
                if isinstance(event, TextDelta):
                    parts.append(event.delta)
        return "".join(parts), self.result


class EventSubscription:
    """Standalone session/member/mob events; terminal errors raise after drain."""

    def __init__(
        self,
        *,
        stream_id: str,
        event_queue: asyncio.Queue[_SubscriptionItem],
        dispatcher: _StdoutDispatcher,
        close_remote: Callable[[str], Any],
        parse_event_fn: Callable[[dict[str, Any]], Any],
    ):
        self._stream_id = stream_id
        self._event_queue = event_queue
        self._dispatcher = dispatcher
        self._close_remote = close_remote
        self._parse_event_fn = parse_event_fn
        self._closed = False
        self._last_rpc_sequence: int | None = None
        self._last_event_seq: int | None = None
        self._terminal_outcome: dict[str, Any] | None = None

    @property
    def stream_id(self) -> str:
        return self._stream_id

    @property
    def terminal_outcome(self) -> dict[str, Any] | None:
        return self._terminal_outcome or self._dispatcher.stream_terminal(self._stream_id)

    @property
    def last_rpc_sequence(self) -> int | None:
        """RPC sequence of the last delivered event, if provided by the server."""
        return self._last_rpc_sequence

    @property
    def last_event_seq(self) -> int | None:
        """Last delivered canonical event seq, not a replay cursor.

        Gap markers do not replace it.
        """
        return self._last_event_seq

    async def __aenter__(self) -> EventSubscription:
        return self

    async def __aexit__(
        self,
        _exc_type: type[BaseException] | None,
        exc: BaseException | None,
        _traceback: object,
    ) -> None:
        try:
            await self.close()
        except BaseException:
            # Cleanup must not replace the iterator's terminal error (or the
            # caller's cancellation). With no primary failure, close rejection
            # still propagates, as it does for an explicit close() call.
            if exc is None:
                raise

    async def close(self) -> None:
        if self._closed:
            return
        # Stream-close authority is server-owned: await the RPC close BEFORE
        # surfacing closed terminal state locally. A rejected close propagates
        # the typed error and leaves the subscription open (retryable); only
        # an accepted close flips ``_closed`` and ends the local queue.
        await self._close_remote(self._stream_id)
        self._closed = True
        self._dispatcher.unsubscribe_stream(self._stream_id)
        self._event_queue.put_nowait(None)

    def __aiter__(self) -> AsyncIterator[Any]:
        return self._iter_events()

    async def _iter_events(self) -> AsyncIterator[Any]:
        while True:
            delivery = await self._event_queue.get()
            if delivery is None or isinstance(delivery, _SubscriptionEnd):
                if isinstance(delivery, _SubscriptionEnd):
                    # Keep the received terminal even if client/transport cleanup
                    # clears the dispatcher's registry before this queue drains.
                    self._terminal_outcome = delivery.terminal_outcome
                terminal = self.terminal_outcome
                if terminal is not None and terminal.get("error") is not None:
                    raise EventSubscriptionError(
                        self._stream_id,
                        terminal,
                        self._last_rpc_sequence,
                        self._last_event_seq,
                    )
                return
            event = self._parse_event_fn(delivery.event)
            envelope = getattr(event, "envelope", event)
            self._last_rpc_sequence = delivery.rpc_sequence
            if not isinstance(getattr(envelope, "payload", None), StreamTruncated):
                self._last_event_seq = getattr(envelope, "seq", None)
            yield event
