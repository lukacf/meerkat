"""Tests for typed event construction and parsing."""
import json

import pytest

from meerkat_meta._sse import SseEvent
from meerkat_meta.events import (
    AgentEvent,
    Event,
    EventStream,
    MobEvent,
    RunCompleted,
    RunFailed,
    RunStarted,
    TextDelta,
    ToolCallRequested,
    ToolExecutionCompleted,
    ToolResultReceived,
    UnknownEvent,
    parse_agent_event,
)


class TestParseAgentEvent:
    def test_text_delta(self):
        ev = parse_agent_event({"type": "text_delta", "delta": "hello"})
        assert isinstance(ev, TextDelta)
        assert ev.delta == "hello"

    def test_run_started(self):
        ev = parse_agent_event({"type": "run_started", "session_id": "s1", "prompt": "hi"})
        assert isinstance(ev, RunStarted)
        assert ev.session_id == "s1"
        assert ev.prompt == "hi"

    def test_run_completed(self):
        ev = parse_agent_event({"type": "run_completed", "session_id": "s1", "result": "done"})
        assert isinstance(ev, RunCompleted)
        assert ev.result == "done"

    def test_run_failed_derives_error_from_meerkat_error_report(self):
        # Wire shape of meerkat-core 0.8.32 `AgentEvent::RunFailed`: the typed
        # `error_report` is the only failure truth; there is no flat `error`.
        raw = {
            "type": "run_failed",
            "session_id": "01a068a0-f911-7331-8187-d436145ac895",
            "error_report": {
                "class": "llm",
                "reason": {"reason_type": "llm_auth_error"},
                "message": "LLM error: authentication failed (401)",
            },
        }
        ev = parse_agent_event(raw)
        assert isinstance(ev, RunFailed)
        assert ev.error == "LLM error: authentication failed (401)"
        assert ev.error_report == raw["error_report"]
        assert ev.error_class == "llm"
        assert ev.reason_type == "llm_auth_error"

    def test_run_failed_keeps_a_flat_error_when_the_wire_carries_one(self):
        ev = parse_agent_event({"type": "run_failed", "session_id": "s1", "error": "boom"})
        assert isinstance(ev, RunFailed)
        assert ev.error == "boom"
        assert ev.error_report == {}
        assert ev.error_class == ""
        assert ev.reason_type == ""

    def test_run_failed_without_typed_reason_has_empty_reason_type(self):
        ev = parse_agent_event({
            "type": "run_failed",
            "session_id": "s1",
            "error_report": {"class": "internal", "message": "runner gave up"},
        })
        assert isinstance(ev, RunFailed)
        assert ev.error == "runner gave up"
        assert ev.error_class == "internal"
        assert ev.reason_type == ""

    def test_tool_call_requested(self):
        ev = parse_agent_event({
            "type": "tool_call_requested",
            "id": "tc-1",
            "name": "search",
            "args": {"query": "test"},
        })
        assert isinstance(ev, ToolCallRequested)
        assert ev.name == "search"
        assert ev.args == {"query": "test"}

    def test_tool_result_received_carries_the_tool_output(self):
        ev = parse_agent_event({
            "type": "tool_result_received",
            "id": "tc-1",
            "tool_call_id": "tc-1",
            "name": "search",
            "content": [{"type": "text", "text": "found"}],
            "result": "found",
            "is_error": False,
        })
        assert isinstance(ev, ToolResultReceived)
        assert (ev.id, ev.name, ev.result, ev.is_error) == ("tc-1", "search", "found", False)
        # Earlier positional patterns keep their meaning; the output is last.
        match ev:
            case ToolResultReceived("tc-1", "search", False, output):
                assert output == "found"
            case _:
                pytest.fail("ToolResultReceived positional pattern did not match")

    def test_tool_result_received_without_result_reads_empty(self):
        ev = parse_agent_event({"type": "tool_result_received", "id": "tc-1", "name": "search",
                                "is_error": False})
        assert isinstance(ev, ToolResultReceived)
        assert ev.result == ""

    def test_tool_execution_completed_carries_no_output(self):
        ev = parse_agent_event({
            "type": "tool_execution_completed",
            "id": "tc-1",
            "tool_call_id": "tc-1",
            "name": "search",
            "is_error": False,
            "duration_ms": 12,
        })
        assert isinstance(ev, ToolExecutionCompleted)
        assert (ev.id, ev.name, ev.is_error, ev.duration_ms) == ("tc-1", "search", False, 12)
        assert not hasattr(ev, "result")

    def test_tool_execution_completed_from_an_older_server_ignores_its_result(self):
        ev = parse_agent_event({
            "type": "tool_execution_completed",
            "id": "tc-1",
            "name": "search",
            "content": [{"type": "text", "text": "found"}],
            "result": "found",
            "is_error": False,
            "duration_ms": 12,
        })
        assert isinstance(ev, ToolExecutionCompleted)
        assert (ev.id, ev.is_error, ev.duration_ms) == ("tc-1", False, 12)
        assert not hasattr(ev, "result")

    def test_unknown_event_type(self):
        ev = parse_agent_event({"type": "future_event", "foo": "bar"})
        assert isinstance(ev, UnknownEvent)
        assert ev.type == "future_event"
        assert ev.data == {"type": "future_event", "foo": "bar"}

    def test_fallback_resume_hold_keeps_typed_reason(self):
        reason = {
            "reason_type": "model_fallback_resume_held",
            "provider": "openai",
            "model": "gpt-5.5",
            "reason": "context_fit",
        }
        event = parse_agent_event({
            "type": "run_failed",
            "session_id": "session-held",
            "error_report": {"class": "config", "reason": reason, "message": "held"},
        })
        assert isinstance(event, RunFailed)
        assert event.reason_type == "model_fallback_resume_held"
        assert event.error_report["reason"] == reason

    def test_fallback_observations_preserve_the_open_event_payload(self):
        for kind in (
            "model_fallback_skipped", "model_fallback_staged",
            "model_fallback_committed", "model_fallback_target_failed",
        ):
            raw = {
                "type": kind,
                "target": {"provider": "openai", "model": "gpt-5.5"},
                "retry": {"plan": {"attempt": 3}},
            }
            event = parse_agent_event(raw)
            assert isinstance(event, UnknownEvent)
            assert event.type == kind
            assert event.data == raw

    def test_all_events_are_event_subclass(self):
        ev = parse_agent_event({"type": "text_delta", "delta": "x"})
        assert isinstance(ev, Event)
        ev2 = parse_agent_event({"type": "unknown_thing"})
        assert isinstance(ev2, Event)


