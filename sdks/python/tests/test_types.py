import asyncio
"""Conformance tests for Meerkat Python SDK types and events."""

import base64
import json
from pathlib import Path

import pytest


def _make_member_ref(mob_id: str, agent_identity: str) -> str:
    """Build an opaque wire `member_ref` token using the same shape the
    server emits (``base64url({"m": mob_id, "a": agent_identity})``).

    The Python SDK only forwards `member_ref` opaquely — it never decodes
    it — so any non-empty string would satisfy the parser. We still mirror
    the server-side encoding here so test fixtures stay realistic and a
    future round-trip decode assertion would succeed without churn.
    """
    payload = json.dumps({"m": mob_id, "a": agent_identity}, separators=(",", ":"))
    return base64.urlsafe_b64encode(payload.encode("utf-8")).rstrip(b"=").decode("ascii")


try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - exercised on Python 3.10
    import tomli as tomllib

from meerkat import (
    CONTRACT_VERSION,
    Capability,
    McpAddParams,
    McpLiveOpResponse,
    MeerkatClient,
    RealtimeChannel,
    RunResult,
    SchemaWarning,
    SessionAssistantBlock,
    SessionHistory,
    SessionInfo,
    SessionDetails,
    SessionMessage,
    SessionSummary,
    SessionToolCall,
    SessionToolResult,
    SkillKey,
    SkillQuarantineDiagnostic,
    SkillRuntimeDiagnostics,
    SourceHealthSnapshot,
    Session,
    DeferredSession,
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
    SkillResolutionFailed,
    SkillResolutionFailureReason,
    ToolConfigChanged,
    TextDelta,
    ToolCallRequested,
    ToolExecutionCompleted,
    TurnCompleted,
    TurnStarted,
    UnknownEvent,
    parse_event,
)
from meerkat.generated.types import (
    RealtimeCapabilities as GeneratedRealtimeCapabilities,
    RealtimeChannelOpenFrame as GeneratedRealtimeChannelOpenFrame,
    RealtimeOpenInfo as GeneratedRealtimeOpenInfo,
    RuntimeStateResult as GeneratedRuntimeStateResult,
)


def test_contract_version():
    """Contract version should be semver."""
    parts = CONTRACT_VERSION.split(".")
    assert len(parts) == 3
    for part in parts:
        int(part)


def test_contract_version_matches_package_version():
    pyproject = Path(__file__).resolve().parents[1] / "pyproject.toml"
    data = tomllib.loads(pyproject.read_text())
    assert CONTRACT_VERSION == data["project"]["version"]


def test_generated_realtime_types_include_open_info_shape():
    info = GeneratedRealtimeOpenInfo(
        ws_url="ws://localhost:9999/realtime/ws",
        open_token="token-1",
        expires_at="2026-04-15T12:00:00Z",
        target={"type": "session_target", "session_id": "session-1"},
        supported_protocol_versions=["1"],
        default_protocol_version="1",
        capabilities=GeneratedRealtimeCapabilities(
            input_kinds=["text", "audio"],
            output_kinds=["text", "audio"],
            turning_modes=["provider_managed", "explicit_commit"],
            interrupt_supported=True,
            transcript_supported=True,
            tool_lifecycle_events_supported=True,
            video_supported=False,
        ),
    )

    assert info.ws_url.endswith("/realtime/ws")
    assert info.default_protocol_version == "1"
    assert info.supported_protocol_versions == ["1"]
    assert info.capabilities.turning_modes == [
        "provider_managed",
        "explicit_commit",
    ]

    frame = GeneratedRealtimeChannelOpenFrame(
        protocol_version="1",
        open_token="token-1",
        role="primary",
        turning_mode="provider_managed",
    )
    assert frame.protocol_version == "1"


def test_generated_runtime_state_result_carries_state():
    result = GeneratedRuntimeStateResult(state="idle")

    assert result.state == "idle"


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


def test_parse_session_history():
    raw = {
        "session_id": "s1",
        "session_ref": "ref-1",
        "message_count": 3,
        "offset": 1,
        "limit": 2,
        "has_more": True,
        "messages": [
            {"role": "system", "content": "rules"},
            {
                "role": "assistant",
                "content": "working",
                "tool_calls": [{"id": "tc_1", "name": "search", "args": {"q": "rust"}}],
                "stop_reason": "tool_use",
            },
            {
                "role": "tool_results",
                "results": [{"tool_use_id": "tc_1", "content": "done", "is_error": False}],
            },
        ],
    }
    history = MeerkatClient._parse_session_history(raw)
    assert history.session_id == "s1"
    assert history.limit == 2
    assert history.has_more is True
    assert history.messages[1].tool_calls[0].name == "search"
    assert history.messages[2].results[0].tool_use_id == "tc_1"


def test_parse_session_history_preserves_assistant_image_blocks():
    history = MeerkatClient._parse_session_history(
        {
            "session_id": "s1",
            "message_count": 1,
            "offset": 0,
            "has_more": False,
            "messages": [
                {
                    "role": "block_assistant",
                    "blocks": [
                        {
                            "block_type": "image",
                            "data": {
                                "image_id": "img_1",
                                "blob_ref": {
                                    "blob_id": "blob_1",
                                    "media_type": "image/png",
                                },
                                "media_type": "image/png",
                                "width": 1024,
                                "height": 1536,
                                "revised_prompt": {"disposition": "not_requested"},
                                "meta": {"provider": "open_ai", "target_model": "gpt-image-1"},
                            },
                        }
                    ],
                }
            ],
        }
    )

    block = history.messages[0].blocks[0]
    assert block.block_type == "image"
    assert block.image_id == "img_1"
    assert block.blob_id == "blob_1"
    assert block.media_type == "image/png"
    assert block.width == 1024
    assert block.height == 1536
    assert block.revised_prompt == {"disposition": "not_requested"}
    assert block.meta == {"provider": "open_ai", "target_model": "gpt-image-1"}


