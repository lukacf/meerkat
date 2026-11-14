"""The SSE bridge hands an event on when it arrives.

A subscriber sees a run through the gateway's event stream. A reader that
waits for a full buffer holds a small event back until later bytes arrive:
on a quiet stream the end of a run, or the whole of a short one, never
reaches the subscriber. These tests serve a stream that goes quiet after a
few bytes and stays open, in both HTTP framings.
"""
import asyncio
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

import pytest

from meerkat_meta.runtime import SseBridge

# How long a served stream stays open after its last byte, and how long a
# test waits for an event. A reader that waits for a full buffer gets nothing
# until the stream closes, so it runs into the bound long before the hold
# ends. The bound is generous for a loaded runner; delivery takes milliseconds.
HOLD_SECONDS = 30.0
DELIVERY_BOUND_SECONDS = 5.0


class _QuietStream:
    """Serve ``parts`` with ``gap`` seconds between them, then stay open."""

    def __init__(self, parts, *, chunked, gap=0.0, hold=True):
        self.release = threading.Event()
        release = self.release

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1" if chunked else "HTTP/1.0"

            def log_message(self, *args):
                pass

            def do_GET(self):
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                if chunked:
                    self.send_header("Transfer-Encoding", "chunked")
                self.end_headers()
                for index, part in enumerate(parts):
                    if index:
                        time.sleep(gap)
                    if chunked:
                        part = b"%x\r\n%s\r\n" % (len(part), part)
                    self.wfile.write(part)
                    self.wfile.flush()
                if hold:
                    release.wait(HOLD_SECONDS)
                if chunked:
                    try:
                        self.wfile.write(b"0\r\n\r\n")
                    except OSError:
                        pass
                self.close_connection = True

        self._server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._server.daemon_threads = True
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)

    def __enter__(self):
        self._thread.start()
        host, port = self._server.server_address
        return f"http://{host}:{port}"

    def __exit__(self, *exc):
        self.release.set()
        self._server.shutdown()
        self._server.server_close()


def _agent_events(base_url):
    bridge = SseBridge(SimpleNamespace(rust_http_base_url=base_url))
    return bridge.agent_events("agent-1")


FRAMINGS = pytest.mark.parametrize("chunked", [True, False], ids=["chunked", "until-close"])


@FRAMINGS
@pytest.mark.asyncio
@pytest.mark.timeout(60)
async def test_a_small_event_is_delivered_while_the_stream_stays_open(chunked):
    served = _QuietStream([b"event: run_completed\ndata: {}\n\n"], chunked=chunked)
    with served as base_url:
        events = _agent_events(base_url)
        try:
            event = await asyncio.wait_for(events.__anext__(), DELIVERY_BOUND_SECONDS)
        finally:
            served.release.set()
            await events.aclose()
    assert (event.event, event.data) == ("run_completed", "{}")


@FRAMINGS
@pytest.mark.asyncio
@pytest.mark.timeout(60)
async def test_an_event_split_across_two_writes_is_delivered_whole(chunked):
    parts = [b'event: text_delta\ndata: {"del', b'ta": "hi"}\n\n']
    served = _QuietStream(parts, chunked=chunked, gap=0.3)
    with served as base_url:
        events = _agent_events(base_url)
        try:
            event = await asyncio.wait_for(events.__anext__(), DELIVERY_BOUND_SECONDS)
        finally:
            served.release.set()
            await events.aclose()
    assert (event.event, event.data) == ("text_delta", '{"delta": "hi"}')


@FRAMINGS
@pytest.mark.asyncio
@pytest.mark.timeout(60)
async def test_the_iteration_ends_when_the_server_closes_the_stream(chunked):
    parts = [b"data: first\n\n", b"data: second\n\n"]
    served = _QuietStream(parts, chunked=chunked, hold=False)
    with served as base_url:
        received = [event.data async for event in _agent_events(base_url)]
    assert received == ["first", "second"]
