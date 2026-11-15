"""An event reaches the subscriber when the gateway writes it.

The gateway keeps an event stream open between runs and writes each event as
it happens. The subscriber must see an event as soon as its bytes arrive: the
last events of a run (``turn_completed``, ``run_completed``) are followed by
nothing until the member's next run, so a reader that waits for more bytes
holds them back until that run, or forever.

The stream here is a local HTTP server that writes one small frame, waits
until the test has received it, writes the next, and keeps the connection
open after the last. No gateway and no model are involved. The rows cover
both subscriptions, ``subscribe_agent`` and ``subscribe_mob``, and the ASGI
app's two event routes, which read the gateway through the same reader, in
both HTTP framings.
"""
from __future__ import annotations

import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

import pytest

from meerkat_meta import RunCompleted, RunStarted, TurnCompleted
from meerkat_meta.runtime import AsgiApp, MobHandle, SseBridge

_RUN = [
    {"type": "run_started", "session_id": "s-1", "prompt": "Hello"},
    {"type": "turn_completed", "stop_reason": "end_turn"},
    {"type": "run_completed", "session_id": "s-1", "result": "Hi"},
]


def _frame(event_id: str, payload: dict, data: dict) -> bytes:
    return f"id: {event_id}\nevent: {payload['type']}\ndata: {json.dumps(data)}\n\n".encode()


# What the gateway writes on each route for the run above.
_FRAMES = {
    "/agents/agent-1/events": [
        _frame(f"1:{seq}", payload, payload) for seq, payload in enumerate(_RUN, 1)
    ],
    "/mob/events": [
        _frame(
            f"mob:{seq}",
            payload,
            {"member_id": "agent-1", "source": "agent-1", "payload": payload},
        )
        for seq, payload in enumerate(_RUN)
    ],
}

# How long one event may take to reach the subscriber after it was written.
_ARRIVAL_SECONDS = 5.0
# How long the stream stays open when nothing releases it.
_HOLD_SECONDS = 30.0


class _OpenEventStream:
    """Serves a route's frames one at a time, then keeps the connection open."""

    def __init__(self, *, chunked: bool) -> None:
        self.paths: list[str] = []
        self._received = threading.Semaphore(0)
        self._released = threading.Event()
        stream = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args) -> None:  # quiet
                return

            def do_GET(self) -> None:
                stream.paths.append(self.path)
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                if chunked:
                    self.send_header("Transfer-Encoding", "chunked")
                else:
                    self.send_header("Connection", "close")
                self.end_headers()
                try:
                    for frame in _FRAMES[self.path]:
                        if chunked:
                            self.wfile.write(b"%x\r\n%s\r\n" % (len(frame), frame))
                        else:
                            self.wfile.write(frame)
                        self.wfile.flush()
                        # The next frame is written only once the test holds
                        # this one: nothing later pushes it through.
                        if not stream._received.acquire(timeout=_HOLD_SECONDS):
                            break
                    stream._released.wait(_HOLD_SECONDS)
                    if chunked:
                        self.wfile.write(b"0\r\n\r\n")
                        self.wfile.flush()
                except OSError:
                    pass
                self.close_connection = True

        self._server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.base_url = f"http://127.0.0.1:{self._server.server_address[1]}"
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()

    def received(self) -> None:
        self._received.release()

    def close(self) -> None:
        self._released.set()
        for _ in _RUN:
            self._received.release()
        self._server.shutdown()
        self._server.server_close()


def _mob_handle(base_url: str) -> MobHandle:
    runtime = SimpleNamespace(rust_http_base_url=base_url)
    runtime.sse_bridge = lambda: SseBridge(runtime)
    return MobHandle(runtime)


async def _each_event_as_it_is_written(stream: _OpenEventStream, events) -> list:
    """The events that arrived, each within ``_ARRIVAL_SECONDS`` of being
    written, while the stream stayed open."""
    received = []
    waiting = None
    try:
        for _ in _RUN:
            waiting = asyncio.ensure_future(events.__anext__())
            arrived, _ = await asyncio.wait({waiting}, timeout=_ARRIVAL_SECONDS)
            if not arrived:
                break
            received.append(waiting.result())
            waiting = None
            stream.received()
    finally:
        # End the stream before the reader is stopped: a read still waiting
        # for bytes returns when the body ends.
        stream.close()
        if waiting is not None:
            await asyncio.wait({waiting}, timeout=_ARRIVAL_SECONDS)
        await events.aclose()
    return received


_FRAMINGS = pytest.mark.parametrize("chunked", [True, False], ids=["chunked", "until-close"])
_LATE = (
    "every event must reach the subscriber within "
    f"{_ARRIVAL_SECONDS:.0f} s of being written to the open stream"
)


@pytest.mark.asyncio
@_FRAMINGS
async def test_each_agent_event_arrives_while_the_stream_stays_open(chunked):
    stream = _OpenEventStream(chunked=chunked)
    events = _mob_handle(stream.base_url).subscribe_agent("agent-1")
    received = await _each_event_as_it_is_written(stream, events)

    assert stream.paths == ["/agents/agent-1/events"]
    assert [event.event_type for event in received] == [
        "run_started",
        "turn_completed",
        "run_completed",
    ], _LATE
    assert isinstance(received[0].event, RunStarted)
    assert isinstance(received[1].event, TurnCompleted)
    assert received[2].event == RunCompleted(session_id="s-1", result="Hi")


@pytest.mark.asyncio
@_FRAMINGS
async def test_each_mob_event_arrives_while_the_stream_stays_open(chunked):
    stream = _OpenEventStream(chunked=chunked)
    events = _mob_handle(stream.base_url).subscribe_mob()
    received = await _each_event_as_it_is_written(stream, events)

    assert stream.paths == ["/mob/events"]
    assert [type(event.event) for event in received] == [
        RunStarted,
        TurnCompleted,
        RunCompleted,
    ], _LATE
    assert {event.member_id for event in received} == {"agent-1"}
    assert received[2].event == RunCompleted(session_id="s-1", result="Hi")


@pytest.mark.asyncio
@_FRAMINGS
@pytest.mark.parametrize("path", sorted(_FRAMES), ids=["agent-route", "mob-route"])
async def test_the_asgi_app_forwards_each_event_while_the_stream_stays_open(path, chunked):
    stream = _OpenEventStream(chunked=chunked)
    runtime = SimpleNamespace(rust_http_base_url=stream.base_url)
    runtime.sse_bridge = lambda: SseBridge(runtime)
    sent: asyncio.Queue = asyncio.Queue()

    async def receive():
        await asyncio.Event().wait()

    call = asyncio.ensure_future(
        AsgiApp(runtime)(
            {"type": "http", "method": "GET", "path": path, "headers": []},
            receive,
            sent.put,
        )
    )
    messages = []
    try:
        # The response start, then one body message per event.
        for _ in range(1 + len(_RUN)):
            try:
                messages.append(await asyncio.wait_for(sent.get(), _ARRIVAL_SECONDS))
            except asyncio.TimeoutError:
                break
            if messages[-1]["type"] == "http.response.body":
                stream.received()
    finally:
        stream.close()
        await asyncio.wait({call}, timeout=_ARRIVAL_SECONDS)

    assert stream.paths == [path]
    assert [message["type"] for message in messages] == [
        "http.response.start",
        "http.response.body",
        "http.response.body",
        "http.response.body",
    ], _LATE
    assert messages[0]["status"] == 200
    for message, payload in zip(messages[1:], _RUN):
        assert f"event: {payload['type']}\n".encode() in message["body"]
        assert message["more_body"] is True