def test_skill_key_export():
    key = SkillKey(source_uuid="abc-123", skill_name="my-skill")
    assert key.source_uuid == "abc-123"
    assert key.skill_name == "my-skill"


def test_session_info():
    info = SessionInfo(session_id="abc", is_active=True, message_count=5)
    assert info.session_id == "abc"
    assert info.is_active is True
    assert info.message_count == 5


def test_session_summary_and_details_types():
    summary = SessionSummary(
        session_id="summary-id",
        created_at=1700000000,
        updated_at=1700000100,
        message_count=4,
        total_tokens=128,
        labels={"env": "dev"},
        is_active=False,
    )
    details = SessionDetails(
        session_id="details-id",
        created_at=1700000001,
        updated_at=1700000111,
        message_count=6,
        labels={"team": "sdk"},
        is_active=True,
        model="claude-sonnet-4-6",
        provider="anthropic",
        last_assistant_text="ready",
    )
    assert summary.total_tokens == 128
    assert summary.created_at == 1700000000
    assert details.model == "claude-sonnet-4-6"
    assert details.provider == "anthropic"
    assert details.last_assistant_text == "ready"


def test_session_history_types():
    history = SessionHistory(
        session_id="abc",
        session_ref="ref-1",
        message_count=3,
        offset=1,
        limit=2,
        has_more=True,
        messages=[
            SessionMessage(role="system", content="rules"),
            SessionMessage(
                role="assistant",
                blocks=[
                    SessionAssistantBlock(
                        block_type="tool_use",
                        id="tool-1",
                        name="search",
                        args={"q": "meerkat"},
                    )
                ],
                tool_calls=[SessionToolCall(id="tool-1", name="search", args={"q": "meerkat"})],
                results=[SessionToolResult(tool_use_id="tool-1", content="done")],
            ),
        ],
    )
    assert history.session_ref == "ref-1"
    assert history.has_more is True
    assert history.messages[1].blocks[0].name == "search"


@pytest.mark.asyncio
async def test_session_history_convenience_method_uses_client() -> None:
    expected = SessionHistory(
        session_id="abc",
        session_ref=None,
        message_count=0,
        offset=0,
        limit=None,
        has_more=False,
        messages=[],
    )

    class StubClient:
        async def read_session_history(self, session_id: str, *, offset: int = 0, limit: int | None = None) -> SessionHistory:
            assert session_id == "abc"
            assert offset == 2
            assert limit == 5
            return expected

    session = Session(
        StubClient(),  # type: ignore[arg-type]
        RunResult(session_id="abc", text="ok", usage=Usage()),
    )
    history = await session.history(offset=2, limit=5)
    assert history is expected


@pytest.mark.asyncio
async def test_create_session_returns_runtime_backed_session_wrapper() -> None:
    client = MeerkatClient()
    seen: list[tuple[str, dict[str, object]]] = []

    async def fake_request(method: str, params: dict[str, object]) -> dict[str, object]:
        seen.append((method, params))
        return {
            "session_id": "sess-1",
            "session_ref": "team/runtime",
            "text": "ready",
            "turns": 1,
            "tool_calls": 0,
            "usage": {"input_tokens": 12, "output_tokens": 4},
        }

    client._request = fake_request  # type: ignore[method-assign]

    session = await client.create_session("Summarise the runtime path")

    assert isinstance(session, Session)
    assert session.id == "sess-1"
    assert session.ref == "team/runtime"
    assert session.text == "ready"
    assert session.last_result.session_id == "sess-1"
    assert seen == [("session/create", {"prompt": "Summarise the runtime path"})]


@pytest.mark.asyncio
async def test_deferred_session_history_convenience_method_uses_client() -> None:
    expected = SessionHistory(
        session_id="def",
        session_ref=None,
        message_count=0,
        offset=0,
        limit=None,
        has_more=False,
        messages=[],
    )

    class StubClient:
        async def read_session_history(self, session_id: str, *, offset: int = 0, limit: int | None = None) -> SessionHistory:
            assert session_id == "def"
            assert offset == 1
            assert limit is None
            return expected

    session = DeferredSession(StubClient(), "def")  # type: ignore[arg-type]
    history = await session.history(offset=1)
    assert history is expected


@pytest.mark.asyncio
async def test_create_deferred_session_returns_runtime_backed_deferred_wrapper() -> None:
    client = MeerkatClient()
    seen: list[tuple[str, dict[str, object]]] = []

    async def fake_request(method: str, params: dict[str, object]) -> dict[str, object]:
        seen.append((method, params))
        return {
            "session_id": "sess-2",
            "session_ref": "team/deferred",
        }

    client._request = fake_request  # type: ignore[method-assign]

    deferred = await client.create_deferred_session("Hold until first turn")

    assert isinstance(deferred, DeferredSession)
    assert deferred.id == "sess-2"
    assert deferred.ref == "team/deferred"
    assert seen == [
        (
            "session/create",
            {"prompt": "Hold until first turn", "initial_turn": "deferred"},
        )
    ]


def test_live_mcp_contract_types_exported():
    add = McpAddParams(
        session_id="s1",
        server_config={"command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem"]},
        server_name="filesystem",
        persisted=False,
    )
    response = McpLiveOpResponse(
        session_id="s1",
        operation="remove",
        status="staged",
        persisted=False,
        applied_at_turn=5,
    )
    assert add.server_name == "filesystem"
    assert response.applied_at_turn == 5


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


