"""Streaming API for the Meerkat Python SDK.

Provides _StdoutDispatcher (internal) for multiplexing the rkat rpc stdout
stream, and StreamingTurn (public) for consuming agent events in real-time.
"""

import asyncio
import json
from typing import AsyncIterator, Callable, Optional

from .errors import MeerkatError


class _StdoutDispatcher:
    """Background reader that multiplexes stdout lines to response futures and event queues."""

    def __init__(self, stdout: asyncio.StreamReader):
        self._stdout = stdout
        self._pending_responses: dict[int, asyncio.Future] = {}
        self._event_queues: dict[str, asyncio.Queue] = {}
        self._catchall_queue: Optional[asyncio.Queue] = None
        self._task: Optional[asyncio.Task] = None
        self._closed = False

    def start(self) -> None:
        self._task = asyncio.get_event_loop().create_task(self._read_loop())

    async def stop(self) -> None:
        self._closed = True
        if self._task:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass

    def expect_response(self, request_id: int) -> "asyncio.Future[dict]":
        loop = asyncio.get_event_loop()
        future: asyncio.Future[dict] = loop.create_future()
        self._pending_responses[request_id] = future
        return future

    def subscribe_events(self, session_id: str) -> asyncio.Queue:
        queue: asyncio.Queue = asyncio.Queue()
        self._event_queues[session_id] = queue
        return queue

    def unsubscribe_events(self, session_id: str) -> None:
        self._event_queues.pop(session_id, None)

    def subscribe_catchall(self) -> asyncio.Queue:
        assert self._catchall_queue is None, "Only one catchall subscriber at a time"
        self._catchall_queue = asyncio.Queue()
        return self._catchall_queue

    def unsubscribe_catchall(self) -> None:
        self._catchall_queue = None

    async def _read_loop(self) -> None:
        while not self._closed:
            line = await self._stdout.readline()
            if not line:
                self._fail_all(MeerkatError("CONNECTION_CLOSED", "rkat rpc process closed"))
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
                            MeerkatError(str(err.get("code", "UNKNOWN")), err.get("message", "Unknown error"), err.get("data"))
                        )
                    else:
                        future.set_result(data.get("result", {}))
            elif "method" in data:
                params = data.get("params", {})
                session_id = params.get("session_id", "")
                event = params.get("event")
                queue = self._event_queues.get(session_id)
                if queue is not None:
                    await queue.put(event)
                elif self._catchall_queue is not None:
                    await self._catchall_queue.put(event)
                    self._event_queues[session_id] = self._catchall_queue
                    self._catchall_queue = None

    def _fail_all(self, error: MeerkatError) -> None:
        for future in self._pending_responses.values():
            if not future.done():
                future.set_exception(error)
        self._pending_responses.clear()
        for queue in self._event_queues.values():
            queue.put_nowait(None)
        self._event_queues.clear()
        if self._catchall_queue is not None:
            self._catchall_queue.put_nowait(None)
            self._catchall_queue = None


class StreamingTurn:
    """Async iterable of events from a running turn, with access to the final result.

    Usage::

        async with client.create_session_streaming("Hello!") as stream:
            async for event in stream:
                if event["type"] == "text_delta":
                    print(event["delta"], end="", flush=True)
            result = stream.result
    """

    def __init__(self, *, session_id: str, event_queue: asyncio.Queue,
                 response_future: "asyncio.Future[dict]", dispatcher: _StdoutDispatcher,
                 parse_result: Callable[[dict], object]):
        self._session_id = session_id
        self._event_queue = event_queue
        self._response_future = response_future
        self._dispatcher = dispatcher
        self._parse_result = parse_result
        self._result = None

    @property
    def session_id(self) -> str:
        return self._session_id

    @property
    def result(self):
        if self._result is None:
            raise MeerkatError("STREAM_NOT_CONSUMED", "Iterate the stream before accessing result")
        return self._result

    async def __aenter__(self) -> "StreamingTurn":
        return self

    async def __aexit__(self, *exc) -> None:
        if self._session_id:
            self._dispatcher.unsubscribe_events(self._session_id)
        else:
            self._dispatcher.unsubscribe_catchall()

    def __aiter__(self) -> AsyncIterator[dict]:
        return self._iter_events()

    async def _iter_events(self) -> AsyncIterator[dict]:
        while True:
            if self._response_future.done():
                while not self._event_queue.empty():
                    event = self._event_queue.get_nowait()
                    if event is None:
                        break
                    yield event
                self._result = self._parse_result(await self._response_future)
                if not self._session_id:
                    self._session_id = getattr(self._result, "session_id", "")
                return
            try:
                event = await asyncio.wait_for(self._event_queue.get(), timeout=0.05)
            except asyncio.TimeoutError:
                continue
            if event is None:
                self._result = self._parse_result(await self._response_future)
                if not self._session_id:
                    self._session_id = getattr(self._result, "session_id", "")
                return
            yield event

    async def collect(self):
        """Consume all events silently and return the final result."""
        async for _ in self:
            pass
        return self.result

    async def collect_text(self) -> tuple:
        """Consume events, accumulate text_delta events, return (full_text, result)."""
        parts: list[str] = []
        async for event in self:
            if isinstance(event, dict) and event.get("type") == "text_delta":
                parts.append(event.get("delta", ""))
        return "".join(parts), self.result
