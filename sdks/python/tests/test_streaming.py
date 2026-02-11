"""Tests for the streaming API: _StdoutDispatcher and StreamingTurn."""

import asyncio
import json

import pytest

from meerkat.errors import MeerkatError
from meerkat.streaming import StreamingTurn, _StdoutDispatcher


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def make_reader(lines: list[str]) -> asyncio.StreamReader:
    """Create a StreamReader pre-loaded with newline-terminated JSON lines."""
    reader = asyncio.StreamReader()
    for line in lines:
        if not line.endswith("\n"):
            line += "\n"
        reader.feed_data(line.encode())
    reader.feed_eof()
    return reader


def jline(obj: dict) -> str:
    """Serialize a dict to a JSON line."""
    return json.dumps(obj)


def response(request_id: int, result: dict) -> str:
    return jline({"jsonrpc": "2.0", "id": request_id, "result": result})


def error_response(request_id: int, code: int, message: str) -> str:
    return jline({"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}})


def event_notification(session_id: str, event: dict) -> str:
    return jline({
        "jsonrpc": "2.0",
        "method": "session/event",
        "params": {"session_id": session_id, "event": event},
    })


RUN_RESULT = {
    "session_id": "s1",
    "text": "Hello!",
    "turns": 1,
    "tool_calls": 0,
    "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15},
}


# ===========================================================================
# _StdoutDispatcher tests
# ===========================================================================

class TestStdoutDispatcher:

    @pytest.mark.asyncio
    async def test_response_dispatched_to_correct_future(self):
        reader = make_reader([response(1, {"text": "hello"})])
        d = _StdoutDispatcher(reader)
        d.start()
        future = d.expect_response(1)
        result = await asyncio.wait_for(future, timeout=1.0)
        assert result == {"text": "hello"}
        await d.stop()

    @pytest.mark.asyncio
    async def test_error_response_rejects_future(self):
        reader = make_reader([error_response(1, -32600, "bad request")])
        d = _StdoutDispatcher(reader)
        d.start()
        future = d.expect_response(1)
        with pytest.raises(MeerkatError, match="bad request"):
            await asyncio.wait_for(future, timeout=1.0)
        await d.stop()

    @pytest.mark.asyncio
    async def test_notification_dispatched_to_event_queue(self):
        ev = {"type": "text_delta", "delta": "hi"}
        reader = make_reader([event_notification("s1", ev)])
        d = _StdoutDispatcher(reader)
        d.start()
        queue = d.subscribe_events("s1")
        event = await asyncio.wait_for(queue.get(), timeout=1.0)
        assert event == ev
        await d.stop()

    @pytest.mark.asyncio
    async def test_unsubscribed_notifications_dropped(self):
        """Events for sessions with no subscriber are silently dropped."""
        reader = make_reader([
            event_notification("unknown", {"type": "text_delta", "delta": "x"}),
            response(1, {}),
        ])
        d = _StdoutDispatcher(reader)
        d.start()
        future = d.expect_response(1)
        result = await asyncio.wait_for(future, timeout=1.0)
        assert result == {}
        await d.stop()

    @pytest.mark.asyncio
    async def test_interleaved_events_and_response(self):
        reader = make_reader([
            event_notification("s1", {"type": "turn_started", "turn_number": 0}),
            event_notification("s1", {"type": "text_delta", "delta": "Hello"}),
            event_notification("s1", {"type": "text_complete", "content": "Hello"}),
            response(1, RUN_RESULT),
        ])
        d = _StdoutDispatcher(reader)
        d.start()
        queue = d.subscribe_events("s1")
        future = d.expect_response(1)

        events = []
        # Collect events until the response future resolves
        while not future.done():
            try:
                ev = await asyncio.wait_for(queue.get(), timeout=0.2)
                if ev is not None:
                    events.append(ev)
            except asyncio.TimeoutError:
                pass
        # Drain remaining
        while not queue.empty():
            ev = queue.get_nowait()
            if ev is not None:
                events.append(ev)

        result = await future
        assert len(events) == 3
        assert events[0]["type"] == "turn_started"
        assert events[1]["type"] == "text_delta"
        assert events[2]["type"] == "text_complete"
        assert result["session_id"] == "s1"
        await d.stop()

    @pytest.mark.asyncio
    async def test_eof_fails_pending_futures(self):
        reader = make_reader([])  # immediate EOF
        d = _StdoutDispatcher(reader)
        d.start()
        future = d.expect_response(1)
        with pytest.raises(MeerkatError, match="rpc process closed"):
            await asyncio.wait_for(future, timeout=1.0)
        await d.stop()

    @pytest.mark.asyncio
    async def test_eof_sends_sentinel_to_queues(self):
        reader = make_reader([])
        d = _StdoutDispatcher(reader)
        d.start()
        queue = d.subscribe_events("s1")
        sentinel = await asyncio.wait_for(queue.get(), timeout=1.0)
        assert sentinel is None
        await d.stop()

    @pytest.mark.asyncio
    async def test_catchall_captures_unknown_session(self):
        """Catchall captures events before session_id is known, promotes on response."""
        ev1 = {"type": "run_started", "session_id": "new-id", "prompt": "hi"}
        ev2 = {"type": "text_delta", "delta": "hello"}
        reader = make_reader([
            event_notification("new-id", ev1),
            event_notification("new-id", ev2),
            response(1, {"session_id": "new-id", "text": "done"}),
        ])
        d = _StdoutDispatcher(reader)
        d.start()
        catchall = d.subscribe_catchall(request_id=1)
        _ = d.expect_response(1)
        # Events arrive via catchall before response
        e1 = await asyncio.wait_for(catchall.get(), timeout=1.0)
        assert e1["type"] == "run_started"
        e2 = await asyncio.wait_for(catchall.get(), timeout=1.0)
        assert e2["type"] == "text_delta"
        # After response, catchall is promoted to session "new-id"
        await asyncio.sleep(0.05)  # let dispatcher process response
        assert d._catchall_queue is None
        await d.stop()

    @pytest.mark.asyncio
    async def test_multiple_sessions_routed_independently(self):
        reader = make_reader([
            event_notification("s1", {"type": "text_delta", "delta": "A"}),
            event_notification("s2", {"type": "text_delta", "delta": "B"}),
        ])
        d = _StdoutDispatcher(reader)
        d.start()
        q1 = d.subscribe_events("s1")
        q2 = d.subscribe_events("s2")
        e1 = await asyncio.wait_for(q1.get(), timeout=1.0)
        e2 = await asyncio.wait_for(q2.get(), timeout=1.0)
        assert e1["delta"] == "A"
        assert e2["delta"] == "B"
        await d.stop()

    @pytest.mark.asyncio
    async def test_unsubscribe_removes_queue(self):
        reader = make_reader([
            event_notification("s1", {"type": "text_delta", "delta": "A"}),
            response(1, {}),
        ])
        d = _StdoutDispatcher(reader)
        d.start()
        queue = d.subscribe_events("s1")
        d.unsubscribe_events("s1")
        assert "s1" not in d._event_queues
        # Event should be dropped, response still works
        future = d.expect_response(1)
        result = await asyncio.wait_for(future, timeout=1.0)
        assert result == {}
        await d.stop()

    @pytest.mark.asyncio
    async def test_multiple_responses_dispatched_correctly(self):
        reader = make_reader([
            response(2, {"b": True}),
            response(1, {"a": True}),
        ])
        d = _StdoutDispatcher(reader)
        d.start()
        f1 = d.expect_response(1)
        f2 = d.expect_response(2)
        r2 = await asyncio.wait_for(f2, timeout=1.0)
        r1 = await asyncio.wait_for(f1, timeout=1.0)
        assert r1 == {"a": True}
        assert r2 == {"b": True}
        await d.stop()


# ===========================================================================
# StreamingTurn tests
# ===========================================================================

class TestStreamingTurn:

    @pytest.mark.asyncio
    async def test_iterate_events_and_access_result(self):
        reader = make_reader([
            event_notification("s1", {"type": "turn_started", "turn_number": 0}),
            event_notification("s1", {"type": "text_delta", "delta": "Hi"}),
            response(1, RUN_RESULT),
        ])
        d = _StdoutDispatcher(reader)
        d.start()
        queue = d.subscribe_events("s1")
        future = d.expect_response(1)

        stream = StreamingTurn(
            session_id="s1",
            event_queue=queue,
            response_future=future,
            dispatcher=d,
            parse_result=_mock_parse_result,
        )

        events = []
        async with stream:
            async for event in stream:
                events.append(event)
            result = stream.result

        assert len(events) == 2
        assert events[0]["type"] == "turn_started"
        assert events[1]["type"] == "text_delta"
        assert result.session_id == "s1"
        assert result.text == "Hello!"
        await d.stop()

    @pytest.mark.asyncio
    async def test_result_raises_before_iteration(self):
        reader = make_reader([response(1, RUN_RESULT)])
        d = _StdoutDispatcher(reader)
        d.start()
        queue = d.subscribe_events("s1")
        future = d.expect_response(1)

        stream = StreamingTurn(
            session_id="s1",
            event_queue=queue,
            response_future=future,
            dispatcher=d,
            parse_result=_mock_parse_result,
        )

        with pytest.raises(MeerkatError, match="Iterate the stream"):
            _ = stream.result

        await d.stop()

    @pytest.mark.asyncio
    async def test_collect_returns_result(self):
        reader = make_reader([
            event_notification("s1", {"type": "text_delta", "delta": "yo"}),
            response(1, RUN_RESULT),
        ])
        d = _StdoutDispatcher(reader)
        d.start()
        queue = d.subscribe_events("s1")
        future = d.expect_response(1)

        stream = StreamingTurn(
            session_id="s1",
            event_queue=queue,
            response_future=future,
            dispatcher=d,
            parse_result=_mock_parse_result,
        )

        async with stream:
            result = await stream.collect()

        assert result.text == "Hello!"
        await d.stop()

    @pytest.mark.asyncio
    async def test_collect_text_accumulates_deltas(self):
        reader = make_reader([
            event_notification("s1", {"type": "text_delta", "delta": "Hel"}),
            event_notification("s1", {"type": "text_delta", "delta": "lo!"}),
            response(1, RUN_RESULT),
        ])
        d = _StdoutDispatcher(reader)
        d.start()
        queue = d.subscribe_events("s1")
        future = d.expect_response(1)

        stream = StreamingTurn(
            session_id="s1",
            event_queue=queue,
            response_future=future,
            dispatcher=d,
            parse_result=_mock_parse_result,
        )

        async with stream:
            text, result = await stream.collect_text()

        assert text == "Hel" + "lo!"
        assert result.text == "Hello!"
        await d.stop()

    @pytest.mark.asyncio
    async def test_context_manager_unsubscribes(self):
        reader = make_reader([response(1, RUN_RESULT)])
        d = _StdoutDispatcher(reader)
        d.start()
        queue = d.subscribe_events("s1")
        future = d.expect_response(1)

        stream = StreamingTurn(
            session_id="s1",
            event_queue=queue,
            response_future=future,
            dispatcher=d,
            parse_result=_mock_parse_result,
        )

        async with stream:
            _ = await stream.collect()

        assert "s1" not in d._event_queues
        await d.stop()

    @pytest.mark.asyncio
    async def test_no_events_before_response(self):
        """Response arrives with no events — iteration ends immediately."""
        reader = make_reader([response(1, RUN_RESULT)])
        d = _StdoutDispatcher(reader)
        d.start()
        queue = d.subscribe_events("s1")
        future = d.expect_response(1)

        stream = StreamingTurn(
            session_id="s1",
            event_queue=queue,
            response_future=future,
            dispatcher=d,
            parse_result=_mock_parse_result,
        )

        events = []
        async with stream:
            async for event in stream:
                events.append(event)

        assert events == []
        assert stream.result.session_id == "s1"
        await d.stop()


# ---------------------------------------------------------------------------
# Test helpers
# ---------------------------------------------------------------------------

def _mock_parse_result(data: dict):
    """Minimal parser that returns a WireRunResult-like object."""
    from meerkat.generated.types import WireRunResult, WireUsage
    usage_data = data.get("usage", {})
    return WireRunResult(
        session_id=data.get("session_id", ""),
        text=data.get("text", ""),
        turns=data.get("turns", 0),
        tool_calls=data.get("tool_calls", 0),
        usage=WireUsage(
            input_tokens=usage_data.get("input_tokens", 0),
            output_tokens=usage_data.get("output_tokens", 0),
            total_tokens=usage_data.get("total_tokens", 0),
        ),
    )
