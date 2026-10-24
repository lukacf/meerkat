import asyncio
from pathlib import Path

import pytest


FAKE_RPC = Path(__file__).resolve().parent / "fixtures" / "fake_stream_rpc.mjs"


@pytest.mark.asyncio
async def test_python_sdk_buffered_stream_flushes_in_order_and_remote_end_is_visible():
    from meerkat import MeerkatClient

    async with MeerkatClient(str(FAKE_RPC)) as client:
        sub = await client.subscribe_session_events("buffered-session")
        deltas = []
        async for event in sub:
            deltas.append(getattr(event.payload, "delta", None))

        assert deltas == ["hi", "there"]
        assert sub.terminal_outcome is not None
        assert sub.terminal_outcome.get("outcome") == "remote_end"


@pytest.mark.asyncio
async def test_python_sdk_terminal_error_stream_is_visible_without_manual_close():
    from meerkat import EventSubscriptionError, MeerkatClient, MeerkatError

    async with MeerkatClient(str(FAKE_RPC)) as client:
        sub = await client.subscribe_session_events("terminal-error-session")
        events = []
        with pytest.raises(EventSubscriptionError) as failure:
            async for event in sub:
                events.append(event)

        assert events == []
        assert isinstance(failure.value, MeerkatError)
        assert failure.value.code == "stream_queue_overflow"
        assert failure.value.last_rpc_sequence is None
        assert failure.value.last_event_seq is None
        assert sub.terminal_outcome is not None
        assert sub.terminal_outcome.get("outcome") == "terminal_error"
        assert sub.terminal_outcome.get("error", {}).get("code") == "stream_queue_overflow"


async def open_subscription(client, scope, scenario):
    if scope == "session":
        return await client.subscribe_session_events(scenario)
    if scope == "member":
        return await client.subscribe_mob_member_events(scenario, "synthetic-agent")
    return await client.subscribe_mob_events(scenario)


@pytest.mark.asyncio
@pytest.mark.parametrize("scope", ["session", "mob", "member"])
@pytest.mark.parametrize("scenario,code,message,outcome", [
    ("queued-terminal-error", "stream_queue_overflow", "synthetic stream overflow", "terminal_error"),
    ("queued-unknown-error", "future_stream_code", "future detail: preserve exactly", "future_terminal_outcome"),
    ("live-terminal-error", "stream_queue_overflow", "synthetic stream overflow", "terminal_error"),
])
async def test_subscription_drains_events_before_typed_terminal_error(scope, scenario, code, message, outcome):
    from meerkat import EventSubscriptionError, MeerkatClient, MeerkatError

    async with MeerkatClient(str(FAKE_RPC)) as client:
        sub = await open_subscription(client, scope, scenario)
        assert sub.last_rpc_sequence is None
        assert sub.last_event_seq is None
        iterator = sub.__aiter__()
        first = await anext(iterator)
        first_envelope = first.envelope if scope == "mob" else first
        assert first_envelope.payload.delta == "first"
        assert (sub.last_rpc_sequence, sub.last_event_seq) == (7, 41)
        second = await anext(iterator)
        second_envelope = second.envelope if scope == "mob" else second
        assert second_envelope.payload.delta == "second"
        assert (sub.last_rpc_sequence, sub.last_event_seq) == (8, 42)
        with pytest.raises(EventSubscriptionError) as failure:
            await anext(iterator)
        error = failure.value
        assert isinstance(error, MeerkatError)
        assert (error.code, error.message) == (code, message)
        assert error.stream_id == sub.stream_id
        assert (error.last_rpc_sequence, error.last_event_seq) == (8, 42)
        assert error.terminal_outcome == sub.terminal_outcome
        assert error.terminal_outcome["outcome"] == outcome
        assert error.terminal_outcome["error"] == {"code": code, "message": message}
        assert error.details == {"stream_id": sub.stream_id, "terminal_outcome": sub.terminal_outcome,
                                 "last_rpc_sequence": 8, "last_event_seq": 42}
        with pytest.raises(StopAsyncIteration):
            await anext(iterator)


