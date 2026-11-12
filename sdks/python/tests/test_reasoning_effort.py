"""The provider-neutral ``reasoning_effort`` option.

``session/create`` takes a level; ``turn/start`` and ``mob/turn_start`` take
its tri-state (a level, ``{"action": "clear"}``, absent). An older server
ignores the parameter on ``session/create`` and refuses it on a turn, so the
SDK refuses to send it below contract 0.9.15. A level the model or route does
not accept is refused with ``error.data.kind`` ``"reasoning_effort_refused"``
(``SessionError`` in crates/meerkat-core/src/service/mod.rs), which
``reasoning_effort_refusal`` reads back.
"""

from __future__ import annotations

from typing import Any

import pytest

from meerkat import (
    REASONING_EFFORT_CLEAR,
    REASONING_EFFORT_LEVELS,
    REASONING_EFFORT_REFUSAL_REASONS,
    MeerkatClient,
    ReasoningRoute,
    RequestReasoningEffortResolved,
    RunFailed,
    RunResult,
    RunStarted,
    Session,
    UnknownEvent,
    Usage,
    parse_event,
    parse_reasoning_effort_refusal,
    parse_reasoning_effort_report,
    parse_reasoning_effort_resolution,
    reasoning_effort_refusal,
)
from meerkat.errors import CapabilityUnavailableError, MeerkatError

SESSION_ID = "019a2b3c-0000-7000-8000-000000000001"

RUN_RESULT = {
    "session_id": SESSION_ID,
    "text": "ok",
    "turns": 1,
    "tool_calls": 0,
    "usage": {"input_tokens": 1, "output_tokens": 1},
}


def fake_client(
    contract_version: str = "0.9.15",
) -> tuple[MeerkatClient, list[tuple[str, dict[str, Any]]]]:
    client = MeerkatClient()
    client._server_contract_version = contract_version
    calls: list[tuple[str, dict[str, Any]]] = []

    async def fake_request(method: str, params: dict[str, Any]) -> dict[str, Any]:
        calls.append((method, params))
        return dict(RUN_RESULT)

    client._request = fake_request  # type: ignore[method-assign]
    return client, calls


def session(client: MeerkatClient) -> Session:
    return Session(client, RunResult(session_id=SESSION_ID, text="ready", usage=Usage()))


def test_the_level_vocabulary_is_the_seven_levels_in_one_spelling() -> None:
    assert REASONING_EFFORT_LEVELS == (
        "none",
        "minimal",
        "low",
        "medium",
        "high",
        "xhigh",
        "max",
    )
    assert "x_high" not in REASONING_EFFORT_LEVELS
    assert REASONING_EFFORT_CLEAR == {"action": "clear"}


@pytest.mark.asyncio
async def test_create_session_sends_the_level() -> None:
    client, calls = fake_client()

    await client.create_session("hello", reasoning_effort="high")
    await client.create_deferred_session("hello", reasoning_effort="xhigh")
    await client.create_session("hello")

    assert [method for method, _ in calls] == ["session/create"] * 3
    assert calls[0][1]["reasoning_effort"] == "high"
    assert calls[1][1]["reasoning_effort"] == "xhigh"
    assert "reasoning_effort" not in calls[2][1]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("reasoning_effort", "wire"),
    [
        pytest.param("low", "low", id="level"),
        pytest.param(
            {"action": "set", "value": "max"}, {"action": "set", "value": "max"}, id="set"
        ),
        pytest.param(REASONING_EFFORT_CLEAR, {"action": "clear"}, id="clear"),
    ],
)
async def test_turns_send_the_tri_state(reasoning_effort: Any, wire: Any) -> None:
    client, calls = fake_client()

    await session(client).turn("next", reasoning_effort=reasoning_effort)
    await client.mob_turn_start("mob-1", "worker", "next", reasoning_effort=reasoning_effort)

    assert [method for method, _ in calls] == ["turn/start", "mob/turn_start"]
    assert calls[0][1]["reasoning_effort"] == wire
    assert calls[1][1]["reasoning_effort"] == wire


@pytest.mark.asyncio
async def test_a_turn_that_names_no_level_sends_none() -> None:
    client, calls = fake_client()

    await session(client).turn("next", provider_params={"temperature": 0.2})
    await client.mob_turn_start("mob-1", "worker", "next")

    assert "reasoning_effort" not in calls[0][1]
    assert "reasoning_effort" not in calls[1][1]


OLDER_SERVER_REFUSAL = (
    r"reasoning_effort needs a server contract >= 0\.9\.15; this server reports 0\.9\.14"
)


