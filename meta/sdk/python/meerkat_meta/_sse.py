"""SSE streaming bridge for ASGI integration."""
from __future__ import annotations

import asyncio
import json
from typing import Any, AsyncIterator

from .errors import TransportError


class SseEvent:
    __slots__ = ("id", "event", "data")

    def __init__(self, *, id: str | None = None, event: str = "message", data: str = ""):
        self.id = id
        self.event = event
        self.data = data

    def encode(self) -> str:
        lines: list[str] = []
        if self.id is not None:
            lines.append(f"id: {self.id}")
        if self.event != "message":
            lines.append(f"event: {self.event}")
        for line in self.data.split("\n"):
            lines.append(f"data: {line}")
        lines.append("")
        lines.append("")
        return "\n".join(lines)

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {"event": self.event, "data": self.data}
        if self.id is not None:
            result["id"] = self.id
        return result


class SseEventStream:
    """Async iterator yielding SSE-formatted strings.

    Use with StreamingResponse(stream, media_type="text/event-stream").
    """

    def __init__(
        self,
        source: AsyncIterator[dict[str, Any]],
        *,
        keep_alive_interval: float = 15.0,
    ):
        self._source = source
        self._keep_alive_interval = keep_alive_interval

    async def __aiter__(self) -> AsyncIterator[str]:
        source_iter = self._source.__aiter__()
        while True:
            try:
                event_data = await asyncio.wait_for(
                    source_iter.__anext__(),
                    timeout=self._keep_alive_interval,
                )
                data_str = (
                    json.dumps(event_data.get("data", {}))
                    if not isinstance(event_data.get("data"), str)
                    else event_data.get("data", "")
                )
                event = SseEvent(
                    id=event_data.get("id"),
                    event=event_data.get("event", "message"),
                    data=data_str,
                )
                yield event.encode()
            except asyncio.TimeoutError:
                yield ": keep-alive\n\n"
            except StopAsyncIteration:
                break


async def parse_sse_stream(
    response_body: AsyncIterator[bytes],
) -> AsyncIterator[SseEvent]:
    """Parse an SSE byte stream into SseEvent objects.

    Lines are split on bytes and each complete line is decoded as strict
    UTF-8, so a multi-byte character that a chunk boundary splits arrives
    whole. Bytes that are not UTF-8, in a complete line or in a stream that
    ends inside a character, raise :class:`~meerkat_meta.errors.TransportError`
    (caused by the ``UnicodeDecodeError``) instead of being repaired with
    U+FFFD.
    """
    buffer = b""
    current_id: str | None = None
    current_event = "message"
    current_data: list[str] = []

    async for chunk in response_body:
        buffer += chunk
        while b"\n" in buffer:
            raw_line, buffer = buffer.split(b"\n", 1)
            line = _decode_sse_line(raw_line.rstrip(b"\r"))

            if not line:
                if current_data:
                    yield SseEvent(
                        id=current_id,
                        event=current_event,
                        data="\n".join(current_data),
                    )
                current_id = None
                current_event = "message"
                current_data = []
            elif line.startswith(":"):
                continue
            elif line.startswith("id: "):
                current_id = line[4:]
            elif line.startswith("event: "):
                current_event = line[7:]
            elif line.startswith("data: "):
                current_data.append(line[6:])
            elif line.startswith("data:"):
                current_data.append(line[5:])

    # An unterminated last line is dropped, as before; one that ends inside a
    # character means the stream was cut mid-character, which is refused too.
    _decode_sse_line(buffer)


def _decode_sse_line(raw: bytes) -> str:
    """One SSE line as strict UTF-8; bytes that are not UTF-8 refuse the stream."""
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise TransportError("SSE stream is not valid UTF-8") from error
