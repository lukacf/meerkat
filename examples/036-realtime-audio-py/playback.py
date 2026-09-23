"""One device owner; interruption bypasses the PCM queue."""

from __future__ import annotations

import asyncio
from collections import deque
from dataclasses import dataclass, replace
import threading
from typing import Any


@dataclass(frozen=True)
class AudioChunk:
    data: bytes
    response_id: str | None
    item_id: str | None
    content_index: int | None


class Playback:
    def __init__(self, *, enabled: bool = True) -> None:
        self._condition = threading.Condition()
        self._queue: deque[AudioChunk] = deque()
        self._current: AudioChunk | None = None
        self._latest: AudioChunk | None = None
        self._buffered: AudioChunk | None = None
        self._enabled = enabled
        self._responses: set[str] = set()
        self._items: set[tuple[str, int | None]] = set()
        self._item_responses: dict[str, set[str]] = {}
        self._anonymous_blocked = False
        self._closed = False
        self._aborting = False
        self._stream: Any = None
        self._control = asyncio.Lock()
        self._finished = asyncio.Event()

    def _blocked(self, chunk: AudioChunk) -> bool:
        responses = self._item_responses.get(chunk.item_id, set())
        ambiguous_truncation = chunk.item_id is None and any(
            chunk.response_id in self._item_responses.get(item, set())
            and (index is None or chunk.content_index is None or index == chunk.content_index)
            for item, index in self._items
        )
        return (
            chunk.response_id in self._responses
            or bool(responses & self._responses)
            or len(responses) > 1
            or (chunk.item_id, chunk.content_index) in self._items
            or (chunk.item_id, None) in self._items
            or (chunk.content_index is None and any(item == chunk.item_id for item, _ in self._items))
            or ambiguous_truncation
            or (self._anonymous_blocked and not chunk.response_id and not chunk.item_id)
        )

    def _remember_identity(self, chunk: AudioChunk) -> AudioChunk:
        if chunk.item_id and chunk.response_id:
            self._item_responses.setdefault(chunk.item_id, set()).add(chunk.response_id)
        responses = self._item_responses.get(chunk.item_id, set())
        if chunk.response_id is None and len(responses) == 1:
            return replace(chunk, response_id=next(iter(responses)))
        return chunk

    @staticmethod
    def _scope(chunk: AudioChunk) -> tuple[str | None, str | None, int | None]:
        return chunk.response_id, chunk.item_id, chunk.content_index

    def enqueue(self, data: bytes, observation: dict[str, Any]) -> None:
        chunk = AudioChunk(
            data, observation.get("response_id"),
            observation.get("item_id") or observation.get("provider_item_id"),
            observation.get("content_index"),
        )
        with self._condition:
            chunk = self._remember_identity(chunk)
            if not self._enabled or self._closed or self._blocked(chunk):
                return
            self._latest = chunk
            self._queue.append(chunk)
            self._condition.notify_all()

    async def interrupt(self, observation: dict[str, Any]) -> None:
        async with self._control:
            with self._condition:
                if self._closed:
                    return
                response = observation.get("response_id")
                item = observation.get("item_id") or observation.get("provider_item_id")
                index = observation.get("content_index")
                if not response and not item:
                    active = self._current or self._buffered or self._latest
                    if active:
                        response, item, index = active.response_id, active.item_id, active.content_index
                    self._anonymous_blocked = True
                identity = self._remember_identity(AudioChunk(b"", response, item, index))
                response = identity.response_id
                if item and (not response or observation.get("observation") == "assistant_transcript_truncated"):
                    self._items.add((item, index))
                elif response:
                    self._responses.add(response)
                # Identity-free late audio cannot safely be assigned to a new response.
                self._anonymous_blocked = True
                self._queue = deque(c for c in self._queue if not self._blocked(c))
                abort = self._buffered is not None and self._blocked(self._buffered)
                if abort:
                    self._aborting = True
            if abort:
                await self._abort()

    async def _abort(self) -> None:
        with self._condition:
            stream = self._stream
            self._aborting = True
        try:
            if stream is not None:
                # PortAudio abort unblocks a blocking write. Never await that write
                # before requesting abort, nor mistake task cancellation for thread exit.
                task = asyncio.create_task(asyncio.to_thread(stream.abort))
                try:
                    await asyncio.shield(task)
                finally:
                    await asyncio.shield(task)
            with self._condition:
                self._buffered = None
        finally:
            with self._condition:
                self._aborting = False
                self._condition.notify_all()

    async def close(self) -> None:
        async with self._control:
            with self._condition:
                if self._closed:
                    return
                self._closed = True
                self._queue.clear()
                self._condition.notify_all()
            await self._abort()
            self._finished.set()

    def _play(self, stream: Any) -> None:
        with self._condition:
            self._stream = stream
        try:
            while True:
                with self._condition:
                    self._condition.wait_for(
                        lambda: not self._aborting and (self._closed or self._queue)
                    )
                    if self._closed:
                        return
                    chunk = self._queue.popleft()
                    buffered = self._buffered
                if buffered is not None and self._scope(buffered) != self._scope(chunk):
                    # A completed write is not a playback receipt. Drain before
                    # handing the shared device to a different interruptible scope,
                    # so aborting old output can never flush or replay newer output.
                    try:
                        stream.stop()
                    except Exception:
                        with self._condition:
                            if not self._closed and not self._blocked(buffered):
                                raise
                    with self._condition:
                        self._condition.wait_for(lambda: not self._aborting)
                        self._buffered = None
                with self._condition:
                    self._condition.wait_for(lambda: not self._aborting)
                    if self._closed:
                        return
                    if self._blocked(chunk):
                        continue
                    self._current = chunk
                    self._buffered = chunk
                    if not stream.active:
                        stream.start()
                try:
                    stream.write(chunk.data)
                except Exception:
                    with self._condition:
                        if not self._closed and not self._blocked(chunk):
                            raise
                finally:
                    with self._condition:
                        self._current = None
        finally:
            with self._condition:
                self._condition.wait_for(lambda: not self._aborting)
                self._stream = None
            stream.close()

    async def run(self, stream: Any = None) -> None:
        if stream is None:
            await self._finished.wait()
            return
        worker = asyncio.create_task(asyncio.to_thread(self._play, stream))
        try:
            await asyncio.shield(worker)
        finally:
            await self.close()
            await asyncio.shield(worker)