def test_parse_tool_config_changed():
    raw = {
        "type": "tool_config_changed",
        "payload": {
            "operation": "remove",
            "target": "filesystem",
            "status": "staged",
            "persisted": False,
            "applied_at_turn": 7,
        },
    }
    event = parse_event(raw)
    assert isinstance(event, ToolConfigChanged)
    assert event.payload.operation == "remove"
    assert event.payload.target == "filesystem"
    assert event.payload.status == "staged"
    assert event.payload.persisted is False
    assert event.payload.applied_at_turn == 7


def test_parse_tool_config_changed_with_malformed_payload():
    raw = {
        "type": "tool_config_changed",
        "payload": "not-an-object",
    }
    event = parse_event(raw)
    assert isinstance(event, ToolConfigChanged)
    assert event.payload.operation is None
    assert event.payload.target is None
    assert event.payload.status is None
    assert event.payload.persisted is None
    assert event.payload.applied_at_turn is None


def test_parse_tool_config_changed_with_bad_applied_at_turn():
    raw = {
        "type": "tool_config_changed",
        "payload": {
            "operation": "add",
            "target": "filesystem",
            "status": "staged",
            "persisted": True,
            "applied_at_turn": "oops",
        },
    }
    event = parse_event(raw)
    assert isinstance(event, ToolConfigChanged)
    assert event.payload.applied_at_turn is None


def test_parse_tool_config_changed_with_non_boolean_persisted():
    raw = {
        "type": "tool_config_changed",
        "payload": {
            "operation": "add",
            "target": "filesystem",
            "status": "staged",
            "persisted": "false",
        },
    }
    event = parse_event(raw)
    assert isinstance(event, ToolConfigChanged)
    assert event.payload.persisted is None


def test_parse_tool_config_changed_with_malformed_operation_status_semantics():
    raw = {
        "type": "tool_config_changed",
        "payload": {
            "operation": "bogus",
            "target": 0,
            "status": 0,
            "persisted": "false",
        },
    }
    event = parse_event(raw)
    assert isinstance(event, ToolConfigChanged)
    assert event.payload.operation is None
    assert event.payload.target is None
    assert event.payload.status is None
    assert event.payload.persisted is None


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


def test_parse_turn_completed_does_not_default_missing_or_malformed_stop_reason():
    missing = parse_event({"type": "turn_completed"})
    invalid_string = parse_event({"type": "turn_completed", "stop_reason": "not_real"})
    non_string = parse_event({"type": "turn_completed", "stop_reason": 0})

    assert isinstance(missing, TurnCompleted)
    assert isinstance(invalid_string, TurnCompleted)
    assert isinstance(non_string, TurnCompleted)
    assert missing.stop_reason is None
    assert invalid_string.stop_reason is None
    assert non_string.stop_reason is None


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
    assert event.is_error is False


def test_parse_tool_execution_completed_does_not_coerce_missing_or_malformed_is_error():
    missing = parse_event({
        "type": "tool_execution_completed",
        "id": "t1",
        "name": "search",
        "result": "found it",
    })
    malformed = parse_event({
        "type": "tool_execution_completed",
        "id": "t1",
        "name": "search",
        "result": "found it",
        "is_error": "false",
    })
    assert isinstance(missing, ToolExecutionCompleted)
    assert isinstance(malformed, ToolExecutionCompleted)
    assert missing.is_error is None
    assert malformed.is_error is None


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


def test_parse_skill_resolution_failed_with_typed_reason():
    source_uuid = "00000000-0000-4b11-8111-000000000001"
    raw = {
        "type": "skill_resolution_failed",
        "skill_key": {"source_uuid": source_uuid, "skill_name": "email-extractor"},
        "reason": {
            "reason_type": "not_found",
            "key": {"source_uuid": source_uuid, "skill_name": "email-extractor"},
        },
        "reference": f"{source_uuid}/email-extractor",
        "error": f"skill not found: {source_uuid}/email-extractor",
    }
    event = parse_event(raw)
    assert isinstance(event, SkillResolutionFailed)
    assert event.skill_key == SkillKey(
        source_uuid=source_uuid,
        skill_name="email-extractor",
    )
    assert isinstance(event.reason, SkillResolutionFailureReason)
    assert event.reason.reason_type == "not_found"
    assert event.reason.key == SkillKey(
        source_uuid=source_uuid,
        skill_name="email-extractor",
    )
    assert event.reference == f"{source_uuid}/email-extractor"
    assert event.error == f"skill not found: {source_uuid}/email-extractor"


def test_parse_legacy_skill_resolution_failed_payload():
    raw = {
        "type": "skill_resolution_failed",
        "reference": "legacy/ref",
        "error": "missing",
    }
    event = parse_event(raw)
    assert isinstance(event, SkillResolutionFailed)
    assert event.skill_key is None
    assert event.reason is None
    assert event.reference == "legacy/ref"
    assert event.error == "missing"


def test_parse_skill_resolution_failed_does_not_fabricate_malformed_reason():
    raw = {
        "type": "skill_resolution_failed",
        "reason": {"reason_type": 0, "message": "bad"},
        "reference": "legacy/ref",
        "error": "missing",
    }
    event = parse_event(raw)
    assert isinstance(event, SkillResolutionFailed)
    assert event.reason is None


def test_parse_inline_video_content_block():
    block = MeerkatClient._parse_content_block(  # type: ignore[attr-defined]
        {
            "type": "video",
            "media_type": "video/mp4",
            "duration_ms": 12000,
            "source": "inline",
            "data": "AAAA",
        }
    )
    assert block["type"] == "video"
    assert block["media_type"] == "video/mp4"
    assert block["duration_ms"] == 12000
    assert block["data"] == "AAAA"


