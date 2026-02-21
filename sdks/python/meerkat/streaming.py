"""Streaming API for the Meerkat Python SDK.

Provides :class:`_StdoutDispatcher` (internal) for multiplexing the rkat-rpc
stdout stream, and :class:`EventStream` (public) for consuming typed agent
events in real-time.
"""

from __future__ import annotations

import asyncio
import json
from typing import TYPE_CHECKING, Any, AsyncIterator, Callable

from .errors import MeerkatError
from .events import Event, TextDelta, parse_event

if TYPE_CHECKING:
    from .session import Session
    from .types import RunResult


class _StdoutDispatcher:
    """Background reader that multiplexes stdout lines to response futures and event queues."""

    def __init__(self, stdout: asyncio.StreamReader):
        self._stdout = stdout
        self._pending_responses: dict[int, asyncio.Future[dict[str, Any]]] = {}
        self._event_queues: dict[str, asyncio.Queue[dict[str, Any] | None]] = {}
        self._pending_stream_queue: asyncio.Queue[dict[str, Any] | None] | None = None
        self._pending_stream_request_id: int | None = None
        self._unmatched_buffer: dict[str, list[dict[str, Any]]] = {}
        self._task: asyncio.Task[None] | None = None
        self._closed = False

    def start(self) -> None:
        self._task = asyncio.get_running_loop().create_task(self._read_loop())

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

    def subscribe_pending_stream(self, request_id: int) -> asyncio.Queue[dict[str, Any] | None]:
        if self._pending_stream_queue is not None:
            raise RuntimeError("Only one pending stream at a time")
        self._pending_stream_queue = asyncio.Queue()
        self._pending_stream_request_id = request_id
        return self._pending_stream_queue

    def unsubscribe_pending_stream(self) -> None:
        self._pending_stream_queue = None
        self._pending_stream_request_id = None
        self._unmatched_buffer.clear()

    async def _read_loop(self) -> None:
        while not self._closed:
            line = await self._stdout.readline()
            if not line:
                self._fail_all("CONNECTION_CLOSED", "rkat-rpc process closed")
                return
            try:
                data = json.loads(line)
            except json.JSONDecodeError:
                continue
            if "id" in data:
                request_id = data["id"]
                future = self._pending_responses.pop(request_id, None)
                if future and not future.done():
                    if data.get("error"):
                        err = data["error"]
                        future.set_exception(
                            MeerkatError(
                                str(err.get("code", "UNKNOWN")),
                                err.get("message", "Unknown error"),
                                err.get("data"),
                            )
                        )
                    else:
                        result = data.get("result", {})
                        future.set_result(result)
                        if (
                            request_id == self._pending_stream_request_id
                            and self._pending_stream_queue is not None
                        ):
                            sid = result.get("session_id", "")
                            if sid:
                                for evt in self._unmatched_buffer.pop(sid, []):
                                    await self._pending_stream_queue.put(evt)
                                self._event_queues[sid] = self._pending_stream_queue
                            self._pending_stream_queue = None
                            self._pending_stream_request_id = None
                            self._unmatched_buffer.clear()
            elif "method" in data:
                params = data.get("params", {})
                session_id = params.get("session_id", "")
                event = params.get("event")
                queue = self._event_queues.get(session_id)
                if queue is not None:
                    await queue.put(event)
                elif self._pending_stream_queue is not None:
                    self._unmatched_buffer.setdefault(session_id, []).append(event)

    def _fail_all(self, code: str, message: str) -> None:
        for future in self._pending_responses.values():
            if not future.done():
                future.set_exception(MeerkatError(code, message))
        self._pending_responses.clear()
        for queue in self._event_queues.values():
            queue.put_nowait(None)
        self._event_queues.clear()
        if self._pending_stream_queue is not None:
            self._pending_stream_queue.put_nowait(None)
            self._pending_stream_queue = None
            self._pending_stream_request_id = None
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
    ):
        self._session_id = session_id
        self._event_queue = event_queue
        self._response_future = response_future
        self._dispatcher = dispatcher
        self._parse_result = parse_result
        self._result: RunResult | None = None
        self._pending_send = pending_send
        self._session = session

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
        else:
            self._dispatcher.unsubscribe_pending_stream()

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
                        yield parse_event(raw)
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
                    yield parse_event(raw)
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