@pytest.mark.asyncio
async def test_sdk_refuses_the_field_for_an_older_server_contract() -> None:
    """Released 0.9.14 has no such parameter: its session/create ignores it
    and the session would run with no level; its turn/start refuses it."""
    client, calls = fake_client("0.9.14")
    live = session(client)

    with pytest.raises(CapabilityUnavailableError, match=OLDER_SERVER_REFUSAL) as refused:
        await client.create_session("hello", reasoning_effort="high")
    assert refused.value.code == "CAPABILITY_UNAVAILABLE"
    with pytest.raises(CapabilityUnavailableError, match=OLDER_SERVER_REFUSAL):
        await client.create_deferred_session("hello", reasoning_effort="high")
    with pytest.raises(CapabilityUnavailableError, match=OLDER_SERVER_REFUSAL):
        await live.turn("next", reasoning_effort="high")
    with pytest.raises(CapabilityUnavailableError, match=OLDER_SERVER_REFUSAL):
        await live.turn("next", reasoning_effort=REASONING_EFFORT_CLEAR)
    with pytest.raises(CapabilityUnavailableError, match=OLDER_SERVER_REFUSAL):
        await client.mob_turn_start("mob-1", "worker", "next", reasoning_effort="high")
    assert calls == [], "nothing is written to a server that would drop the level"

    # A server that reports no contract at all is refused too.
    unknown, unknown_calls = fake_client("")
    with pytest.raises(CapabilityUnavailableError, match="this server reports none"):
        await unknown.create_session("hello", reasoning_effort="high")
    assert unknown_calls == []

    # Calls that name no level are unaffected.
    await client.create_session("hello")
    await live.turn("next")
    assert [method for method, _ in calls] == ["session/create", "turn/start"]


# The refusal's data for a level the model does not accept.
UNSUPPORTED = {
    "kind": "reasoning_effort_refused",
    "retryable": False,
    "reasoning_effort_refusal": {
        "requested": "max",
        "provider": "anthropic",
        "model": "claude-opus-4-5",
        "route": {"backend": "anthropic_api", "api": "messages"},
        "reason": "unsupported_level",
        "supported": ["low", "medium", "high"],
        "facts": "catalog",
    },
}


def refused(data: Any) -> MeerkatError:
    return MeerkatError("INVALID_PARAMS", "reasoning effort refused", details=data)


def with_refusal(**changes: Any) -> dict[str, Any]:
    refusal = {**UNSUPPORTED["reasoning_effort_refusal"], **changes}
    return {**UNSUPPORTED, "reasoning_effort_refusal": refusal}


def test_the_typed_refusal_is_read_from_the_error_data() -> None:
    refusal = reasoning_effort_refusal(refused(UNSUPPORTED))

    assert refusal is not None
    assert refusal.requested == "max"
    assert refusal.provider == "anthropic"
    assert refusal.model == "claude-opus-4-5"
    assert refusal.route == ReasoningRoute(backend="anthropic_api", api="messages")
    assert refusal.reason == "unsupported_level"
    assert refusal.supported == ("low", "medium", "high")
    assert refusal.facts == "catalog"
    assert refusal.conflicting_param is None
    assert refusal.conflicting_level is None
    assert refusal.transient is False
    assert refusal.retryable is False


def test_the_eight_reasons_are_closed_and_only_one_is_transient() -> None:
    assert REASONING_EFFORT_REFUSAL_REASONS == (
        "unsupported_level",
        "supported_levels_unknown",
        "supported_levels_unavailable",
        "conflicting_provider_effort",
        "conflicting_thinking_budget",
        "conflicting_thinking_type",
        "conflicting_reasoning_body",
        "conflicting_reasoning_mode",
    )
    for reason in REASONING_EFFORT_REFUSAL_REASONS:
        transient = reason == "supported_levels_unavailable"
        data = {**with_refusal(reason=reason), "retryable": transient}
        refusal = reasoning_effort_refusal(refused(data))
        assert refusal is not None, reason
        assert refusal.reason == reason
        # The class is read from the reason; the server's mark agrees.
        assert refusal.transient is transient, reason
        assert refusal.retryable is transient, reason

    # The class does not depend on the advisory mark being present.
    unmarked = {k: v for k, v in with_refusal(reason="supported_levels_unavailable").items() if k != "retryable"}
    refusal = reasoning_effort_refusal(refused(unmarked))
    assert refusal is not None
    assert refusal.transient is True
    assert refusal.retryable is None