@pytest.mark.asyncio
async def test_client_comms_send_and_peers_call_expected_rpc_methods():
    client = MeerkatClient()
    calls = []

    async def fake_request(method, params):
        calls.append((method, params))
        if method == "comms/peers":
            return {"peers": [{"name": "agent-a"}]}
        return {"kind": "peer_message_sent", "acked": True}

    client._request = fake_request  # type: ignore[method-assign]

    send_receipt = await client.send("s1", kind="peer_message", to="agent-a", body="hello")
    peers = await client.peers("s1")

    assert send_receipt["kind"] == "peer_message_sent"
    assert peers["peers"] == [{"name": "agent-a"}]
    assert [m for m, _ in calls] == ["comms/send", "comms/peers"]


@pytest.mark.asyncio
async def test_client_read_session_history_calls_expected_rpc_method():
    client = MeerkatClient()
    calls = []

    async def fake_request(method, params):
        calls.append((method, params))
        return {
            "session_id": params["session_id"],
            "session_ref": "history-ref",
            "message_count": 3,
            "offset": params["offset"],
            "limit": params.get("limit"),
            "has_more": False,
            "messages": [
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": "ok", "stop_reason": "end_turn"},
            ],
        }

    client._request = fake_request  # type: ignore[method-assign]

    history = await client.read_session_history("s1", offset=1, limit=2)

    assert history.session_id == "s1"
    assert history.session_ref == "history-ref"
    assert history.offset == 1
    assert history.limit == 2
    assert [m for m, _ in calls] == ["session/history"]
    assert calls[0][1] == {"session_id": "s1", "offset": 1, "limit": 2}
    assert history.messages[1].role == "assistant"


@pytest.mark.asyncio
async def test_client_list_sessions_parses_summary_shape():
    client = MeerkatClient()

    async def fake_request(method, params):
        assert method == "session/list"
        assert params == {"labels": {"team": "sdk"}, "limit": 2, "offset": 1}
        return {
            "sessions": [
                {
                    "session_id": "s1",
                    "session_ref": "realm/s1",
                    "created_at": 1711111111,
                    "updated_at": 1711111222,
                    "message_count": 3,
                    "total_tokens": 77,
                    "labels": {"team": "sdk"},
                    "is_active": False,
                }
            ]
        }

    client._request = fake_request  # type: ignore[method-assign]

    sessions = await client.list_sessions(labels={"team": "sdk"}, limit=2, offset=1)
    assert len(sessions) == 1
    assert isinstance(sessions[0], SessionSummary)
    assert sessions[0].session_id == "s1"
    assert sessions[0].total_tokens == 77
    assert sessions[0].created_at == 1711111111


@pytest.mark.asyncio
async def test_client_read_session_parses_details_shape():
    client = MeerkatClient()

    async def fake_request(method, params):
        assert method == "session/read"
        assert params == {"session_id": "s1"}
        return {
            "session_id": "s1",
            "session_ref": "realm/s1",
            "created_at": 1711111111,
            "updated_at": 1711111222,
            "message_count": 5,
            "labels": {"team": "sdk"},
            "is_active": True,
            "model": "claude-sonnet-4-6",
            "provider": "anthropic",
            "last_assistant_text": "hello",
        }

    client._request = fake_request  # type: ignore[method-assign]

    details = await client.read_session("s1")
    assert isinstance(details, SessionDetails)
    assert details.session_id == "s1"
    assert details.model == "claude-sonnet-4-6"
    assert details.provider == "anthropic"
    assert details.last_assistant_text == "hello"


@pytest.mark.asyncio
async def test_client_models_catalog_and_schedule_wrappers_use_expected_rpc_methods():
    client = MeerkatClient()
    calls = []

    async def fake_request(method, params):
        calls.append((method, params))
        if method == "models/catalog":
            return {
                "contract_version": {"major": 0, "minor": 5, "patch": 1},
                "providers": [],
            }
        if method == "schedule/list":
            return {"schedules": []}
        if method == "schedule/occurrences":
            return {"occurrences": []}
        if method == "schedule/tools":
            return {"tools": [{"name": "meerkat_schedule_list"}]}
        return {"ok": True}

    client._request = fake_request  # type: ignore[method-assign]

    models = await client.get_models_catalog()
    assert models["contract_version"] == {"major": 0, "minor": 5, "patch": 1}

    await client.create_schedule({"name": "test"})
    await client.get_schedule("sch_1")
    await client.list_schedules(labels={"env": "test"}, limit=5, offset=2)
    await client.update_schedule({"schedule_id": "sch_1", "name": "updated"})
    await client.pause_schedule("sch_1")
    await client.resume_schedule("sch_1")
    await client.delete_schedule("sch_1")
    await client.list_schedule_occurrences("sch_1")
    tools = await client.list_schedule_tools()
    assert tools["tools"][0]["name"] == "meerkat_schedule_list"
    await client.call_schedule_tool({"name": "meerkat_schedule_list"})

    assert [method for method, _ in calls] == [
        "models/catalog",
        "schedule/create",
        "schedule/get",
        "schedule/list",
        "schedule/update",
        "schedule/pause",
        "schedule/resume",
        "schedule/delete",
        "schedule/occurrences",
        "schedule/tools",
        "schedule/call",
    ]
    assert calls[3][1] == {"labels": {"env": "test"}, "limit": 5, "offset": 2}


