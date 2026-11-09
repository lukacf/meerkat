"""The SSE reader decodes whole lines.

A multi-byte character that a chunk boundary splits arrives intact, whatever
the split, and a complete line that is not UTF-8 is refused instead of being
repaired with U+FFFD.
"""
import pytest

from meerkat_meta import TransportError
from meerkat_meta._sse import parse_sse_stream


async def _chunks(*parts: bytes):
    for part in parts:
        yield part


async def _collect(*parts: bytes):
    return [event async for event in parse_sse_stream(_chunks(*parts))]


def _splits_inside(payload: bytes, char: str) -> list[tuple[bytes, bytes]]:
    """Every two-chunk split of ``payload`` that falls inside ``char``."""
    encoded = char.encode("utf-8")
    start = payload.index(encoded)
    return [(payload[: start + cut], payload[start + cut :]) for cut in range(1, len(encoded))]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "char",
    ["\u00e9", "\u2019", "\U0001f600"],
    ids=["two-byte", "three-byte", "four-byte"],
)
async def test_a_character_split_across_chunks_arrives_whole(char):
    data = f'{{"delta": "it{char}s"}}'
    frame = f"event: text\ndata: {data}\n\n".encode("utf-8")
    splits = _splits_inside(frame, char)
    assert len(splits) == len(char.encode("utf-8")) - 1
    for head, tail in splits:
        events = await _collect(head, tail)
        assert [(event.event, event.data) for event in events] == [("text", data)], (head, tail)


@pytest.mark.asyncio
async def test_one_byte_chunks_and_a_split_at_a_line_boundary_keep_every_line():
    text = "caf\u00e9 \u2019 \U0001f600"
    frame = f"id: 7\nevent: text\ndata: {text}\n\n".encode("utf-8")

    one_byte = await _collect(*[frame[index : index + 1] for index in range(len(frame))])
    assert [(event.id, event.event, event.data) for event in one_byte] == [("7", "text", text)]

    newline = frame.index(b"\n") + 1
    at_boundary = await _collect(frame[:newline], frame[newline:])
    assert [(event.id, event.data) for event in at_boundary] == [("7", text)]


@pytest.mark.asyncio
async def test_a_line_that_is_not_utf8_is_refused_with_the_sdk_transport_error():
    # A truncated three-byte sequence, then the end of the line.
    with pytest.raises(TransportError, match="UTF-8") as refused:
        await _collect(b"data: it\xe2\x80s\n\n")
    assert isinstance(refused.value.__cause__, UnicodeDecodeError)


@pytest.mark.asyncio
async def test_a_stream_that_ends_inside_a_character_is_refused():
    # A complete event, then the first two bytes of U+2019 and the end.
    with pytest.raises(TransportError, match="UTF-8"):
        await _collect(b"data: whole\n\ndata: it\xe2\x80")
