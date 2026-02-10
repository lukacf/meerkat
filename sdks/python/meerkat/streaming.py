"""Async iterator over WireEvent from the rkat rpc event stream."""

import json
from typing import AsyncIterator

from .generated.types import WireEvent


class EventStream:
    """Async iterator that yields WireEvent objects from a JSON-RPC notification stream.

    Usage:
        async for event in EventStream(process.stdout):
            print(event.event)
    """

    def __init__(self, stream):
        self._stream = stream
        self._closed = False

    def __aiter__(self) -> "EventStream":
        return self

    async def __anext__(self) -> WireEvent:
        while not self._closed:
            line = self._stream.readline()
            if not line:
                self._closed = True
                raise StopAsyncIteration

            try:
                data = json.loads(line)
            except json.JSONDecodeError:
                continue

            # Skip non-notification messages (responses have "id")
            if "id" in data:
                continue

            # Parse notification params as WireEvent
            params = data.get("params", {})
            return WireEvent(
                session_id=params.get("session_id", ""),
                sequence=params.get("sequence", 0),
                event=params.get("event"),
                contract_version=params.get("contract_version", ""),
            )

        raise StopAsyncIteration