@pytest.mark.asyncio
async def test_session_turn_and_stream_support_full_turn_overrides():
    session_calls = []

    class StubClient:
        async def _start_turn(self, session_id, prompt, **kwargs):
            session_calls.append(("turn", session_id, prompt, kwargs))
            return RunResult(session_id=session_id, text="ok", usage=Usage())

        def _start_turn_streaming(self, session_id, prompt, **kwargs):
            session_calls.append(("stream", session_id, prompt, kwargs))
            return "stream-handle"

    session = Session(
        StubClient(),  # type: ignore[arg-type]
        RunResult(session_id="s1", text="ready", usage=Usage()),
    )

    result = await session.turn(
        "next",
        additional_instructions=["Follow policy A."],
        keep_alive=True,
        model="claude-sonnet-4-6",
        provider="anthropic",
        max_tokens=512,
        system_prompt="System",
        output_schema={"type": "object"},
        structured_output_retries=3,
        provider_params={"reasoning_effort": "low"},
    )
    assert result.text == "ok"

    stream_handle = session.stream(
        "stream it",
        additional_instructions=["Follow policy B."],
        keep_alive=False,
        model="gpt-5.4",
        provider="openai",
        max_tokens=256,
        system_prompt="Stream system",
        output_schema={"type": "object"},
        structured_output_retries=2,
        provider_params={"foo": "bar"},
    )
    assert stream_handle == "stream-handle"
    assert session_calls[0][0] == "turn"
    assert session_calls[0][3]["additional_instructions"] == ["Follow policy A."]
    assert session_calls[0][3]["keep_alive"] is True
    assert session_calls[1][0] == "stream"
    assert session_calls[1][3]["additional_instructions"] == ["Follow policy B."]
    assert session_calls[1][3]["model"] == "gpt-5.4"


@pytest.mark.asyncio
async def test_client_mcp_methods_send_expected_rpc_calls():
    client = MeerkatClient()
    calls = []

    async def fake_request(method, params):
        calls.append((method, params))
        return {
            "session_id": params["session_id"],
            "operation": method.split("/")[1],
            "status": "staged",
            "persisted": params.get("persisted", False),
            "server_name": params.get("server_name"),
            "applied_at_turn": None,
        }

    client._request = fake_request  # type: ignore[method-assign]

    add = await client.mcp_add("s1", "filesystem", {"cmd": "npx"})
    remove = await client.mcp_remove("s1", "filesystem", persisted=True)
    reload = await client.mcp_reload("s1", server_name="filesystem")

    assert add.operation == "add"
    assert remove.operation == "remove"
    assert reload.operation == "reload"
    assert [m for m, _ in calls] == ["mcp/add", "mcp/remove", "mcp/reload"]


@pytest.mark.asyncio
async def test_client_mcp_methods_propagate_request_failures():
    client = MeerkatClient()

    async def fake_request(_method, _params):
        raise MeerkatError("TRANSPORT", "boom")

    client._request = fake_request  # type: ignore[method-assign]

    with pytest.raises(MeerkatError, match="boom"):
        await client.mcp_add("s1", "filesystem", {"cmd": "npx"})
    with pytest.raises(MeerkatError, match="boom"):
        await client.mcp_remove("s1", "filesystem")
    with pytest.raises(MeerkatError, match="boom"):
        await client.mcp_reload("s1", server_name="filesystem")


@pytest.mark.asyncio
async def test_client_mcp_methods_reject_malformed_response():
    client = MeerkatClient()

    async def fake_request(_method, _params):
        return {
            "session_id": "s1",
            "operation": "add",
            "status": "staged",
            "persisted": "false",
        }

    client._request = fake_request  # type: ignore[method-assign]

    with pytest.raises(MeerkatError, match="persisted must be boolean"):
        await client.mcp_add("s1", "filesystem", {"cmd": "npx"})


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


@pytest.mark.asyncio
async def test_client_session_and_mob_observe_methods_use_explicit_rpc_methods():
    client = MeerkatClient()
    calls = []

    async def fake_request(method, params):
        calls.append((method, params))
        return {"stream_id": f"{method}-stream"}

    class DummyDispatcher:
        def subscribe_stream(self, stream_id):
            queue = asyncio.Queue()
            queue.put_nowait(None)
            return queue

        def unsubscribe_stream(self, _stream_id):
            return None

    client._request = fake_request  # type: ignore[method-assign]
    client._dispatcher = DummyDispatcher()  # type: ignore[assignment]

    session_sub = await client.subscribe_session_events("s1")
    member_sub = await client.subscribe_mob_member_events("mob-1", "agent-a")
    mob_sub = await client.subscribe_mob_events("mob-1")

    assert session_sub.stream_id == "session/stream_open-stream"
    assert member_sub.stream_id == "mob/stream_open-stream"
    assert mob_sub.stream_id == "mob/stream_open-stream"
    assert calls == [
        ("session/stream_open", {"session_id": "s1"}),
        ("mob/stream_open", {"mob_id": "mob-1", "agent_identity": "agent-a"}),
        ("mob/stream_open", {"mob_id": "mob-1"}),
    ]