class TestPatternMatching:
    def test_match_text_delta(self):
        ev = parse_agent_event({"type": "text_delta", "delta": "chunk"})
        match ev:
            case TextDelta(delta=d):
                assert d == "chunk"
            case _:
                raise AssertionError("should match TextDelta")

    def test_match_run_completed(self):
        ev = parse_agent_event({"type": "run_completed", "result": "done"})
        match ev:
            case RunCompleted(result=r):
                assert r == "done"
            case _:
                raise AssertionError("should match RunCompleted")

    def test_match_run_failed_error_sees_the_derived_message(self):
        ev = parse_agent_event({
            "type": "run_failed",
            "session_id": "s1",
            "error_report": {"class": "llm", "message": "rate limited"},
        })
        match ev:
            case RunFailed(error=err):
                assert err == "rate limited"
            case _:
                raise AssertionError("should match RunFailed")


class TestAgentEvent:
    def test_from_sse_typed(self):
        payload = json.dumps({"type": "text_delta", "delta": "hello"})
        sse = SseEvent(id="ev-1", event="text_delta", data=payload)
        ev = AgentEvent.from_sse(sse, agent_id="agent-1")
        assert ev.event_type == "text_delta"
        assert isinstance(ev.event, TextDelta)
        assert ev.event.delta == "hello"

    def test_from_sse_unknown(self):
        payload = json.dumps({"type": "future_thing", "x": 1})
        sse = SseEvent(id="ev-2", event="future_thing", data=payload)
        ev = AgentEvent.from_sse(sse)
        assert isinstance(ev.event, UnknownEvent)


