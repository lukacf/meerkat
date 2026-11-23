"""Terminal usage comes from the Meta SSE payload, never reconstructed totals.

The payload fields follow meerkat-core's serialized usage on the two terminal
events, AgentEvent::{RunCompleted,TurnCompleted} with Usage/TurnUsage. Meta's
console_agent_event_payload forwards those usage objects unchanged.
"""

import dataclasses
import json
import logging

import pytest

import meerkat_meta
from meerkat_meta._sse import SseEvent
from meerkat_meta.events import (
    AgentEvent,
    MobEvent,
    RunCompleted,
    TextDelta,
    TurnCompleted,
    parse_agent_event,
)


def measured_usage():
    return {
        "input_tokens": 300,
        "output_tokens": 40,
        "cache_creation_tokens": 100,
        "cache_read_tokens": 200,
        "reasoning_tokens": 10,
        "accounting": {
            "provider": "anthropic",
            "model": "fixture-model",
            "presented_tokens": 600,
            "convention": "anthropic_disjoint_input_components",
            "aggregation": "sum_disjoint_provider_components",
        },
    }


def terminal(kind, **fields):
    common = (
        {"session_id": "session-1", "result": "done", "structured_output": {"ok": True}}
        if kind == "run_completed"
        else {"stop_reason": "end_turn"}
    )
    return {"type": kind, **common, **fields}


@pytest.mark.parametrize("kind", ["turn_completed", "run_completed"])
def test_terminal_usage_is_a_typed_payload_projection(kind):
    raw = {"input_tokens": 600, "output_tokens": 40,
           "cache_creation_tokens": None, "cache_read_tokens": 200,
           "reasoning_tokens": 10}
    event = parse_agent_event(terminal(kind, usage=raw))
    usage = getattr(event, "usage", None)
    assert usage is not None, "the terminal payload's usage must not be discarded"
    assert dataclasses.is_dataclass(usage)
    assert isinstance(usage, meerkat_meta.Usage)
    assert usage.input_tokens == 600
    assert usage.output_tokens == 40
    assert usage.cache_creation_tokens is None
    assert usage.cache_read_tokens == 200
    assert usage.reasoning_tokens == 10
    assert usage.accounting is None
    assert not hasattr(usage, "total_tokens"), "these events carry no total_tokens field"
    if isinstance(event, RunCompleted):
        assert event.result == "done"
        assert event.structured_output == {"ok": True}
    else:
        assert isinstance(event, TurnCompleted)
        assert event.stop_reason == "end_turn"


def test_turn_usage_retains_provider_accounting_without_normalizing_raw_input():
    event = parse_agent_event(terminal("turn_completed", usage=measured_usage()))
    usage = getattr(event, "usage", None)
    assert usage is not None, "the measured turn's usage must not be discarded"
    assert usage.input_tokens == 300
    assert usage.accounting.presented_tokens == 600
    assert isinstance(usage.accounting, meerkat_meta.ProviderTokenAccounting)
    assert dataclasses.asdict(usage.accounting) == measured_usage()["accounting"]
    with pytest.raises(dataclasses.FrozenInstanceError):
        usage.input_tokens = 1


@pytest.mark.parametrize("kind", ["turn_completed", "run_completed"])
@pytest.mark.parametrize("fields", [{}, {"usage": None}])
def test_missing_or_null_usage_is_not_a_zero_measurement(kind, fields):
    event = parse_agent_event(terminal(kind, **fields))
    assert event.usage is None


@pytest.mark.parametrize("kind", ["turn_completed", "run_completed"])
def test_explicit_zero_counters_and_absent_details_stay_distinct(kind):
    event = parse_agent_event(terminal(kind, usage={"input_tokens": 0, "output_tokens": 0}))
    assert event.usage is not None
    assert event.usage.input_tokens == event.usage.output_tokens == 0
    assert event.usage.cache_creation_tokens is None
    assert event.usage.cache_read_tokens is None
    assert event.usage.reasoning_tokens is None
    assert event.usage.accounting is None


@pytest.mark.parametrize("kind", ["turn_completed", "run_completed"])
def test_explicit_zero_detail_counters_are_preserved(kind):
    event = parse_agent_event(terminal(kind, usage={
        "input_tokens": 0, "output_tokens": 0, "cache_creation_tokens": 0,
        "cache_read_tokens": 0, "reasoning_tokens": 0,
    }))
    assert event.usage.cache_creation_tokens == 0
    assert event.usage.cache_read_tokens == 0
    assert event.usage.reasoning_tokens == 0