@pytest.mark.asyncio
async def test_client_mob_lifecycle_and_send_methods_use_explicit_rpc_methods():
    client = MeerkatClient()
    calls = []

    async def fake_request(method, params):
        calls.append((method, params))
        if method == "mob/create":
            return {"mob_id": "mob-1"}
        if method == "mob/list":
            return {"mobs": [{"mob_id": "mob-1"}]}
        if method == "mob/status":
            return {"mob_id": "mob-1", "status": "running"}
        if method == "mob/members":
            return {
                "members": [
                    {
                        "agent_identity": "agent-a",
                        "member_ref": _make_member_ref("mob-1", "agent-a"),
                        "profile": "planner",
                    }
                ]
            }
        if method == "mob/spawn":
            return {
                "mob_id": "mob-1",
                "agent_identity": "agent-a",
                "member_ref": _make_member_ref("mob-1", "agent-a"),
            }
        if method == "mob/member_status":
            return {
                "status": "active",
                "tokens_used": 5,
                "is_final": False,
                "agent_runtime_id": {"identity": "agent-a", "generation": 1},
                "fence_token": 7,
                "realtime_attachment_status": "binding_ready",
            }
        if method == "mob/respawn":
            return {
                "status": "completed",
                "receipt": {
                    "identity": "agent-a",
                    "member_ref": _make_member_ref("mob-1", "agent-a"),
                },
            }
        if method == "mob/flows":
            return {"flows": ["incident"]}
        if method == "mob/flow_run":
            return {"run_id": "run-1"}
        if method == "mob/flow_status":
            return {"run": {"status": "running"}}
        if method == "mob/wait_kickoff":
            return {
                "members": [
                    {
                        "agent_identity": "agent-a",
                        "agent_runtime_id": {"identity": "agent-a", "generation": 1},
                        "fence_token": 7,
                        "status": "active",
                        "tokens_used": 3,
                        "is_final": False,
                    }
                ]
            }
        if method == "mob/wait_ready":
            return {
                "members": [
                    {
                        "agent_identity": "agent-a",
                        "agent_runtime_id": {"identity": "agent-a", "generation": 1},
                        "fence_token": 7,
                        "status": "active",
                        "tokens_used": 3,
                        "is_final": False,
                    }
                ]
            }
        if method == "mob/append_system_context":
            return {"mob_id": "mob-1", "agent_identity": "agent-a", "status": "staged"}
        if method == "session/realtime_attachment_status":
            return {"status": "binding_ready"}
        return {}

    client._request = fake_request  # type: ignore[method-assign]
    client.require_capability = lambda _cap: None  # type: ignore[method-assign]

    mob = await client.create_mob(definition={"id": "mob-1", "profiles": {"worker": {"model": "claude-sonnet-4-6"}}})
    assert mob.id == "mob-1"
    assert await client.list_mobs() == [{"mob_id": "mob-1"}]
    assert await client.mob_status("mob-1") == {"mob_id": "mob-1", "status": "running"}
    expected_agent_a_ref = _make_member_ref("mob-1", "agent-a")
    assert await client.list_mob_members("mob-1") == [
        {
            "agent_identity": "agent-a",
            "member_ref": expected_agent_a_ref,
            "profile": "planner",
        }
    ]
    spawn_receipt = await client.spawn_mob_member(
        "mob-1",
        profile="planner",
        agent_identity="agent-a",
    )
    assert spawn_receipt == {
        "mob_id": "mob-1",
        "agent_identity": "agent-a",
        "member_ref": expected_agent_a_ref,
    }
    await client.retire_mob_member("mob-1", "agent-a")
    status = await client.mob_member_status("mob-1", "agent-a")
    assert "agent_runtime_id" not in status
    assert "fence_token" not in status
    assert status["realtime_attachment_status"] == "binding_ready"

    runtime_status = await client.runtime_realtime_attachment_status("session-1")
    assert runtime_status.status == "binding_ready"

    client._request = fake_request  # type: ignore[method-assign]
    await client.respawn_mob_member("mob-1", "agent-a", "hello")
    await client.wire_mob_members("mob-1", "a", "b")
    await client.unwire_mob_members(
        "mob-1",
        "a",
        {"external": {"name": "remote", "peer_id": "ed25519:remote", "address": "inproc://remote"}},
    )
    await client.mob_lifecycle("mob-1", "start")
    wait_members = await client.wait_mob_kickoff(
        "mob-1",
        member_ids=["agent-a"],
        timeout_ms=1234,
    )
    assert wait_members[0]["agent_identity"] == "agent-a"
    assert wait_members[0]["status"] == "active"
    assert "agent_runtime_id" not in wait_members[0]
    assert "fence_token" not in wait_members[0]

    mob_handle = client.mob("mob-1")
    scoped_wait_members = await mob_handle.wait_for_kickoff_complete(timeout_ms=99)
    assert scoped_wait_members[0]["agent_identity"] == "agent-a"
    ready_members = await client.wait_mob_ready(
        "mob-1",
        member_ids=["agent-a"],
        timeout_ms=222,
    )
    assert ready_members[0]["agent_identity"] == "agent-a"
    scoped_ready_members = await mob_handle.wait_for_ready(timeout_ms=88)
    assert scoped_ready_members[0]["agent_identity"] == "agent-a"

    append_result = await client.append_mob_system_context("mob-1", "agent-a", "context")
    assert append_result == {
        "mob_id": "mob-1",
        "agent_identity": "agent-a",
        "status": "staged",
    }
    assert await client.list_mob_flows("mob-1") == ["incident"]
    assert await client.run_mob_flow("mob-1", "incident") == "run-1"
    assert await client.get_mob_flow_status("mob-1", "run-1") == {"status": "running"}
    await client.cancel_mob_flow("mob-1", "run-1")

    assert [method for method, _ in calls] == [
        "mob/create",
        "mob/list",
        "mob/status",
        "mob/members",
        "mob/spawn",
        "mob/retire",
        "mob/member_status",
        "session/realtime_attachment_status",
        "mob/respawn",
        "mob/wire",
        "mob/unwire",
        "mob/lifecycle",
        "mob/wait_kickoff",
        "mob/wait_kickoff",
        "mob/wait_ready",
        "mob/wait_ready",
        "mob/append_system_context",
        "mob/flows",
        "mob/flow_run",
        "mob/flow_status",
        "mob/flow_cancel",
    ]
    assert calls[9][1] == {"mob_id": "mob-1", "member": "a", "peer": {"local": "b"}}
    assert calls[10][1] == {
        "mob_id": "mob-1",
        "member": "a",
        "peer": {
            "external": {
                "name": "remote",
                "peer_id": "ed25519:remote",
                "address": "inproc://remote",
            }
        },
    }
    assert calls[7][1] == {
        "session_id": "session-1",
    }
    assert calls[12][1] == {
        "mob_id": "mob-1",
        "member_ids": ["agent-a"],
        "timeout_ms": 1234,
    }
    assert calls[13][1] == {"mob_id": "mob-1", "timeout_ms": 99}
    assert calls[4][1]["agent_identity"] == "agent-a"