@pytest.mark.asyncio
@pytest.mark.parametrize("scope", ["session", "mob", "member"])
async def test_gap_marker_preserves_last_canonical_event_sequence_on_terminal_error(scope):
    from meerkat import EventSubscriptionError, MeerkatClient
    from meerkat.events import StreamTruncated

    async with MeerkatClient(str(FAKE_RPC)) as client:
        sub = await open_subscription(client, scope, "queued-gap-terminal-error")
        iterator = sub.__aiter__()
        first = await anext(iterator)
        first_envelope = first.envelope if scope == "mob" else first
        assert first_envelope.seq == 41
        assert (sub.last_rpc_sequence, sub.last_event_seq) == (7, 41)
        marker = await anext(iterator)
        marker_envelope = marker.envelope if scope == "mob" else marker
        assert marker_envelope.seq == 0
        assert isinstance(marker_envelope.payload, StreamTruncated)
        assert marker_envelope.payload.reason.dropped == 1
        assert (sub.last_rpc_sequence, sub.last_event_seq) == (9, 41)
        with pytest.raises(EventSubscriptionError) as failure:
            await anext(iterator)
        error = failure.value
        assert error.code == "stream_queue_overflow"
        assert (error.last_rpc_sequence, error.last_event_seq) == (9, 41)
        assert error.details["last_rpc_sequence"] == 9
        assert error.details["last_event_seq"] == 41
        assert error.terminal_outcome == sub.terminal_outcome


@pytest.mark.asyncio
async def test_received_terminal_error_survives_dispatcher_transport_cleanup():
    from meerkat import EventSubscriptionError
    from meerkat.event_envelope import parse_agent_event_envelope
    from meerkat.streaming import EventSubscription, _StdoutDispatcher
    from tests.test_streaming import agent_event_envelope, jline, make_reader

    terminal = {"stream_id": "stream-1", "ended": True, "outcome": "terminal_error",
                "error": {"code": "future_stream_code", "message": "exact terminal detail"}}
    envelope = agent_event_envelope({"type": "text_delta", "delta": "before EOF"})
    reader = make_reader([
        jline({"jsonrpc": "2.0", "method": "session/stream_event", "params": {
            "stream_id": "stream-1", "sequence": 19, "event": envelope}}),
        jline({"jsonrpc": "2.0", "method": "session/stream_end", "params": terminal}),
    ])
    dispatcher = _StdoutDispatcher(reader)
    sub = EventSubscription(
        stream_id="stream-1", event_queue=dispatcher.subscribe_stream("stream-1"),
        dispatcher=dispatcher, close_remote=lambda _: asyncio.sleep(0),
        parse_event_fn=parse_agent_event_envelope,
    )
    dispatcher.start()
    assert await dispatcher.wait_read_loop_done(timeout=1)
    assert dispatcher.stream_terminal("stream-1") is None
    iterator = sub.__aiter__()
    assert (await anext(iterator)).payload.delta == "before EOF"
    with pytest.raises(EventSubscriptionError) as failure:
        await anext(iterator)
    assert failure.value.terminal_outcome == terminal
    assert sub.terminal_outcome == terminal
    assert (failure.value.last_rpc_sequence, failure.value.last_event_seq) == (19, envelope["seq"])


@pytest.mark.asyncio
@pytest.mark.parametrize("scope", ["session", "mob", "member"])
async def test_open_response_then_eof_retains_events_and_primary_terminal_error(scope):
    import json
    from types import SimpleNamespace

    from meerkat import EventSubscriptionError, MeerkatClient
    from meerkat.streaming import _StdoutDispatcher
    from tests.test_streaming import agent_event_envelope, jline, make_reader

    notification = "session" if scope == "session" else "mob"
    terminal = {"stream_id": "stream-1", "ended": True, "outcome": "terminal_error",
                "error": {"code": "future_stream_code", "message": "retained before EOF"}}
    envelope = agent_event_envelope({"type": "text_delta", "delta": "queued before open resumes"})
    event = envelope if scope != "mob" else {
        "source": {"identity": "synthetic-agent", "generation": 0},
        "role": "synthetic-player", "envelope": envelope,
    }
    reader = make_reader([
        jline({"jsonrpc": "2.0", "id": 1, "result": {"stream_id": "stream-1"}}),
        jline({"jsonrpc": "2.0", "method": f"{notification}/stream_event", "params": {
            "stream_id": "stream-1", "sequence": 19, "event": event}}),
        jline({"jsonrpc": "2.0", "method": f"{notification}/stream_end", "params": terminal}),
    ])
    written = asyncio.Event()

    class Writer:
        def write(self, data):
            assert json.loads(data)["method"] == f"{notification}/stream_open"
            written.set()

        async def drain(self):
            pass

    client = MeerkatClient()
    dispatcher = _StdoutDispatcher(reader)
    client._dispatcher = dispatcher
    client._process = SimpleNamespace(stdin=Writer())
    opening = asyncio.create_task(open_subscription(client, scope, "synthetic-target"))
    await asyncio.wait_for(written.wait(), 1)
    # Every buffered readline completes without suspension. The open response
    # resolves first, but EOF cleanup runs before the real opener can register.
    dispatcher.start()
    assert await dispatcher.wait_read_loop_done(timeout=1)
    assert dispatcher.transport_fault.code == "CONNECTION_CLOSED"
    sub = await asyncio.wait_for(opening, 1)
    assert sub.last_rpc_sequence is None
    assert "stream-1" not in dispatcher._stream_queues
    with pytest.raises(EventSubscriptionError) as failure:
        async with sub:
            iterator = sub.__aiter__()
            first = await asyncio.wait_for(anext(iterator), 1)
            first_envelope = first.envelope if scope == "mob" else first
            assert first_envelope.payload.delta == "queued before open resumes"
            await asyncio.wait_for(anext(iterator), 1)
    assert failure.value.terminal_outcome == terminal
    assert sub.terminal_outcome == terminal
    assert (failure.value.last_rpc_sequence, failure.value.last_event_seq) == (19, envelope["seq"])
    # __aexit__ attempted authoritative close, which failed on the dead
    # transport. Its CONNECTION_CLOSED must not replace the received error.
    assert sub._closed is False
    assert not dispatcher._unmatched_stream_buffer
    assert not dispatcher._unmatched_stream_end


