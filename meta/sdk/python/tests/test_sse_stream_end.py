"""The end of an SSE stream discards pending data, as the SSE spec does.

An event without its closing blank line is not dispatched.
"""
import pytest

from meerkat_meta._sse import parse_sse_stream


async def _chunks(*parts: bytes):
    for part in parts:
        yield part


@pytest.mark.asyncio
async def test_a_final_event_without_its_closing_blank_line_is_not_dispatched():
    stream = _chunks(b"data: first\n\n", b"event: last\ndata: pending\n")
    events = [event async for event in parse_sse_stream(stream)]
    assert [event.data for event in events] == ["first"]
