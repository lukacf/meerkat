import asyncio
"""Conformance tests for Meerkat Python SDK types and events."""

import pytest

from meerkat import (
    CONTRACT_VERSION,
    Capability,
    McpAddParams,
    McpLiveOpResponse,
    MeerkatClient,
    RunResult,
    SchemaWarning,
    SessionAssistantBlock,
    SessionHistory,
    SessionInfo,
    SessionMessage,
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
    ToolConfigChanged,
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


def test_skill_key_export():
    key = SkillKey(source_uuid="abc-123", skill_name="my-skill")
    assert key.source_uuid == "abc-123"
    assert key.skill_name == "my-skill"


def test_session_info():
    info = SessionInfo(session_id="abc", is_active=True, message_count=5)
    assert info.session_id == "abc"
    assert info.is_active is True
    assert info.message_count == 5


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


def test_live_mcp_contract_types_exported():
    add = McpAddParams(session_id="s1", server_name="filesystem", persisted=False)
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
    assert event.payload.operation == ""
    assert event.payload.target == ""
    assert event.payload.status == ""
    assert event.payload.persisted is False
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
    assert event.payload.persisted is False


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
        ("mob/stream_open", {"mob_id": "mob-1", "member_id": "agent-a"}),
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
            return {"members": [{"meerkat_id": "agent-a"}]}
        if method == "mob/flows":
            return {"flows": ["incident"]}
        if method == "mob/flow_run":
            return {"run_id": "run-1"}
        if method == "mob/flow_status":
            return {"run": {"status": "running"}}
        return {}

    client._request = fake_request  # type: ignore[method-assign]
    client.require_capability = lambda _cap: None  # type: ignore[method-assign]

    mob = await client.create_mob(prefab="office")
    assert mob.id == "mob-1"
    assert await client.list_mobs() == [{"mob_id": "mob-1"}]
    assert await client.mob_status("mob-1") == {"mob_id": "mob-1", "status": "running"}
    assert await client.list_mob_members("mob-1") == [{"meerkat_id": "agent-a"}]
    await client.spawn_mob_member("mob-1", profile="planner", meerkat_id="agent-a")
    await client.retire_mob_member("mob-1", "agent-a")
    await client.respawn_mob_member("mob-1", "agent-a", "hello")
    await client.wire_mob_members("mob-1", "a", "b")
    await client.unwire_mob_members("mob-1", "a", "b")
    await client.mob_lifecycle("mob-1", "start")
    await client.send_mob_message("mob-1", "agent-a", "hello")
    await client.append_mob_system_context("mob-1", "agent-a", "context")
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
        "mob/respawn",
        "mob/wire",
        "mob/unwire",
        "mob/lifecycle",
        "mob/send",
        "mob/append_system_context",
        "mob/flows",
        "mob/flow_run",
        "mob/flow_status",
        "mob/flow_cancel",
    ]