@pytest.mark.asyncio
async def test_late_subscription_without_terminal_has_eof_after_received_prefix():
    from meerkat.streaming import _StdoutDispatcher, _SubscriptionEvent
    from tests.test_streaming import agent_event_envelope, stream_notification, make_reader

    envelope = agent_event_envelope({"type": "text_delta", "delta": "prefix"})
    dispatcher = _StdoutDispatcher(make_reader([stream_notification("stream-1", envelope)]))
    dispatcher.start()
    assert await dispatcher.wait_read_loop_done(timeout=1)
    queue = dispatcher.subscribe_stream("stream-1")
    assert await asyncio.wait_for(queue.get(), 1) == _SubscriptionEvent(envelope, None)
    assert await asyncio.wait_for(queue.get(), 1) is None
    assert "stream-1" not in dispatcher._stream_queues


@pytest.mark.asyncio
@pytest.mark.parametrize("primary", [ValueError("caller failed"), asyncio.CancelledError(), None])
async def test_subscription_context_cleanup_preserves_primary_failure(primary):
    from types import SimpleNamespace

    from meerkat import EventSubscription, MeerkatError

    cleanup_error = MeerkatError("STREAM_CLOSE_REJECTED", "cleanup refused")
    calls = []

    async def rejecting_close(stream_id):
        calls.append(stream_id)
        raise cleanup_error

    sub = EventSubscription(
        stream_id="stream-1", event_queue=asyncio.Queue(),
        dispatcher=SimpleNamespace(unsubscribe_stream=lambda _: None),
        close_remote=rejecting_close, parse_event_fn=lambda event: event,
    )
    expected = primary if primary is not None else cleanup_error
    with pytest.raises(type(expected)) as failure:
        async with sub:
            if primary is not None:
                raise primary
    assert failure.value is expected
    assert calls == ["stream-1"]
    assert sub._closed is False


@pytest.mark.parametrize("raw", ["not an error record", {}, {"code": 7, "message": False}])
def test_malformed_terminal_error_retains_raw_data(raw):
    from meerkat import EventSubscriptionError, MeerkatError

    terminal = {"stream_id": "stream-1", "outcome": "future_outcome", "error": raw}
    error = EventSubscriptionError("stream-1", terminal, None, None)
    assert isinstance(error, MeerkatError)
    assert error.code == "INVALID_RESPONSE"
    assert error.terminal_outcome == terminal


@pytest.mark.asyncio
@pytest.mark.parametrize("scope", ["session", "mob", "member"])
@pytest.mark.parametrize("outcome", ["remote_end", "explicit_close"])
async def test_subscription_without_terminal_error_ends_normally(scope, outcome):
    from meerkat import MeerkatClient

    scenario = "queued-" + outcome.replace("_", "-")
    async with MeerkatClient(str(FAKE_RPC)) as client:
        sub = await open_subscription(client, scope, scenario)
        events = [event async for event in sub]
        assert len(events) == 2
        assert sub.terminal_outcome["outcome"] == outcome
        assert sub.terminal_outcome.get("error") is None
        assert (sub.last_rpc_sequence, sub.last_event_seq) == (8, 42)
