"""Async iterator over WireEvent from the rkat rpc event stream."""

import asyncio
import json

from .generated.types import WireEvent


class EventStream:
    """Async iterator that yields WireEvent objects from a JSON-RPC notification stream.

    Reads from an ``asyncio.StreamReader`` (the stdout of the rkat rpc process).
    Skips JSON-RPC response messages (which have an ``id`` field) and only
    yields notification payloads parsed as ``WireEvent``.

    Usage::

        async for event in EventStream(process.stdout):
            print(event.event)
    """

    def __init__(self, stream: asyncio.StreamReader):
        self._stream = stream
        self._closed = False

    def __aiter__(self) -> "EventStream":
        return self

    async def __anext__(self) -> WireEvent:
        while not self._closed:
            line = await self._stream.readline()
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