class TestMobEvent:
    @pytest.mark.asyncio
    async def test_unfamiliar_hook_reasons_stay_opaque_and_tool_status_unchanged(self):
        # No receiving-refusal carrier is declared here; unfamiliar fields
        # remain open payloads, not a local permission or run-failure fact.
        reasons = [
            {"reason_code": "future_hook_cause", "version": 99, "details": {"kind": "denied"}},
            {"reason_code": 17, "refusal": "denied"},
            ["denied", None],
            None,
        ]
        hooks = [{
            "type": "hook_launch_refused", "hook_id": "host-hook", "point": "pre_tool_execution",
            "tool_use_id": "  opaque-call  ", "reason": reason,
            "future_detail": {"version": 99, "values": [None, 1]},
        } for reason in reasons]
        result = '{"version":99,"cause":{"kind":"denied","future":[null,true]}}'
        payloads = [
            *hooks,
            {"type": "tool_execution_completed", "id": "  opaque-call  ", "name": "read_file",
             "is_error": True, "duration_ms": 1},
            {"type": "tool_result_received", "id": "  opaque-call  ", "name": "read_file",
             "content": [{"type": "text", "text": result}], "result": result,
             "is_error": True, "future_detail": {"kind": "denied"}},
            {"type": "run_completed", "session_id": "session-opaque", "result": "done"},
        ]

        async def wire():
            for index, payload in enumerate(payloads):
                yield SseEvent(id=str(index), event=payload["type"], data=json.dumps({
                    "member_id": "agent-1", "payload": payload,
                }))

        events = [envelope.event async for envelope in EventStream(wire(), MobEvent)]
        for event, hook in zip(events, hooks):
            assert isinstance(event, UnknownEvent)
            assert event.type == "hook_launch_refused"
            assert event.data == hook
        tool, feedback, completed = events[len(hooks):]
        assert isinstance(tool, ToolExecutionCompleted)
        assert (tool.id, tool.is_error) == ("  opaque-call  ", True)
        assert isinstance(feedback, ToolResultReceived)
        assert (feedback.id, feedback.result, feedback.is_error) == ("  opaque-call  ", result, True)
        assert isinstance(completed, RunCompleted)
        assert (completed.session_id, completed.result) == ("session-opaque", "done")
        assert not any(isinstance(event, RunFailed) for event in events)

    @pytest.mark.asyncio
    async def test_local_refusal_wire_keeps_call_feedback_and_stream_continuity(self):
        refusal = '{"error":"operation_refused","message":"operation unavailable under current authorization"}'
        hook = {
            "type": "hook_launch_refused", "hook_id": "host-hook",
            "point": "pre_tool_execution", "tool_use_id": "  mechanical-call  ",
            "reason": {"reason_code": "confinement_refused", "refusal": "unsupported_platform"},
            "future_detail": {"owner": ["opaque", 1]},
        }
        # The host projector keeps the native content on the result event and
        # derives the published result field there. The execution fact for the
        # same call carries no body.
        payloads = [
            hook,
            {"type": "tool_execution_completed", "id": "  blocked-call  ",
             "tool_call_id": "  blocked-call  ", "name": "delete_file",
             "is_error": True, "duration_ms": 0},
            {"type": "tool_result_received", "id": "  blocked-call  ",
             "tool_call_id": "  blocked-call  ", "name": "delete_file",
             "content": [{"type": "text", "text": refusal}], "result": refusal,
             "is_error": True},
            {"type": "tool_execution_completed", "id": "permitted-call",
             "tool_call_id": "permitted-call", "name": "read_file",
             "is_error": False, "duration_ms": 1},
            {"type": "tool_result_received", "id": "permitted-call",
             "tool_call_id": "permitted-call", "name": "read_file",
             "content": [{"type": "text", "text": "permitted result"}],
             "result": "permitted result", "is_error": False},
            {"type": "run_completed", "session_id": "session-1", "result": "done"},
        ]

        async def wire():
            for index, payload in enumerate(payloads):
                yield SseEvent(id=str(index), event=payload["type"], data=json.dumps({
                    "member_id": "agent-1", "source": "agent-1", "payload": payload,
                }))

        events = [event async for event in EventStream(wire(), MobEvent)]
        assert [type(event.event) for event in events] == [
            UnknownEvent, ToolExecutionCompleted, ToolResultReceived,
            ToolExecutionCompleted, ToolResultReceived, RunCompleted,
        ]
        assert all(event.member_id == "agent-1" for event in events)
        unknown, blocked, blocked_feedback, permitted, permitted_feedback, completed = (
            event.event for event in events
        )
        assert isinstance(unknown, UnknownEvent)
        assert unknown.data == hook
        assert isinstance(blocked, ToolExecutionCompleted)
        assert (blocked.id, blocked.is_error, blocked.duration_ms) == ("  blocked-call  ", True, 0)
        assert isinstance(blocked_feedback, ToolResultReceived)
        assert (blocked_feedback.id, blocked_feedback.result, blocked_feedback.is_error) == (
            "  blocked-call  ", refusal, True,
        )
        assert isinstance(permitted, ToolExecutionCompleted)
        assert (permitted.id, permitted.is_error, permitted.duration_ms) == ("permitted-call", False, 1)
        assert isinstance(permitted_feedback, ToolResultReceived)
        assert (permitted_feedback.id, permitted_feedback.result, permitted_feedback.is_error) == (
            "permitted-call", "permitted result", False,
        )
        assert isinstance(completed, RunCompleted)
        assert (completed.session_id, completed.result) == ("session-1", "done")

    def test_from_sse_typed(self):
        payload = json.dumps({
            "member_id": "agent-1",
            "timestamp_ms": 1000,
            "payload": {"type": "text_delta", "delta": "hi"},
        })
        sse = SseEvent(id="ev-1", event="mob_event", data=payload)
        ev = MobEvent.from_sse(sse)
        assert ev.member_id == "agent-1"
        assert ev.timestamp_ms == 1000
        assert isinstance(ev.event, TextDelta)
        assert ev.event.delta == "hi"

    def test_from_sse_unknown_payload(self):
        payload = json.dumps({
            "member_id": "agent-2",
            "payload": {"type": "new_thing"},
        })
        sse = SseEvent(id="ev-2", event="mob_event", data=payload)
        ev = MobEvent.from_sse(sse)
        assert ev.member_id == "agent-2"
        assert isinstance(ev.event, UnknownEvent)
