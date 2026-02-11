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
        # request_id that owns the catchall (for validation)
        self._catchall_request_id: Optional[int] = None
        self._task: Optional[asyncio.Task] = None
        self._closed = False

    def start(self) -> None:
        """Start the background reader task."""
        self._task = asyncio.get_running_loop().create_task(self._read_loop())

    async def stop(self) -> None:
        """Stop the background reader, fail pending futures, and cancel the task."""
        self._closed = True
        # Fail all pending futures so callers don't hang
        self._fail_all("CLIENT_CLOSED", "client stopped")
        if self._task:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass

    def expect_response(self, request_id: int) -> "asyncio.Future[dict]":
        """Register a future that will be resolved when a response with this ID arrives."""
        future: asyncio.Future[dict] = asyncio.get_running_loop().create_future()
        self._pending_responses[request_id] = future
        return future

    def subscribe_events(self, session_id: str) -> asyncio.Queue:
        """Create an event queue for a session. None sentinel signals end-of-stream."""
        queue: asyncio.Queue = asyncio.Queue()
        self._event_queues[session_id] = queue
        return queue

    def unsubscribe_events(self, session_id: str) -> None:
        """Remove the event queue for a session."""
        self._event_queues.pop(session_id, None)

    def subscribe_catchall(self, request_id: int) -> asyncio.Queue:
        """Subscribe a catch-all queue for events with no dedicated subscriber.

        Only one catchall can be active at a time. Used by
        ``create_session_streaming`` before the session_id is known.
        The ``request_id`` is used to correlate the catchall with the
        response that will reveal the session_id.
        """
        if self._catchall_queue is not None:
            raise RuntimeError("Only one catchall subscriber at a time")
        self._catchall_queue = asyncio.Queue()
        self._catchall_request_id = request_id
        return self._catchall_queue

    def unsubscribe_catchall(self) -> None:
        """Remove the catchall queue without promoting it."""
        self._catchall_queue = None
        self._catchall_request_id = None

    async def _read_loop(self) -> None:
        """Main loop: read lines, dispatch to responses or event queues."""
        while not self._closed:
            line = await self._stdout.readline()
            if not line:
                self._fail_all("CONNECTION_CLOSED", "rkat rpc process closed")
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
                        # If this is the response for the catchall owner,
                        # promote the catchall queue to the real session_id.
                        if (
                            request_id == self._catchall_request_id
                            and self._catchall_queue is not None
                        ):
                            sid = result.get("session_id", "")
                            if sid:
                                self._event_queues[sid] = self._catchall_queue
                            self._catchall_queue = None
                            self._catchall_request_id = None
            elif "method" in data:
                params = data.get("params", {})
                session_id = params.get("session_id", "")
                event = params.get("event")
                queue = self._event_queues.get(session_id)
                if queue is not None:
                    await queue.put(event)
                elif (
                    self._catchall_queue is not None
                    and len(self._event_queues) == 0
                ):
                    # Only route to catchall when no other sessions are
                    # subscribed — prevents cross-session contamination
                    # under concurrency.
                    await self._catchall_queue.put(event)

    def _fail_all(self, code: str, message: str) -> None:
        """Fail all pending response futures and close all event queues.

        Creates a fresh MeerkatError per future to avoid shared tracebacks.
        """
        for future in self._pending_responses.values():
            if not future.done():
                future.set_exception(MeerkatError(code, message))
        self._pending_responses.clear()
        for queue in self._event_queues.values():
            queue.put_nowait(None)
        self._event_queues.clear()
        if self._catchall_queue is not None:
            self._catchall_queue.put_nowait(None)
            self._catchall_queue = None
            self._catchall_request_id = None


class StreamingTurn:
    """Async iterable of events from a running turn, with access to the final result.

    Usage::

        async with client.create_session_streaming("Hello!") as stream:
            async for event in stream:
                if event["type"] == "text_delta":
                    print(event["delta"], end="", flush=True)
            result = stream.result
    """

    def __init__(
        self,
        *,
        session_id: str,
        event_queue: asyncio.Queue,
        response_future: "asyncio.Future[dict]",
        dispatcher: _StdoutDispatcher,
        parse_result: Callable[[dict], object],
        pending_send: Optional[tuple] = None,
    ):
        self._session_id = session_id
        self._event_queue = event_queue
        self._response_future = response_future
        self._dispatcher = dispatcher
        self._parse_result = parse_result
        self._result = None
        # (stdin, encoded_line) — flushed in __aenter__ so the method can be non-async
        self._pending_send: Optional[tuple] = pending_send

    @property
    def session_id(self) -> str:
        """The session ID for this turn."""
        return self._session_id

    @property
    def result(self):
        """The final run result. Available after iteration completes."""
        if self._result is None:
            raise MeerkatError(
                "STREAM_NOT_CONSUMED",
                "Iterate the stream before accessing result",
            )
        return self._result

    async def __aenter__(self) -> "StreamingTurn":
        if self._pending_send is not None:
            stdin, data = self._pending_send
            self._pending_send = None
            stdin.write(data)
            await stdin.drain()
        return self

    async def __aexit__(self, *_exc) -> None:
        if self._session_id:
            self._dispatcher.unsubscribe_events(self._session_id)
        else:
            self._dispatcher.unsubscribe_catchall()

    def __aiter__(self) -> AsyncIterator[dict]:
        return self._iter_events()

    async def _iter_events(self) -> AsyncIterator[dict]:
        """Yield events until the response future resolves, then drain remaining."""
        queue_get: Optional[asyncio.Task] = None
        # Wrap the response future once (not per-iteration)
        response_task = asyncio.ensure_future(self._response_future)
        try:
            while True:
                if response_task.done():
                    # Drain any remaining queued events
                    while not self._event_queue.empty():
                        event = self._event_queue.get_nowait()
                        if event is None:
                            break
                        yield event
                    self._result = self._parse_result(await response_task)
                    if not self._session_id:
                        self._session_id = getattr(self._result, "session_id", "")
                    return

                # Wait for either an event or the response, whichever comes first.
                if queue_get is None or queue_get.done():
                    queue_get = asyncio.ensure_future(self._event_queue.get())

                done, _ = await asyncio.wait(
                    [queue_get, response_task],
                    return_when=asyncio.FIRST_COMPLETED,
                )

                # If the queue produced a result, yield it
                if queue_get in done:
                    event = queue_get.result()
                    queue_get = None  # will be recreated next iteration
                    if event is None:
                        self._result = self._parse_result(await response_task)
                        if not self._session_id:
                            self._session_id = getattr(self._result, "session_id", "")
                        return
                    yield event
                # If the response resolved, loop back to the top which handles it
        finally:
            # Cancel any pending queue_get task to avoid orphaned task warnings
            if queue_get is not None and not queue_get.done():
                queue_get.cancel()

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
