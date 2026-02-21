"""Conformance tests for Meerkat Python SDK types and events."""

from meerkat import (
    CONTRACT_VERSION,
    Capability,
    MeerkatClient,
    RunResult,
    SchemaWarning,
    SessionInfo,
    SkillKey,
    SkillQuarantineDiagnostic,
    SkillRuntimeDiagnostics,
    SourceHealthSnapshot,
    Usage,
)
from meerkat.errors import (
    CapabilityUnavailableError,
    MeerkatError,
    SessionNotFoundError,
    SkillNotFoundError,
)
from meerkat.events import (
    BudgetWarning,
    CompactionStarted,
    Event,
    HookDenied,
    Retrying,
    RunCompleted,
    RunStarted,
    TextDelta,
    ToolCallRequested,
    ToolExecutionCompleted,
    TurnCompleted,
    TurnStarted,
    UnknownEvent,
    parse_event,
)


def test_contract_version():
    """Contract version should be semver."""
    parts = CONTRACT_VERSION.split(".")
    assert len(parts) == 3
    for part in parts:
        int(part)


# ---------------------------------------------------------------------------
# Public types
# ---------------------------------------------------------------------------

def test_usage_defaults():
    usage = Usage()
    assert usage.input_tokens == 0
    assert usage.output_tokens == 0
    assert usage.cache_creation_tokens is None
    assert usage.cache_read_tokens is None


def test_usage_frozen():
    usage = Usage(input_tokens=10, output_tokens=5)
    try:
        usage.input_tokens = 99  # type: ignore[misc]
        assert False, "Should be frozen"
    except AttributeError:
        pass


def test_run_result_defaults():
    result = RunResult()
    assert result.session_id == ""
    assert result.text == ""
    assert result.turns == 0
    assert result.tool_calls == 0
    assert result.usage == Usage()
    assert result.skill_diagnostics is None


def test_run_result_skill_diagnostics():
    diagnostics = SkillRuntimeDiagnostics(
        source_health=SourceHealthSnapshot(
            state="degraded",
            invalid_ratio=0.25,
            invalid_count=1,
            total_count=4,
            failure_streak=2,
            handshake_failed=False,
        ),
        quarantined=[
            SkillQuarantineDiagnostic(
                source_uuid="src-1",
                skill_id="extract/email",
                location="project",
                error_code="bad_frontmatter",
                error_class="ValidationError",
                message="missing description",
                first_seen_unix_secs=10,
                last_seen_unix_secs=20,
            )
        ],
    )
    result = RunResult(
        session_id="s1",
        text="ok",
        usage=Usage(input_tokens=10, output_tokens=5),
        skill_diagnostics=diagnostics,
    )
    assert result.skill_diagnostics == diagnostics


def test_parse_run_result_skill_diagnostics():
    raw = {
        "session_id": "s1",
        "text": "ok",
        "turns": 1,
        "tool_calls": 0,
        "usage": {"input_tokens": 10, "output_tokens": 5},
        "skill_diagnostics": {
            "source_health": {
                "state": "healthy",
                "invalid_ratio": 0.0,
                "invalid_count": 0,
                "total_count": 10,
                "failure_streak": 0,
                "handshake_failed": False,
            },
            "quarantined": [
                {
                    "source_uuid": "src-1",
                    "skill_id": "extract/email",
                    "location": "project",
                    "error_code": "bad_frontmatter",
                    "error_class": "ValidationError",
                    "message": "missing description",
                    "first_seen_unix_secs": 10,
                    "last_seen_unix_secs": 20,
                }
            ],
        },
    }
    result = MeerkatClient._parse_run_result(raw)
    assert result.skill_diagnostics is not None
    assert result.skill_diagnostics.source_health.state == "healthy"
    assert result.skill_diagnostics.quarantined[0].skill_id == "extract/email"


def test_skill_key_export():
    key = SkillKey(source_uuid="abc-123", skill_name="my-skill")
    assert key.source_uuid == "abc-123"
    assert key.skill_name == "my-skill"


def test_session_info():
    info = SessionInfo(session_id="abc", is_active=True, message_count=5)
    assert info.session_id == "abc"
    assert info.is_active is True
    assert info.message_count == 5


def test_capability_available():
    cap = Capability(id="sessions", description="Session lifecycle", status="Available")
    assert cap.available is True
    disabled = Capability(id="comms", status="DisabledByPolicy")
    assert disabled.available is False


def test_normalize_status_accepts_externally_tagged_enum():
    status = MeerkatClient._normalize_status(
        {"DisabledByPolicy": {"description": "comms disabled"}}
    )
    assert status == "DisabledByPolicy"


# ---------------------------------------------------------------------------
# Error hierarchy
# ---------------------------------------------------------------------------

def test_error_hierarchy():
    assert issubclass(CapabilityUnavailableError, MeerkatError)
    assert issubclass(SessionNotFoundError, MeerkatError)
    assert issubclass(SkillNotFoundError, MeerkatError)


