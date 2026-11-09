"""Mock-RPC tests for the structural mob events SDK surface."""

from unittest.mock import MagicMock

import pytest


def make_mock_mob_handle(rpc_responses=None):
    from meerkat_meta.runtime import MobHandle

    runtime = MagicMock()
    calls = []

    async def mock_rpc(method, params=None):
        calls.append((method, params))
        if rpc_responses and method in rpc_responses:
            return rpc_responses[method]
        return {}

    runtime._rpc = mock_rpc
    handle = MobHandle.__new__(MobHandle)
    handle._runtime = runtime
    return handle, calls


@pytest.mark.asyncio
async def test_query_mob_events_uses_correct_method_and_returns_typed():
    handle, calls = make_mock_mob_handle({
        "meta/mob_events/query": {
            "events": [
                {
                    "event_id": "mob-evt-1",
                    "cursor": 1,
                    "mob_id": "mob-A",
                    "timestamp_ms": 100,
                    "kind": "flow_started",
                    "run_id": "run-x",
                    "step_id": None,
                    "agent_identity": None,
                    "data": {"type": "flow_started", "run_id": "run-x"},
                }
            ],
            "next_after_seq": 1,
        }
    })
    from meerkat_meta import EventQuery, MobStructuralEvent

    result = await handle.query_mob_events(EventQuery(mob_id="mob-A", run_id="run-x"))
    assert calls[0][0] == "meta/mob_events/query"
    assert calls[0][1]["mob_id"] == "mob-A"
    assert calls[0][1]["run_id"] == "run-x"
    assert len(result) == 1
    assert isinstance(result[0], MobStructuralEvent)
    assert result[0].kind == "flow_started"
    assert result[0].cursor == 1
    assert result[0].run_id == "run-x"


@pytest.mark.asyncio
async def test_query_mob_events_accepts_dict_query():
    handle, calls = make_mock_mob_handle({
        "meta/mob_events/query": {"events": [], "next_after_seq": None}
    })
    result = await handle.query_mob_events({"mob_id": "mob-A"})
    assert calls[0][1] == {"mob_id": "mob-A"}
    assert result == []


@pytest.mark.asyncio
async def test_query_mob_events_default_empty_params():
    handle, calls = make_mock_mob_handle({
        "meta/mob_events/query": {"events": []}
    })
    await handle.query_mob_events()
    assert calls[0][1] == {}


@pytest.mark.asyncio
async def test_subscribe_mob_events_yields_typed_envelopes():
    handle, _ = make_mock_mob_handle({
        "meta/mob_events/subscribe": {
            "stream": "mob_events",
            "events": [
                {
                    "event_id": "mob-evt-1",
                    "cursor": 1,
                    "mob_id": "mob-A",
                    "timestamp_ms": 1,
                    "kind": "flow_started",
                    "run_id": "run-x",
                    "step_id": None,
                    "agent_identity": None,
                    "data": {},
                },
                {
                    "event_id": "mob-evt-2",
                    "cursor": 2,
                    "mob_id": "mob-A",
                    "timestamp_ms": 2,
                    "kind": "step_dispatched",
                    "run_id": "run-x",
                    "step_id": "step-1",
                    "agent_identity": "worker-1",
                    "data": {},
                },
            ],
            "next_after_seq": 2,
        }
    })
    from meerkat_meta import EventQuery, MobStructuralEvent

    received = []
    async for event in handle.subscribe_mob_events(EventQuery(mob_id="mob-A")):
        received.append(event)
    assert len(received) == 2
    assert all(isinstance(e, MobStructuralEvent) for e in received)
    assert received[0].kind == "flow_started"
    assert received[1].step_id == "step-1"
    assert received[1].agent_identity == "worker-1"


@pytest.mark.asyncio
async def test_event_query_to_dict_includes_new_filters():
    from meerkat_meta import EventQuery

    query = EventQuery(
        mob_id="mob-A",
        run_id="run-x",
        step_id="step-1",
        identity="worker-1",
        after_seq=42,
    )
    payload = query.to_dict()
    assert payload["mob_id"] == "mob-A"
    assert payload["run_id"] == "run-x"
    assert payload["step_id"] == "step-1"
    assert payload["identity"] == "worker-1"
    assert payload["after_seq"] == 42


def test_mob_structural_event_from_dict_handles_optional_fields():
    from meerkat_meta import MobStructuralEvent

    event = MobStructuralEvent.from_dict({
        "event_id": "mob-evt-1",
        "cursor": 7,
        "mob_id": "mob-A",
        "timestamp_ms": 10,
        "kind": "members_wired",
        "data": {"type": "members_wired", "a": "x", "b": "y"},
    })
    assert event.event_id == "mob-evt-1"
    assert event.cursor == 7
    assert event.run_id is None
    assert event.step_id is None
    assert event.agent_identity is None
    assert event.data["a"] == "x"