@pytest.mark.asyncio
async def test_realtime_wrappers_and_channel_scaffold() -> None:
    client = MeerkatClient()
    calls: list[tuple[str, dict[str, object]]] = []

    async def fake_request(method: str, params: dict[str, object]) -> dict[str, object]:
        calls.append((method, params))
        if method == "realtime/open_info":
            return {
                "ws_url": "ws://localhost:9999/realtime/ws",
                "open_token": "token-1",
                "expires_at": "2026-04-15T12:00:00Z",
                "target": params["target"],
                "supported_protocol_versions": ["1"],
                "default_protocol_version": "1",
                "capabilities": {
                    "input_kinds": ["text", "audio"],
                    "output_kinds": ["text", "audio"],
                    "turning_modes": ["provider_managed"],
                    "interrupt_supported": True,
                    "transcript_supported": True,
                    "tool_lifecycle_events_supported": False,
                    "video_supported": False,
                },
            }
        if method == "realtime/status":
            return {"status": {"state": "opening", "attempt_count": 0}}
        if method == "realtime/capabilities":
            return {
                "capabilities": {
                    "input_kinds": ["text", "audio"],
                    "output_kinds": ["text", "audio"],
                    "turning_modes": ["provider_managed"],
                    "interrupt_supported": True,
                    "transcript_supported": True,
                    "tool_lifecycle_events_supported": False,
                    "video_supported": False,
                }
            }
        raise AssertionError(f"unexpected method {method}")

    client._request = fake_request  # type: ignore[method-assign]

    session_channel = RealtimeChannel.session(client, "session-1")
    assert session_channel.open_request().target == {
        "type": "session_target",
        "session_id": "session-1",
    }
    assert session_channel.open_request().role == "primary"
    assert session_channel.open_request().turning_mode == "provider_managed"

    open_info = await client.realtime_open_info(session_channel.open_request())
    status = await client.realtime_status({"target": session_channel.target})
    capabilities = await client.realtime_capabilities({"target": session_channel.target})

    scoped_open_info = await session_channel.open_info()
    scoped_status = await session_channel.status()
    scoped_capabilities = await session_channel.capabilities()

    assert open_info.default_protocol_version == "1"
    assert status.status["state"] == "opening"
    assert capabilities.capabilities["turning_modes"] == ["provider_managed"]
    assert scoped_open_info.open_token == "token-1"
    assert scoped_status.status["state"] == "opening"
    assert scoped_capabilities.capabilities["input_kinds"] == ["text", "audio"]
    assert [method for method, _ in calls] == [
        "realtime/open_info",
        "realtime/status",
        "realtime/capabilities",
        "realtime/open_info",
        "realtime/status",
        "realtime/capabilities",
    ]


# W3-H: `RealtimeChannelTarget::MobMember { mob_id, agent_identity }` is
# a first-class wire variant. The SDK builds the MobMember variant
# directly — no pre-resolution of `mob/member_status → current_session_id`,
# no session-id pin — so respawn-driven session rotation does not
# require any SDK round-trip or reconnect. See
# `meerkat-contracts/src/wire/realtime.rs:224-232` for the contract and
# `meerkat-mob/tests/member_realtime_bindings.rs` for the machine-owned
# binding lifecycle this relies on.
@pytest.mark.asyncio
async def test_realtime_channel_mob_member_builds_mob_member_wire_target() -> None:
    client = MeerkatClient()
    calls: list[tuple[str, dict[str, object]]] = []

    async def fake_request(method: str, params: dict[str, object]) -> dict[str, object]:
        calls.append((method, params))
        if method == "realtime/open_info":
            return {
                "ws_url": "ws://localhost:9999/realtime/ws",
                "open_token": "token-mob",
                "expires_at": "2026-04-15T12:00:00Z",
                "target": params["target"],
                "supported_protocol_versions": ["1"],
                "default_protocol_version": "1",
                "capabilities": {
                    "input_kinds": ["text"],
                    "output_kinds": ["text"],
                    "turning_modes": ["explicit_commit"],
                    "interrupt_supported": True,
                    "transcript_supported": True,
                    "tool_lifecycle_events_supported": False,
                    "video_supported": False,
                },
            }
        if method == "realtime/status":
            return {"status": {"state": "opening", "attempt_count": 0}}
        if method == "realtime/capabilities":
            return {
                "capabilities": {
                    "input_kinds": ["text"],
                    "output_kinds": ["text"],
                    "turning_modes": ["explicit_commit"],
                    "interrupt_supported": True,
                    "transcript_supported": True,
                    "tool_lifecycle_events_supported": False,
                    "video_supported": False,
                }
            }
        raise AssertionError(f"unexpected method {method}")

    client._request = fake_request  # type: ignore[method-assign]

    member_channel = RealtimeChannel.mob_member(
        client,
        "mob-1",
        "agent-a",
        role="observer",
        turning_mode="explicit_commit",
    )

    # W3-H: the wire target carries identity — no pre-resolve round-trip,
    # no pinned session id. `open_request()` returns the MobMember
    # variant as-is.
    expected_target = {
        "type": "mob_member",
        "mob_id": "mob-1",
        "agent_identity": "agent-a",
    }
    assert member_channel.target == expected_target
    open_request = member_channel.open_request()
    assert open_request.target == expected_target

    # open_info() sends the MobMember target directly.
    open_info = await member_channel.open_info()
    assert open_info.target == expected_target

    # status() and capabilities() also send the MobMember target with
    # no `mob/member_status` pre-resolve round-trip.
    status_result = await member_channel.status()
    assert status_result.status["state"] == "opening"
    await member_channel.capabilities()

    # Call order encodes the new contract: only `realtime/*` calls
    # cross the wire, never `mob/member_status`. Respawn rotates the
    # bound session inside the server; the SDK is unaware.
    assert [method for method, _ in calls] == [
        "realtime/open_info",
        "realtime/status",
        "realtime/capabilities",
    ]

    # Every outbound target carries the MobMember variant — the SDK
    # never emits the retired `mob_member_target` shape and never
    # emits a `session_target` synthesized from `mob/member_status`.
    for method, params in calls:
        target = params.get("target") if isinstance(params, dict) else None
        assert isinstance(target, dict), f"{method} lost the target"
        assert target.get("type") == "mob_member", (
            f"{method} leaked non-MobMember wire shape {target!r}"
        )
        assert target.get("type") != "mob_member_target", (
            f"{method} leaked retired mob_member_target wire shape"
        )