def test_unknown_and_unavailable_levels_have_no_supported_set() -> None:
    for reason in ("supported_levels_unknown", "supported_levels_unavailable"):
        refusal = reasoning_effort_refusal(
            refused(with_refusal(reason=reason, supported=None, facts=None))
        )
        assert refusal is not None
        assert refusal.supported is None
        assert refusal.facts is None
    # A route or model that takes no level states the empty set.
    refusal = reasoning_effort_refusal(refused(with_refusal(supported=[])))
    assert refusal is not None
    assert refusal.supported == ()


def test_a_refusal_that_names_no_route_parses() -> None:
    refusal = reasoning_effort_refusal(
        refused(with_refusal(route=None, reason="supported_levels_unknown", supported=None, facts=None))
    )
    assert refusal is not None
    assert refusal.route is None
    # A document that leaves the key out reads the same way.
    without_route = with_refusal(reason="supported_levels_unknown", supported=None, facts=None)
    del without_route["reasoning_effort_refusal"]["route"]
    refusal = reasoning_effort_refusal(refused(without_route))
    assert refusal is not None
    assert refusal.route is None


def test_a_conflict_names_the_setting_and_its_level() -> None:
    refusal = reasoning_effort_refusal(
        refused(
            with_refusal(
                reason="conflicting_provider_effort",
                conflicting_param="provider_params.provider_tag.reasoning_effort",
                conflicting_level="low",
            )
        )
    )
    assert refusal is not None
    assert refusal.conflicting_param == "provider_params.provider_tag.reasoning_effort"
    assert refusal.conflicting_level == "low"


def test_advisory_route_and_facts_values_are_kept_as_sent() -> None:
    refusal = reasoning_effort_refusal(
        refused(
            with_refusal(route={"backend": "new_backend", "api": "new_api"}, facts="new_source")
        )
    )
    assert refusal is not None
    assert refusal.route == ReasoningRoute(backend="new_backend", api="new_api")
    assert refusal.facts == "new_source"


def test_a_refusal_this_sdk_cannot_read_keeps_the_raw_data() -> None:
    unknown_reason = with_refusal(reason="a_reason_from_a_newer_server")
    error = refused(unknown_reason)
    assert reasoning_effort_refusal(error) is None
    assert error.details == unknown_reason

    for unreadable in (
        with_refusal(requested="x_high"),
        with_refusal(supported=["low", "x_high"]),
        with_refusal(route="anthropic_api/messages"),
        with_refusal(model=None),
        {**UNSUPPORTED, "reasoning_effort_refusal": None},
    ):
        assert reasoning_effort_refusal(refused(unreadable)) is None

    # Another error's data is not a refusal.
    assert reasoning_effort_refusal(refused({"kind": "mob_quarantined"})) is None
    assert reasoning_effort_refusal(refused(None)) is None
    assert reasoning_effort_refusal(ValueError("no details")) is None
    assert parse_reasoning_effort_refusal("not an object") is None


# The `reasoning_effort` block of a run result after a model fallback switch.
REPORT = {
    "requested": "high",
    "sent": [
        {
            "level": "high",
            "provider": "openai",
            "model": "gpt-5.5",
            "route": {"backend": "openai_api", "api": "responses"},
            "fields": ["reasoning.effort"],
            "facts": "catalog",
            "decided_by": "request",
            "requests": 3,
        },
        {
            "level": "high",
            "provider": "anthropic",
            "model": "claude-opus-4-8",
            "route": {"backend": "new_backend", "api": "new_api"},
            "fields": ["thinking.type", "output_config.effort"],
            "facts": "new_source",
            "decided_by": "new_decider",
            "requests": 1,
        },
    ],
}


def test_the_run_report_lists_what_was_sent() -> None:
    report = parse_reasoning_effort_report(REPORT)

    assert report is not None
    assert report.requested == "high"
    assert [row.model for row in report.sent] == ["gpt-5.5", "claude-opus-4-8"]
    first, second = report.sent
    assert first.level == "high"
    assert first.provider == "openai"
    assert first.route == ReasoningRoute(backend="openai_api", api="responses")
    assert first.fields == ("reasoning.effort",)
    assert first.facts == "catalog"
    assert first.decided_by == "request"
    assert first.requests == 3
    # Advisory values this SDK does not know are kept as sent.
    assert second.route == ReasoningRoute(backend="new_backend", api="new_api")
    assert second.facts == "new_source"
    assert second.decided_by == "new_decider"

    assert parse_reasoning_effort_report(None) is None
    assert parse_reasoning_effort_report({"sent": []}) is None