def test_error_fields():
    err = MeerkatError("TEST_CODE", "test message", details={"key": "value"})
    assert err.code == "TEST_CODE"
    assert err.message == "test message"
    assert err.details == {"key": "value"}
    assert str(err) == "test message"


# ---------------------------------------------------------------------------
# Typed events
# ---------------------------------------------------------------------------

def test_text_delta_is_event():
    td = TextDelta(delta="hello")
    assert isinstance(td, Event)
    assert td.delta == "hello"


def test_events_are_frozen():
    td = TextDelta(delta="hi")
    try:
        td.delta = "bye"  # type: ignore[misc]
        assert False, "Should be frozen"
    except AttributeError:
        pass


def test_turn_completed_has_usage():
    u = Usage(input_tokens=100, output_tokens=50)
    tc = TurnCompleted(stop_reason="end_turn", usage=u)
    assert tc.stop_reason == "end_turn"
    assert tc.usage.input_tokens == 100


def test_tool_call_requested():
    tc = ToolCallRequested(id="t1", name="search", args={"query": "rust"})
    assert tc.name == "search"
    assert tc.args == {"query": "rust"}


def test_budget_warning():
    bw = BudgetWarning(budget_type="tokens", used=900, limit=1000, percent=90.0)
    assert bw.percent == 90.0


def test_hook_denied_optional_payload():
    hd = HookDenied(hook_id="h1", point="pre_tool_execution",
                     reason_code="policy_violation", message="blocked")
    assert hd.payload is None
    hd2 = HookDenied(hook_id="h1", point="pre_tool_execution",
                      reason_code="safety", message="bad", payload={"detail": "x"})
    assert hd2.payload == {"detail": "x"}


# ---------------------------------------------------------------------------
# Event parser
# ---------------------------------------------------------------------------

def test_parse_text_delta():
    raw = {"type": "text_delta", "delta": "Hello"}
    event = parse_event(raw)
    assert isinstance(event, TextDelta)
    assert event.delta == "Hello"


def test_parse_turn_started():
    raw = {"type": "turn_started", "turn_number": 3}
    event = parse_event(raw)
    assert isinstance(event, TurnStarted)
    assert event.turn_number == 3


def test_parse_turn_completed_with_usage():
    raw = {
        "type": "turn_completed",
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 50, "output_tokens": 20},
    }
    event = parse_event(raw)
    assert isinstance(event, TurnCompleted)
    assert event.stop_reason == "end_turn"
    assert event.usage.input_tokens == 50
    assert event.usage.output_tokens == 20
    assert event.usage.cache_creation_tokens is None


def test_parse_run_completed():
    raw = {
        "type": "run_completed",
        "session_id": "abc-123",
        "result": "Done!",
        "usage": {"input_tokens": 100, "output_tokens": 50},
    }
    event = parse_event(raw)
    assert isinstance(event, RunCompleted)
    assert event.session_id == "abc-123"
    assert event.result == "Done!"
    assert event.usage.input_tokens == 100


def test_parse_tool_execution_completed():
    raw = {
        "type": "tool_execution_completed",
        "id": "t1",
        "name": "search",
        "result": "found it",
        "is_error": False,
        "duration_ms": 42,
    }
    event = parse_event(raw)
    assert isinstance(event, ToolExecutionCompleted)
    assert event.name == "search"
    assert event.duration_ms == 42


def test_parse_unknown_event_type():
    raw = {"type": "future_event_v2", "data": "something"}
    event = parse_event(raw)
    assert isinstance(event, UnknownEvent)
    assert event.type == "future_event_v2"
    assert event.data == raw


def test_parse_missing_type():
    raw = {"delta": "oops"}
    event = parse_event(raw)
    assert isinstance(event, UnknownEvent)
    assert event.type == ""


def test_parse_compaction_started():
    raw = {"type": "compaction_started", "input_tokens": 5000,
           "estimated_history_tokens": 4000, "message_count": 12}
    event = parse_event(raw)
    assert isinstance(event, CompactionStarted)
    assert event.input_tokens == 5000
    assert event.message_count == 12


def test_parse_retrying():
    raw = {"type": "retrying", "attempt": 2, "max_attempts": 3,
           "error": "rate limit", "delay_ms": 2000}
    event = parse_event(raw)
    assert isinstance(event, Retrying)
    assert event.attempt == 2
    assert event.delay_ms == 2000


# ---------------------------------------------------------------------------
# Pattern matching
# ---------------------------------------------------------------------------

def test_match_case_text_delta():
    event = parse_event({"type": "text_delta", "delta": "hi"})
    match event:
        case TextDelta(delta=text):
            assert text == "hi"
        case _:
            assert False, "Should match TextDelta"


def test_match_case_run_started():
    event = parse_event({"type": "run_started", "session_id": "s1", "prompt": "yo"})
    match event:
        case RunStarted(session_id=sid, prompt=p):
            assert sid == "s1"
            assert p == "yo"
        case _:
            assert False, "Should match RunStarted"