@pytest.mark.asyncio
async def test_mob_helper_and_respawn_paths_use_identity_native_receipts() -> None:
    client = MeerkatClient()
    calls: list[tuple[str, dict[str, object]]] = []

    async def fake_request(method: str, params: dict[str, object]) -> dict[str, object]:
        calls.append((method, params))
        if method == "mob/spawn_helper":
            return {
                "output": "ok",
                "tokens_used": 1,
                "agent_identity": "helper-a",
                "member_ref": _make_member_ref("mob-1", "helper-a"),
            }
        if method == "mob/fork_helper":
            return {
                "output": "forked",
                "tokens_used": 2,
                "agent_identity": "fork-a",
                "member_ref": _make_member_ref("mob-1", "fork-a"),
            }
        if method == "mob/respawn":
            return {
                "status": "completed",
                "receipt": {
                    "identity": "agent-a",
                    "member_ref": _make_member_ref("mob-1", "agent-a"),
                },
            }
        raise AssertionError(f"unexpected method {method}")

    client._request = fake_request  # type: ignore[method-assign]

    helper = await client.spawn_mob_helper("mob-1", "help", role_name="worker")
    forked = await client.fork_mob_helper("mob-1", "agent-a", "help", role_name="worker")
    respawned = await client.respawn_mob_member("mob-1", "agent-a")

    # App-facing receipts expose only `member_ref`; binding-era
    # `agent_runtime_id` / `fence_token` / `previous_fence_token` are
    # retired per dogma #10.
    assert helper["agent_identity"] == "helper-a"
    assert helper["member_ref"] == _make_member_ref("mob-1", "helper-a")
    assert "agent_runtime_id" not in helper
    assert "fence_token" not in helper
    assert forked["agent_identity"] == "fork-a"
    assert forked["member_ref"] == _make_member_ref("mob-1", "fork-a")
    assert "agent_runtime_id" not in forked
    assert "fence_token" not in forked
    assert respawned["receipt"]["agent_identity"] == "agent-a"
    assert respawned["receipt"]["member_ref"] == _make_member_ref("mob-1", "agent-a")
    assert "agent_runtime_id" not in respawned["receipt"]
    assert "fence_token" not in respawned["receipt"]
    assert "previous_fence_token" not in respawned["receipt"]
    assert [method for method, _ in calls] == [
        "mob/spawn_helper",
        "mob/fork_helper",
        "mob/respawn",
    ]


@pytest.mark.asyncio
async def test_send_mob_member_content_uses_canonical_host_member_send_lane() -> None:
    client = MeerkatClient()
    calls: list[tuple[str, dict[str, object]]] = []

    async def fake_request(method: str, params: dict[str, object]) -> dict[str, object]:
        calls.append((method, params))
        return {
            "agent_identity": "agent-a",
            "member_ref": _make_member_ref("mob-1", "agent-a"),
            "handling_mode": "steer",
        }

    client._request = fake_request  # type: ignore[method-assign]

    receipt = await client.send_mob_member_content(
        "mob-1",
        "agent-a",
        "hello reviewer",
        handling_mode="steer",
        render_metadata={"class": "peer_request", "salience": "urgent"},
    )

    assert receipt == {
        "agent_identity": "agent-a",
        "member_ref": _make_member_ref("mob-1", "agent-a"),
        "handling_mode": "steer",
    }
    assert calls == [
        (
            "mob/member_send",
            {
                "mob_id": "mob-1",
                "agent_identity": "agent-a",
                "content": "hello reviewer",
                "handling_mode": "steer",
                "render_metadata": {"class": "peer_request", "salience": "urgent"},
            },
        )
    ]


@pytest.mark.asyncio
async def test_send_mob_member_content_rejects_malformed_receipt() -> None:
    client = MeerkatClient()

    async def fake_request(_method: str, _params: dict[str, object]) -> dict[str, object]:
        return {"handling_mode": "queue"}

    client._request = fake_request  # type: ignore[method-assign]

    with pytest.raises(MeerkatError, match="missing member_ref"):
        await client.send_mob_member_content("mob-1", "agent-a", "hello reviewer")