def test_a_resolution_is_read_with_its_requested_level() -> None:
    resolution = parse_reasoning_effort_resolution(
        {
            "requested": "high",
            "level": "high",
            "provider": "openai",
            "model": "gpt-5.5",
            "route": {"backend": "copilot", "api": "chat_completions"},
            "fields": ["reasoning_effort", "reasoning.effort"],
            "facts": "account",
            "decided_by": "request",
        }
    )
    assert resolution is not None
    assert resolution.requested == "high"
    assert resolution.level == "high"
    assert resolution.route == ReasoningRoute(backend="copilot", api="chat_completions")
    assert resolution.fields == ("reasoning_effort", "reasoning.effort")
    assert resolution.facts == "account"
    assert resolution.decided_by == "request"
    assert parse_reasoning_effort_resolution(None) is None


RESOLUTION = {
    "requested": "high",
    "level": "high",
    "provider": "openai",
    "model": "gpt-5.5",
    "route": {"backend": "openai_api", "api": "responses"},
    "fields": ["reasoning.effort"],
    "facts": "catalog",
    "decided_by": "request",
}


def test_the_run_result_carries_the_report() -> None:
    result = MeerkatClient._parse_run_result({**RUN_RESULT, "reasoning_effort": REPORT})
    assert result.reasoning_effort == parse_reasoning_effort_report(REPORT)
    assert result.reasoning_effort is not None
    assert result.reasoning_effort.sent[0].requests == 3
    # A run without a level reports nothing.
    assert MeerkatClient._parse_run_result(dict(RUN_RESULT)).reasoning_effort is None


def test_run_started_and_the_per_request_event_are_typed() -> None:
    started = parse_event(
        {
            "type": "run_started",
            "session_id": "s1",
            "input": {"kind": "content", "content": "yo"},
            "reasoning_effort": RESOLUTION,
        }
    )
    assert isinstance(started, RunStarted)
    assert started.reasoning_effort == parse_reasoning_effort_resolution(RESOLUTION)
    plain = parse_event(
        {"type": "run_started", "session_id": "s1", "input": {"kind": "content", "content": "yo"}}
    )
    assert isinstance(plain, RunStarted)
    assert plain.reasoning_effort is None

    resolved = parse_event(
        {
            "type": "request_reasoning_effort_resolved",
            "turn_number": 2,
            "fallback_attempt": 1,
            "resolution": {**RESOLUTION, "facts": "new_source", "decided_by": "new_decider"},
        }
    )
    assert isinstance(resolved, RequestReasoningEffortResolved)
    assert resolved.turn_number == 2
    assert resolved.fallback_attempt == 1
    assert resolved.resolution is not None
    assert resolved.resolution.level == "high"
    # Advisory values this SDK does not know are kept as sent.
    assert resolved.resolution.facts == "new_source"
    assert resolved.resolution.decided_by == "new_decider"
    # Both positions are optional.
    bare = parse_event({"type": "request_reasoning_effort_resolved", "resolution": RESOLUTION})
    assert isinstance(bare, RequestReasoningEffortResolved)
    assert bare.turn_number is None
    assert bare.fallback_attempt is None


def run_failed(reason: dict[str, Any]) -> RunFailed:
    event = parse_event(
        {
            "type": "run_failed",
            "session_id": "s1",
            "error_report": {"class": "config", "message": "refused", "reason": reason},
        }
    )
    assert isinstance(event, RunFailed)
    return event


def test_a_refused_run_carries_the_typed_refusal() -> None:
    refusal = UNSUPPORTED["reasoning_effort_refusal"]
    reason = run_failed({"reason_type": "reasoning_effort_refused", "refusal": refusal})
    assert reason.error_report.reason.reason_type == "reasoning_effort_refused"
    assert reason.error_report.reason.reasoning_effort_refusal == parse_reasoning_effort_refusal(
        refusal
    )
    assert reason.error_report.reason.reasoning_effort_refusal.transient is False

    # A refusal this SDK cannot read is never read as another reason.
    newer = {**refusal, "reason": "a_reason_from_a_newer_server"}
    unread = run_failed({"reason_type": "reasoning_effort_refused", "refusal": newer})
    assert unread.error_report.reason.reason_type == "unknown"
    assert unread.error_report.reason.raw_reason_type == "reasoning_effort_refused"
    assert unread.error_report.reason.reasoning_effort_refusal is None
    assert unread.error_report.reason.raw["refusal"] == newer


@pytest.mark.parametrize("reason", ["reasoning_effort_unsupported", "a_newer_skip_reason"])
def test_a_fallback_skip_reason_this_sdk_does_not_type_is_kept(reason: str) -> None:
    raw = {
        "type": "model_fallback_skipped",
        "target": {"model": "gpt-5.5", "provider": "openai", "reason": reason},
    }
    event = parse_event(raw)
    assert isinstance(event, UnknownEvent)
    assert event.type == "model_fallback_skipped"
    assert event.data == raw