@pytest.mark.parametrize("kind", ["turn_completed", "run_completed"])
@pytest.mark.parametrize("wrapper", ["agent", "mob"])
def test_terminal_usage_survives_both_sse_wrappers(kind, wrapper):
    raw = terminal(kind, usage={"input_tokens": 600, "output_tokens": 40})
    if wrapper == "agent":
        envelope = AgentEvent.from_sse(SseEvent(event=kind, data=json.dumps(raw)))
    else:
        envelope = MobEvent.from_sse(SseEvent(event=kind, data=json.dumps({
            "member_id": "agent-1", "payload": raw, "timestamp_ms": 7,
        })))
        assert envelope.member_id == "agent-1"
        assert envelope.timestamp_ms == 7
    assert envelope.event.usage.input_tokens == 600
    assert envelope.event.usage.output_tokens == 40


MALFORMED = [
    {}, [], "unknown", False,
    {"input_tokens": 1},
    {"input_tokens": 1, "output_tokens": "2"},
    {"input_tokens": True, "output_tokens": 2},
    {"input_tokens": -1, "output_tokens": 2},
    {"input_tokens": 1, "output_tokens": 2, "cache_read_tokens": "0"},
    {"input_tokens": 1, "output_tokens": 2, "accounting": {}},
]


def said_by_the_sdk(caplog):
    return [record.getMessage() for record in caplog.records if record.name == "meerkat_meta"]


@pytest.mark.parametrize("kind", ["turn_completed", "run_completed"])
@pytest.mark.parametrize("raw", MALFORMED)
def test_malformed_usage_makes_no_counters_and_loses_no_event(kind, raw, caplog):
    with caplog.at_level(logging.WARNING, logger="meerkat_meta"):
        event = parse_agent_event(terminal(kind, usage=raw))
    assert event.usage is None
    if kind == "run_completed":
        assert isinstance(event, RunCompleted)
        assert (event.session_id, event.result, event.structured_output) == (
            "session-1", "done", {"ok": True},
        )
    else:
        assert isinstance(event, TurnCompleted)
        assert event.stop_reason == "end_turn"
    said = said_by_the_sdk(caplog)
    assert len(said) == 1, "the usage left out is said once, not silently"
    assert kind in said[0] and "usage" in said[0]


@pytest.mark.parametrize("kind", ["turn_completed", "run_completed"])
@pytest.mark.parametrize("wrapper", ["agent", "mob"])
def test_a_malformed_usage_does_not_end_a_stream(kind, wrapper, caplog):
    raw = terminal(kind, usage={"input_tokens": 1})
    with caplog.at_level(logging.WARNING, logger="meerkat_meta"):
        if wrapper == "agent":
            envelope = AgentEvent.from_sse(SseEvent(event=kind, data=json.dumps(raw)))
        else:
            envelope = MobEvent.from_sse(SseEvent(event=kind, data=json.dumps({
                "member_id": "agent-1", "payload": raw, "timestamp_ms": 7,
            })))
    assert isinstance(envelope.event, RunCompleted if kind == "run_completed" else TurnCompleted)
    assert envelope.event.usage is None
    assert len(said_by_the_sdk(caplog)) == 1


@pytest.mark.parametrize("fields", [
    {}, {"usage": None}, {"usage": {"input_tokens": 0, "output_tokens": 0}},
])
def test_a_fitting_or_missing_usage_says_nothing(fields, caplog):
    with caplog.at_level(logging.WARNING, logger="meerkat_meta"):
        parse_agent_event(terminal("turn_completed", **fields))
        parse_agent_event(terminal("run_completed", **fields))
    assert said_by_the_sdk(caplog) == []


def test_existing_positional_construction_and_unrelated_events_still_work():
    run = RunCompleted("session-1", "done", {"ok": True})
    turn = TurnCompleted("end_turn")
    assert (run.session_id, run.result, run.structured_output) == (
        "session-1", "done", {"ok": True},
    )
    assert run.__match_args__[:3] == ("session_id", "result", "structured_output")
    assert turn.__match_args__[:1] == ("stop_reason",)
    assert parse_agent_event({"type": "text_delta", "delta": "hello"}) == TextDelta("hello")
