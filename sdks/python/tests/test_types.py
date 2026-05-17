import asyncio
"""Conformance tests for Meerkat Python SDK types and events."""

import base64
import json
from pathlib import Path
from typing import Literal, get_args, get_origin, get_type_hints

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
    AgentErrorReason,
    AgentErrorReport,
    BackgroundJobCompleted,
    BoundaryAppliedToolConfigChangeStatus,
    BudgetWarning,
    CompactionStarted,
    Event,
    ExtractionFailed,
    ExtractionSucceeded,
    HookDenied,
    InteractionComplete,
    Retrying,
    RunCompleted,
    RunFailed,
    RunStarted,
    SkillsResolved,
    SkillResolutionFailed,
    SkillResolutionFailureReason,
    ToolConfigChanged,
    TextDelta,
    ToolCallRequested,
    ToolExecutionCompleted,
    ToolResultReceived,
    TurnCompleted,
    TurnStarted,
    UnknownEvent,
    parse_event,
)
from meerkat.generated.types import (
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


def test_model_profile_type_uses_wire_web_search_key():
    from meerkat import ModelProfile
    from meerkat.types import ResolvedModelCapabilities

    model_profile_fields = set(ModelProfile.__annotations__)
    assert "supports_web_search" in model_profile_fields
    assert "web_search" not in model_profile_fields

    resolved_fields = set(ResolvedModelCapabilities.__annotations__)
    assert "web_search" in resolved_fields


def test_generated_runtime_state_result_carries_state():
    result = GeneratedRuntimeStateResult(state="idle")

    assert result.state == "idle"


def test_generated_mob_contract_types_include_spawn_and_turn_start_shapes():
    from meerkat.generated.types import (
        MobMemberStatusResult as GeneratedMobMemberStatusResult,
        MobSpawnParams as GeneratedMobSpawnParams,
        MobSpawnResult as GeneratedMobSpawnResult,
        MobTurnStartParams as GeneratedMobTurnStartParams,
        WireBudgetSplitPolicy as GeneratedWireBudgetSplitPolicy,
        WireMemberLaunchMode as GeneratedWireMemberLaunchMode,
        WireMobProfile as GeneratedWireMobProfile,
        WireMobToolConfig as GeneratedWireMobToolConfig,
        WireToolAccessPolicy as GeneratedWireToolAccessPolicy,
        WireToolFilter as GeneratedWireToolFilter,
    )

    spawn = GeneratedMobSpawnParams(
        mob_id="mob-1",
        profile="worker",
        agent_identity="worker-1",
    )
    assert spawn.mob_id == "mob-1"

    spawn_with_advanced = GeneratedMobSpawnParams(
        mob_id="mob-1",
        profile="worker",
        agent_identity="worker-2",
        launch_mode={"mode": "fresh"},
        tool_access_policy={"type": "allow_list", "value": ["grep"]},
        budget_split_policy={"type": "remaining"},
        inherited_tool_filter={"Allow": ["grep"]},
        override_profile=GeneratedWireMobProfile(model="claude-sonnet-4-6"),
    )
    assert spawn_with_advanced.launch_mode == {"mode": "fresh"}
    spawn_hints = get_type_hints(GeneratedMobSpawnParams)
    assert "Any" not in str(spawn_hints["launch_mode"])
    assert "Any" not in str(spawn_hints["tool_access_policy"])
    assert "Any" not in str(spawn_hints["budget_split_policy"])
    assert "Any" not in str(spawn_hints["inherited_tool_filter"])
    assert "WireMobProfile" in str(spawn_hints["override_profile"])
    tool_config_hints = get_type_hints(GeneratedWireMobToolConfig)
    assert "rust_bundles" not in tool_config_hints
    assert GeneratedWireMemberLaunchMode is not None
    assert GeneratedWireToolAccessPolicy is not None
    assert GeneratedWireBudgetSplitPolicy is not None
    assert GeneratedWireToolFilter is not None

    turn = GeneratedMobTurnStartParams(
        mob_id="mob-1",
        agent_identity="worker-1",
        prompt=[{"type": "text", "text": "continue"}],
        model="gpt-test",
        clear_provider_params=True,
    )
    assert turn.prompt == [{"type": "text", "text": "continue"}]
    assert turn.model == "gpt-test"

    result = GeneratedMobSpawnResult(
        mob_id="mob-1",
        agent_identity="worker-1",
        member_ref="opaque-member-ref",
    )
    assert result.member_ref == "opaque-member-ref"

    status = GeneratedMobMemberStatusResult(
        status="active",
        tokens_used=0,
        is_final=False,
    )
    assert status.status == "active"


def test_generated_mob_spawn_many_preserves_nested_contract_types():
    from meerkat.generated.types import (
        MobSpawnManyFailedResult as GeneratedMobSpawnManyFailedResult,
        MobSpawnManyFailureCause as GeneratedMobSpawnManyFailureCause,
        MobSpawnManyParams as GeneratedMobSpawnManyParams,
        MobSpawnManyResult as GeneratedMobSpawnManyResult,
        MobSpawnManyResultEntry as GeneratedMobSpawnManyResultEntry,
        MobSpawnManySpawnedResult as GeneratedMobSpawnManySpawnedResult,
        MobSpawnSpecParams as GeneratedMobSpawnSpecParams,
    )

    params_hints = get_type_hints(GeneratedMobSpawnManyParams)
    assert get_origin(params_hints["specs"]) is list
    assert get_args(params_hints["specs"]) == (GeneratedMobSpawnSpecParams,)

    spec_hints = get_type_hints(GeneratedMobSpawnSpecParams)
    assert "Any" not in str(spec_hints["initial_message"])
    assert "Any" not in str(spec_hints["backend"])
    assert "WireAuthBindingRef" in str(spec_hints["auth_binding"])

    result_hints = get_type_hints(GeneratedMobSpawnManyResult)
    assert get_origin(result_hints["results"]) is list
    assert get_args(result_hints["results"]) == (GeneratedMobSpawnManyResultEntry,)

    entry_hints = get_type_hints(GeneratedMobSpawnManyResultEntry)
    assert get_origin(entry_hints["status"]) is Literal
    assert get_args(entry_hints["status"]) == ("spawned", "failed")
    assert get_args(entry_hints["result"]) == (
        GeneratedMobSpawnManySpawnedResult,
        GeneratedMobSpawnManyFailedResult,
    )
    failure_hints = get_type_hints(GeneratedMobSpawnManyFailedResult)
    assert get_origin(failure_hints["cause"]) is Literal
    assert "profile_not_found" in get_args(failure_hints["cause"])
    assert GeneratedMobSpawnManyFailureCause == failure_hints["cause"]

    spec = GeneratedMobSpawnSpecParams(
        profile="worker",
        agent_identity="worker-1",
        initial_message="hello",
    )
    params = GeneratedMobSpawnManyParams(mob_id="mob-1", specs=[spec])
    assert params.specs[0].agent_identity == "worker-1"

    entry = GeneratedMobSpawnManyResultEntry(
        status="spawned",
        result=GeneratedMobSpawnManySpawnedResult(
            agent_identity="worker-1",
            member_ref="opaque-member-ref",
        ),
    )
    failure = GeneratedMobSpawnManyResultEntry(
        status="failed",
        result=GeneratedMobSpawnManyFailedResult(
            cause="profile_not_found",
            message="profile missing",
        ),
    )
    result = GeneratedMobSpawnManyResult(results=[entry, failure])
    assert result.results[0].result.member_ref == "opaque-member-ref"
    assert result.results[1].result.cause == "profile_not_found"
    assert result.results[1].result.message == "profile missing"


def test_generated_mob_wire_members_batch_preserves_report_types():
    from meerkat.generated.types import (
        MobWireMembersBatchEdge as GeneratedMobWireMembersBatchEdge,
        MobWireMembersBatchParams as GeneratedMobWireMembersBatchParams,
        MobWireMembersBatchResult as GeneratedMobWireMembersBatchResult,
    )

    params_hints = get_type_hints(GeneratedMobWireMembersBatchParams)
    assert get_origin(params_hints["edges"]) is list
    assert get_args(params_hints["edges"]) == (GeneratedMobWireMembersBatchEdge,)

    report_hints = get_type_hints(GeneratedMobWireMembersBatchResult)
    assert report_hints["requested"] is int
    assert get_args(report_hints["wired"]) == (GeneratedMobWireMembersBatchEdge,)
    assert get_args(report_hints["already_wired"]) == (GeneratedMobWireMembersBatchEdge,)

    report = GeneratedMobWireMembersBatchResult(
        requested=2,
        wired=[GeneratedMobWireMembersBatchEdge(a="lead", b="worker")],
        already_wired=[GeneratedMobWireMembersBatchEdge(a="lead", b="reviewer")],
    )
    assert report.wired[0].a == "lead"
    assert report.already_wired[0].b == "reviewer"


@pytest.mark.asyncio
async def test_spawn_mob_members_preserves_generated_result_envelope_failures():
    from meerkat import MobSpawnManyResult as PublicMobSpawnManyResult
    from meerkat import MobSpawnManyResultEntry as PublicMobSpawnManyResultEntry
    from meerkat.generated.types import MobSpawnManyResult as GeneratedMobSpawnManyResult
    from meerkat.generated.types import (
        MobSpawnManyResultEntry as GeneratedMobSpawnManyResultEntry,
    )

    assert PublicMobSpawnManyResult is GeneratedMobSpawnManyResult
    assert PublicMobSpawnManyResultEntry is GeneratedMobSpawnManyResultEntry

    client = MeerkatClient()

    async def fake_request(method: str, params: dict[str, object]) -> dict[str, object]:
        assert method == "mob/spawn_many"
        assert params == {
            "mob_id": "mob-1",
            "specs": [{"profile": "worker", "agent_identity": "worker-1"}],
        }
        return {
            "results": [
                {
                    "status": "spawned",
                    "result": {
                        "agent_identity": "worker-1",
                        "member_ref": _make_member_ref("mob-1", "worker-1"),
                    },
                },
                {
                    "status": "failed",
                    "result": {
                        "cause": "profile_not_found",
                        "message": "profile missing",
                    },
                },
            ]
        }

    client._request = fake_request  # type: ignore[method-assign]

    result = await client.spawn_mob_members(
        "mob-1",
        [{"profile": "worker", "agent_identity": "worker-1"}],
    )

    assert isinstance(result, GeneratedMobSpawnManyResult)
    assert [entry.status for entry in result.results] == ["spawned", "failed"]
    assert result.results[0].result.agent_identity == "worker-1"
    assert result.results[0].result.member_ref == _make_member_ref("mob-1", "worker-1")
    assert result.results[1].result.cause == "profile_not_found"
    assert result.results[1].result.message == "profile missing"


@pytest.mark.asyncio
async def test_mob_wire_members_batch_normalizes_payload_and_parses_report():
    from meerkat.generated.types import MobWireMembersBatchEdge

    client = MeerkatClient()
    calls = []

    async def fake_request(method: str, params: dict[str, object]) -> dict[str, object]:
        calls.append((method, params))
        return {
            "requested": 3,
            "wired": [{"a": "lead", "b": "worker"}],
            "already_wired": [{"a": "lead", "b": "reviewer"}],
        }

    client._request = fake_request  # type: ignore[method-assign]

    report = await client.mob_wire_members_batch(
        "mob-1",
        [
            ("lead", "worker"),
            ["lead", "reviewer"],
            MobWireMembersBatchEdge(a="lead", b="critic"),
        ],
    )

    assert calls == [
        (
            "mob/wire_members_batch",
            {
                "mob_id": "mob-1",
                "edges": [
                    {"a": "lead", "b": "worker"},
                    {"a": "lead", "b": "reviewer"},
                    {"a": "lead", "b": "critic"},
                ],
            },
        )
    ]
    assert report.requested == 3
    assert report.wired == [MobWireMembersBatchEdge(a="lead", b="worker")]
    assert report.already_wired == [MobWireMembersBatchEdge(a="lead", b="reviewer")]


@pytest.mark.asyncio
async def test_mob_wire_members_batch_convenience_accepts_dict_edges():
    client = MeerkatClient()

    async def fake_request(method: str, params: dict[str, object]) -> dict[str, object]:
        assert method == "mob/wire_members_batch"
        assert params == {
            "mob_id": "mob-1",
            "edges": [{"a": "lead", "b": "worker"}],
        }
        return {
            "requested": 1,
            "wired": [],
            "already_wired": [{"a": "lead", "b": "worker"}],
        }

    client._request = fake_request  # type: ignore[method-assign]

    report = await client.mob("mob-1").wire_members_batch(
        [{"a": "lead", "b": "worker"}]
    )

    assert report.requested == 1
    assert report.wired == []
    assert report.already_wired[0].a == "lead"


@pytest.mark.asyncio
async def test_mob_wire_members_batch_rejects_malformed_report():
    client = MeerkatClient()

    async def fake_request(method: str, params: dict[str, object]) -> dict[str, object]:
        assert method == "mob/wire_members_batch"
        return {"requested": 1, "wired": [{"a": "lead"}], "already_wired": []}

    client._request = fake_request  # type: ignore[method-assign]

    with pytest.raises(MeerkatError, match="edge endpoints"):
        await client.mob_wire_members_batch("mob-1", [("lead", "worker")])


@pytest.mark.asyncio
async def test_spawn_mob_members_rejects_malformed_result_envelopes():
    malformed_responses = [
        {"results": [{"ok": True, "agent_identity": "worker-1", "member_ref": "ref-worker-1"}]},
        {"results": [{"status": "spawned"}]},
        {
            "results": [
                {
                    "status": "ok",
                    "result": {"agent_identity": "worker-1", "member_ref": "ref-worker-1"},
                }
            ]
        },
        {"results": [{"status": "spawned", "result": {"agent_identity": "worker-1"}}]},
        {"results": [{"status": "failed", "result": {"message": "profile missing"}}]},
        {
            "results": [
                {
                    "status": "failed",
                    "result": {"cause": "future_failure", "message": "profile missing"},
                }
            ]
        },
        {"results": [{"status": "failed", "result": {"message": ""}}]},
        {
            "results": [
                {
                    "status": "spawned",
                    "result": {
                        "agent_identity": "worker-1",
                        "member_ref": "ref-worker-1",
                    },
                    "ok": True,
                }
            ]
        },
        {
            "results": [
                {
                    "status": "failed",
                    "result": {"cause": "profile_not_found", "message": "profile missing"},
                    "error": "legacy profile missing",
                }
            ]
        },
        {
            "results": [
                {
                    "status": "spawned",
                    "result": {
                        "agent_identity": "worker-1",
                        "member_ref": "ref-worker-1",
                        "ok": True,
                    },
                }
            ]
        },
        {
            "results": [
                {
                    "status": "failed",
                    "result": {
                        "cause": "profile_not_found",
                        "message": "profile missing",
                        "error": "legacy profile missing",
                    },
                }
            ]
        },
        {},
    ]

    for response in malformed_responses:
        client = MeerkatClient()

        async def fake_request(method: str, params: dict[str, object]) -> dict[str, object]:
            assert method == "mob/spawn_many"
            return response

        client._request = fake_request  # type: ignore[method-assign]

        with pytest.raises(MeerkatError, match="mob/spawn_many"):
            await client.spawn_mob_members(
                "mob-1",
                [{"profile": "worker", "agent_identity": "worker-1"}],
            )


def test_generated_mob_create_ensure_reconcile_preserve_nested_param_types():
    from meerkat.generated.types import (
        MobCreateParams as GeneratedMobCreateParams,
        MobDefinitionInput as GeneratedMobDefinitionInput,
        MobEnsureMemberParams as GeneratedMobEnsureMemberParams,
        MobMemberSpecWire as GeneratedMobMemberSpecWire,
        MobReconcileParams as GeneratedMobReconcileParams,
    )

    create_hints = get_type_hints(GeneratedMobCreateParams)
    assert create_hints["definition"] is GeneratedMobDefinitionInput

    ensure_hints = get_type_hints(GeneratedMobEnsureMemberParams)
    assert ensure_hints["spec"] is GeneratedMobMemberSpecWire

    reconcile_hints = get_type_hints(GeneratedMobReconcileParams)
    assert "MobMemberSpecWire" in str(reconcile_hints["desired"])
    assert "Any" not in str(reconcile_hints["desired"])

    definition = GeneratedMobDefinitionInput(
        id="mob-1",
        profiles={"worker": {"model": "claude-sonnet-4-6"}},
    )
    create = GeneratedMobCreateParams(definition=definition)
    assert create.definition.id == "mob-1"

    spec = GeneratedMobMemberSpecWire(profile="worker", agent_identity="worker-1")
    ensure = GeneratedMobEnsureMemberParams(mob_id="mob-1", spec=spec)
    assert ensure.spec.agent_identity == "worker-1"

    reconcile = GeneratedMobReconcileParams(mob_id="mob-1", desired=[spec])
    assert reconcile.desired[0].profile == "worker"


def test_generated_mob_member_result_helpers_preserve_schema_types():
    from meerkat.generated.types import (
        MobEnsureMemberResult as GeneratedMobEnsureMemberResult,
        MobMemberListEntryWire as GeneratedMobMemberListEntryWire,
        MobMembersResult as GeneratedMobMembersResult,
        MobSpawnReceiptWire as GeneratedMobSpawnReceiptWire,
        MobSubmitWorkParams as GeneratedMobSubmitWorkParams,
        WireMemberRef as GeneratedWireMemberRef,
        WireMemberState as GeneratedWireMemberState,
        WireMobMemberStatus as GeneratedWireMobMemberStatus,
        WireMobRuntimeMode as GeneratedWireMobRuntimeMode,
    )

    assert GeneratedWireMemberRef is str
    assert get_args(GeneratedWireMobRuntimeMode) == (
        "autonomous_host",
        "turn_driven",
    )
    assert get_args(GeneratedWireMemberState) == ("active", "retiring")
    assert get_args(GeneratedWireMobMemberStatus) == (
        "active",
        "retiring",
        "broken",
        "completed",
        "unknown",
    )

    spawn_hints = get_type_hints(GeneratedMobSpawnReceiptWire)
    assert spawn_hints["member_ref"] is GeneratedWireMemberRef

    submit_hints = get_type_hints(GeneratedMobSubmitWorkParams)
    assert submit_hints["member_ref"] is GeneratedWireMemberRef

    member_hints = get_type_hints(GeneratedMobMemberListEntryWire)
    assert member_hints["member_ref"] is GeneratedWireMemberRef
    assert member_hints["runtime_mode"] == GeneratedWireMobRuntimeMode
    assert member_hints["state"] == GeneratedWireMemberState
    assert member_hints["status"] == GeneratedWireMobMemberStatus

    member_ref = _make_member_ref("mob-1", "worker-1")
    member = GeneratedMobMemberListEntryWire(
        agent_identity="worker-1",
        member_ref=member_ref,
        role="worker",
        runtime_mode="turn_driven",
        state="active",
        status="active",
        is_final=False,
    )
    members = GeneratedMobMembersResult(mob_id="mob-1", members=[member])
    assert members.members[0].member_ref == member_ref

    spawned = GeneratedMobEnsureMemberResult(
        outcome={
            "spawned": GeneratedMobSpawnReceiptWire(
                agent_identity="worker-1",
                member_ref=member_ref,
            ),
        },
    )
    assert spawned.outcome["spawned"].member_ref == member_ref

    submit = GeneratedMobSubmitWorkParams(
        member_ref=members.members[0].member_ref,
        content="continue",
    )
    assert submit.member_ref == member_ref


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


def test_parse_run_result_extraction_error():
    raw = {
        "session_id": "s1",
        "text": "main answer",
        "turns": 1,
        "tool_calls": 0,
        "usage": {"input_tokens": 10, "output_tokens": 5},
        "extraction_error": {
            "last_output": "main answer",
            "attempts": 2,
            "reason": "Invalid JSON",
        },
    }
    result = MeerkatClient._parse_run_result(raw)
    assert result.extraction_error is not None
    assert result.extraction_error.last_output == "main answer"
    assert result.extraction_error.attempts == 2
    assert result.extraction_error.reason == "Invalid JSON"


def test_parse_run_result_rejects_malformed_counters():
    with pytest.raises(MeerkatError, match="missing usage"):
        MeerkatClient._parse_run_result(
            {"session_id": "s1", "text": "ok", "turns": 1, "tool_calls": 0}
        )

    with pytest.raises(MeerkatError, match="turns must be number"):
        MeerkatClient._parse_run_result(
            {
                "session_id": "s1",
                "text": "ok",
                "turns": "1",
                "tool_calls": 0,
                "usage": {"input_tokens": 1, "output_tokens": 1},
            }
        )

    with pytest.raises(MeerkatError, match="usage.input_tokens must be number"):
        MeerkatClient._parse_run_result(
            {
                "session_id": "s1",
                "text": "ok",
                "turns": 1,
                "tool_calls": 0,
                "usage": {"input_tokens": "oops", "output_tokens": 1},
            }
        )


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


@pytest.mark.asyncio
async def test_peer_response_terminal_uses_canonical_peer_id_and_correlation() -> None:
    from meerkat import PeerCorrelationId, PeerId

    client = MeerkatClient()
    seen: list[tuple[str, dict[str, object]]] = []

    async def fake_request(method: str, params: dict[str, object]) -> dict[str, object]:
        seen.append((method, params))
        return {"status": "accepted"}

    client._request = fake_request  # type: ignore[method-assign]

    await client.send_peer_response_terminal(
        "sess-1",
        PeerId("00000000-0000-4000-8000-000000000161"),
        PeerCorrelationId("00000000-0000-4000-8000-000000000162"),
        "completed",
        {"ok": True},
        display_name="analyst",
    )

    assert seen == [
        (
            "session/peer_response_terminal",
            {
                "session_id": "sess-1",
                "peer_id": "00000000-0000-4000-8000-000000000161",
                "request_id": "00000000-0000-4000-8000-000000000162",
                "status": "completed",
                "result": {"ok": True},
                "display_name": "analyst",
            },
        )
    ]
    assert "peer_name" not in seen[0][1]


def test_live_mcp_contract_types_exported():
    add = McpAddParams(
        session_id="s1",
        server_config={
            "name": "filesystem",
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem"],
        },
        persisted=False,
    )
    response = McpLiveOpResponse(
        session_id="s1",
        operation="remove",
        status="staged",
        persisted=False,
        applied_at_turn=5,
    )
    assert add.server_config["name"] == "filesystem"
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


def test_parse_interaction_complete_structured_output():
    raw = {
        "type": "interaction_complete",
        "interaction_id": "i1",
        "result": "{\"answer\":42}",
        "structured_output": {"answer": 42},
    }
    event = parse_event(raw)
    assert isinstance(event, InteractionComplete)
    assert event.interaction_id == "i1"
    assert event.structured_output == {"answer": 42}


def test_parse_scoped_mob_event_omits_runtime_binding_atoms():
    raw = {
        "scope_id": "mob:writer",
        "scope_path": [
            {
                "scope": "mob_member",
                "flow_run_id": "run_1",
                "agent_identity": "writer",
                "agent_runtime_id": "writer:1",
                "fence_token": 1,
                "generation": 1,
            }
        ],
        "event": {"type": "text_delta", "delta": "hi"},
    }

    event = parse_event(raw)

    assert event.scope_id == "mob:writer"
    assert event.scope_path == [
        {
            "scope": "mob_member",
            "flow_run_id": "run_1",
            "agent_identity": "writer",
        }
    ]


def test_parse_attributed_mob_event_omits_source_fence_token():
    event = MeerkatClient._parse_attributed_mob_event(
        {
            "source": "writer",
            "source_fence_token": 7,
            "role": "worker",
            "envelope": {
                "payload": {"type": "text_delta", "delta": "hi"},
            },
        }
    )

    assert event.source == "writer"
    assert not hasattr(event, "source_fence_token")


def test_parse_event_envelope_uses_typed_source_not_legacy_source_id():
    event = MeerkatClient._parse_agent_event_envelope(
        {
            "source": {
                "type": "session",
                "session_id": "00000000-0000-4000-8000-000000000001",
            },
            "source_id": "session:not-a-uuid",
            "payload": {"type": "text_delta", "delta": "hi"},
        }
    )

    assert event.source is not None
    assert event.source.type == "session"
    assert event.source.session_id == "00000000-0000-4000-8000-000000000001"
    assert event.source_id == "session:not-a-uuid"


def test_parse_event_envelope_does_not_classify_legacy_session_string():
    event = MeerkatClient._parse_agent_event_envelope(
        {
            "source_id": "session:00000000-0000-4000-8000-000000000001",
            "payload": {"type": "text_delta", "delta": "hi"},
        }
    )

    assert event.source is None
    assert event.source_id == "session:00000000-0000-4000-8000-000000000001"


def test_parse_tool_config_changed():
    raw = {
        "type": "tool_config_changed",
        "payload": {
            "operation": "remove",
            "target": "filesystem",
            "status": "staged",
            "status_info": {
                "kind": "boundary_applied",
                "base_changed": True,
                "visible_changed": False,
                "revision": 9,
            },
            "persisted": False,
            "applied_at_turn": 7,
        },
    }
    event = parse_event(raw)
    assert isinstance(event, ToolConfigChanged)
    assert event.payload.operation == "remove"
    assert event.payload.target == "filesystem"
    assert event.payload.status == "staged"
    assert isinstance(event.payload.status_info, BoundaryAppliedToolConfigChangeStatus)
    assert event.payload.status_info.base_changed is True
    assert event.payload.status_info.visible_changed is False
    assert event.payload.status_info.revision == 9
    assert event.payload.persisted is False
    assert event.payload.applied_at_turn == 7


def test_parse_tool_config_changed_with_malformed_payload():
    raw = {
        "type": "tool_config_changed",
        "payload": "not-an-object",
    }
    event = parse_event(raw)
    assert isinstance(event, UnknownEvent)
    assert event.type == "malformed_event"
    assert event.data == raw


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
    assert isinstance(event, UnknownEvent)
    assert event.type == "malformed_event"
    assert event.data == raw


def test_parse_background_job_completed_uses_typed_terminal_status():
    raw = {
        "type": "background_job_completed",
        "job_id": "j_123",
        "display_name": "sleep 2",
        "status": "completed",
        "terminal_status": "failed",
        "detail": "exit_code: 1",
    }
    event = parse_event(raw)
    assert isinstance(event, BackgroundJobCompleted)
    assert event.job_id == "j_123"
    assert event.display_name == "sleep 2"
    assert event.legacy_status == "completed"
    assert event.terminal_status == "failed"
    assert event.detail == "exit_code: 1"


def test_parse_background_job_completed_allows_absent_legacy_status():
    raw = {
        "type": "background_job_completed",
        "job_id": "j_123",
        "display_name": "sleep 2",
        "terminal_status": "failed",
        "detail": "exit_code: 1",
    }
    event = parse_event(raw)
    assert isinstance(event, BackgroundJobCompleted)
    assert event.legacy_status is None
    assert event.terminal_status == "failed"


def test_parse_background_job_completed_ignores_malformed_legacy_status():
    raw = {
        "type": "background_job_completed",
        "job_id": "j_123",
        "display_name": "sleep 2",
        "status": 0,
        "terminal_status": "failed",
        "detail": "exit_code: 1",
    }
    event = parse_event(raw)
    assert isinstance(event, BackgroundJobCompleted)
    assert event.legacy_status is None
    assert event.terminal_status == "failed"


def test_parse_background_job_completed_requires_typed_terminal_status():
    raw = {
        "type": "background_job_completed",
        "job_id": "j_123",
        "display_name": "sleep 2",
        "status": "completed",
        "detail": "exit_code: 0",
    }
    event = parse_event(raw)
    assert isinstance(event, UnknownEvent)
    assert event.type == "malformed_event"
    assert event.data == raw


def test_parse_background_job_completed_rejects_unknown_terminal_status():
    raw = {
        "type": "background_job_completed",
        "job_id": "j_123",
        "display_name": "sleep 2",
        "status": "completed",
        "terminal_status": "success",
        "detail": "exit_code: 0",
    }
    event = parse_event(raw)
    assert isinstance(event, UnknownEvent)
    assert event.type == "malformed_event"
    assert event.data == raw


def test_background_job_completed_constructor_requires_terminal_status():
    with pytest.raises(TypeError):
        BackgroundJobCompleted(
            job_id="j_123",
            display_name="sleep 2",
            detail="exit_code: 0",
        )


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


def test_parse_run_failed_preserves_typed_terminal_cause_report():
    raw = {
        "type": "run_failed",
        "session_id": "s1",
        "error_class": "terminal",
        "error": "display text changed by caller",
        "error_report": {
            "class": "llm",
            "message": "machine terminalized LLM failure",
            "reason": {
                "reason_type": "turn_terminal_cause",
                "outcome": "failed",
                "cause_kind": "llm_failure",
            },
        },
    }

    event = parse_event(raw)

    assert isinstance(event, RunFailed)
    assert event.error == "display text changed by caller"
    assert isinstance(event.error_report, AgentErrorReport)
    assert event.error_report.class_ == "llm"
    assert event.error_report.message == "machine terminalized LLM failure"
    assert isinstance(event.error_report.reason, AgentErrorReason)
    assert event.error_report.reason.reason_type == "turn_terminal_cause"
    assert event.error_report.reason.outcome == "failed"
    assert event.error_report.reason.cause_kind == "llm_failure"


def test_parse_run_failed_does_not_infer_terminal_cause_from_display_fields():
    raw = {
        "type": "run_failed",
        "session_id": "s1",
        "error_class": "llm",
        "error": "LLM failure terminal turn",
        "error_report": {
            "class": "llm",
            "message": "LLM failure terminal turn",
            "reason": {
                "reason_type": "turn_terminal_cause",
                "outcome": "failed",
                "cause_kind": "not_a_machine_cause",
            },
        },
    }

    event = parse_event(raw)

    assert isinstance(event, RunFailed)
    assert event.error_report is not None
    assert event.error_report.reason is None


def test_parse_malformed_known_events_preserves_raw_payload():
    cases = [
        (
            {"type": "turn_completed", "usage": {"input_tokens": 50, "output_tokens": 20}},
            "missing stop_reason must not become end_turn",
        ),
        (
            {
                "type": "turn_completed",
                "stop_reason": "end_turn",
                "usage": {"input_tokens": "oops", "output_tokens": 20},
            },
            "malformed usage must not become token counts",
        ),
        (
            {
                "type": "tool_config_changed",
                "payload": {
                    "operation": "add",
                    "target": "filesystem",
                    "status": "staged",
                    "persisted": "false",
                },
            },
            "non-boolean persisted must not become false",
        ),
        (
            {
                "type": "tool_config_changed",
                "payload": {
                    "operation": "bogus",
                    "target": 0,
                    "status": 0,
                    "persisted": "false",
                },
            },
            "malformed operation/status semantics must not become an empty typed payload",
        ),
    ]

    for raw, reason in cases:
        event = parse_event(raw)
        assert isinstance(event, UnknownEvent), reason
        assert event.type == "malformed_event"
        assert event.data == raw


def test_parse_run_completed():
    raw = {
        "type": "run_completed",
        "session_id": "abc-123",
        "result": "Done!",
        "structured_output": {"answer": 42},
        "extraction_required": True,
        "usage": {"input_tokens": 100, "output_tokens": 50},
    }
    event = parse_event(raw)
    assert isinstance(event, RunCompleted)
    assert event.session_id == "abc-123"
    assert event.result == "Done!"
    assert event.structured_output == {"answer": 42}
    assert event.extraction_required is True
    assert event.usage.input_tokens == 100


def test_parse_extraction_terminal_events():
    succeeded = parse_event(
        {
            "type": "extraction_succeeded",
            "session_id": "abc-123",
            "structured_output": {"answer": 42},
            "schema_warnings": [{"provider": "openai", "path": "$", "message": "warn"}],
        }
    )
    assert isinstance(succeeded, ExtractionSucceeded)
    assert succeeded.session_id == "abc-123"
    assert succeeded.structured_output == {"answer": 42}

    failed = parse_event(
        {
            "type": "extraction_failed",
            "session_id": "abc-123",
            "last_output": "main answer",
            "attempts": 2,
            "reason": "Invalid JSON",
        }
    )
    assert isinstance(failed, ExtractionFailed)
    assert failed.last_output == "main answer"
    assert failed.attempts == 2
    assert failed.reason == "Invalid JSON"


def test_parse_run_failed_preserves_hook_denied_error_report():
    raw = {
        "type": "run_failed",
        "session_id": "session-1",
        "error_class": "hook",
        "error": "denied",
        "error_report": {
            "class": "hook",
            "message": "denied",
            "reason": {
                "reason_type": "hook_denied",
                "hook_id": "policy-gate",
                "point": "pre_tool_execution",
                "reason_code": "policy",
            },
        },
    }

    event = parse_event(raw)

    assert isinstance(event, RunFailed)
    assert event.error_report is not None
    assert event.error_report["class"] == "hook"
    assert event.error_report["reason"]["reason_type"] == "hook_denied"
    assert event.error_report["reason"]["hook_id"] == "policy-gate"


def test_parse_run_failed_does_not_promote_string_only_hook_id_mirrors():
    raw = {
        "type": "run_failed",
        "session_id": "session-1",
        "error_class": "hook",
        "error": "denied",
        "error_report": {
            "class": "hook",
            "message": "denied",
            "reason": {
                "reason_type": "hook_denied",
                "hook_id_string": "legacy-policy-gate",
                "point": "pre_tool_execution",
                "reason_code": "policy",
            },
        },
    }

    event = parse_event(raw)

    assert isinstance(event, RunFailed)
    assert event.error_report is not None
    reason = event.error_report["reason"]
    assert "hook_id" not in reason
    assert "hook_id_string" not in reason

    malformed = parse_event({
        "type": "run_failed",
        "session_id": "session-1",
        "error_class": "hook",
        "error": "denied",
        "error_report": {
            "class": "hook",
            "message": "denied",
            "reason": {
                "reason_type": "hook_denied",
                "hook_id": {"value": "policy-gate"},
                "point": "pre_tool_execution",
                "reason_code": "policy",
            },
        },
    })
    assert isinstance(malformed, UnknownEvent)
    assert malformed.type == "malformed_event"

    timeout_string_mirror = parse_event({
        "type": "run_failed",
        "session_id": "session-1",
        "error_class": "hook",
        "error": "timeout",
        "error_report": {
            "class": "hook",
            "message": "timeout",
            "reason": {
                "reason_type": "hook_timeout",
                "hook_id_string": "legacy-policy-gate",
                "timeout_ms": 100,
            },
        },
    })
    assert isinstance(timeout_string_mirror, UnknownEvent)
    assert timeout_string_mirror.type == "malformed_event"


def test_parse_tool_execution_completed():
    raw = {
        "type": "tool_execution_completed",
        "id": "t1",
        "name": "search",
        "result": "found it",
        "content": [
            {"type": "text", "text": "found it"},
            {"type": "image", "media_type": "image/png", "source": "inline", "data": "AAAA"},
        ],
        "is_error": False,
        "duration_ms": 42,
    }
    event = parse_event(raw)
    assert isinstance(event, ToolExecutionCompleted)
    assert event.name == "search"
    assert event.duration_ms == 42
    assert event.is_error is False
    assert event.content == [
        {"type": "text", "text": "found it"},
        {"type": "image", "media_type": "image/png", "source": "inline", "data": "AAAA"},
    ]


def test_parse_tool_result_received_content_blocks():
    raw = {
        "type": "tool_result_received",
        "id": "t1",
        "name": "search",
        "content": [{"type": "text", "text": "found it"}],
        "is_error": False,
    }
    event = parse_event(raw)
    assert isinstance(event, ToolResultReceived)
    assert event.content == [{"type": "text", "text": "found it"}]
    assert event.is_error is False


def test_parse_tool_execution_completed_preserves_malformed_content_blocks():
    raw = {
        "type": "tool_execution_completed",
        "id": "t1",
        "name": "search",
        "result": "found it",
        "content": "not blocks",
        "is_error": False,
        "duration_ms": 42,
    }
    event = parse_event(raw)
    assert isinstance(event, UnknownEvent)
    assert event.type == "malformed_event"
    assert event.data == raw


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
    assert missing.duration_ms is None
    assert malformed.duration_ms is None
    assert missing.content == [{"type": "text", "text": "found it"}]
    assert malformed.content == [{"type": "text", "text": "found it"}]


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


def test_parse_skills_resolved_with_typed_skill_identities():
    source_uuid = "00000000-0000-4b11-8111-000000000001"
    raw = {
        "type": "skills_resolved",
        "skills": [{"source_uuid": source_uuid, "skill_name": "email-extractor"}],
        "injection_bytes": 128,
    }
    event = parse_event(raw)
    assert isinstance(event, SkillsResolved)
    assert event.skills == [
        SkillKey(source_uuid=source_uuid, skill_name="email-extractor"),
    ]
    assert event.injection_bytes == 128


def test_parse_skills_resolved_rejects_legacy_string_only_payload():
    raw = {
        "type": "skills_resolved",
        "skills": ["legacy/ref"],
        "injection_bytes": 128,
    }
    event = parse_event(raw)
    assert isinstance(event, UnknownEvent)
    assert event.type == "malformed_event"


def test_parse_skills_resolved_rejects_missing_typed_identity_fields():
    raw = {
        "type": "skills_resolved",
        "skills": [{"source_uuid": "00000000-0000-4b11-8111-000000000001"}],
        "injection_bytes": 128,
    }
    event = parse_event(raw)
    assert isinstance(event, UnknownEvent)
    assert event.type == "malformed_event"


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


def test_parse_skill_resolution_failed_preserves_unknown_status_as_typed_unknown():
    raw = {
        "type": "skill_resolution_failed",
        "reason": {"reason_type": "future_status", "message": "future details"},
        "reference": "legacy/ref",
        "error": "missing",
    }
    event = parse_event(raw)
    assert isinstance(event, SkillResolutionFailed)
    assert event.skill_key is None
    assert isinstance(event.reason, SkillResolutionFailureReason)
    assert event.reason.reason_type == "unknown"
    assert event.reason.message == "future details"
    assert event.reason.raw_reason_type == "future_status"


def test_parse_skill_resolution_failed_does_not_fabricate_empty_keyed_reason():
    raw = {
        "type": "skill_resolution_failed",
        "reason": {"reason_type": "not_found"},
        "reference": "legacy/ref",
        "error": "missing",
    }
    event = parse_event(raw)
    assert isinstance(event, SkillResolutionFailed)
    assert event.skill_key is None
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

    blocks = [
        {"type": "text", "text": "hello"},
        {"type": "image", "media_type": "image/png", "source": "inline", "data": "AAAA"},
    ]
    send_receipt = await client.send(
        "s1",
        kind="peer_message",
        to="agent-a",
        body="hello",
        blocks=blocks,
    )
    peers = await client.peers("s1")

    assert send_receipt["kind"] == "peer_message_sent"
    assert peers["peers"] == [{"name": "agent-a"}]
    assert [m for m, _ in calls] == ["comms/send", "comms/peers"]
    assert calls[0][1]["blocks"] == blocks


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
            "resolved_capabilities": {
                "vision": True,
                "image_input": True,
                "image_tool_results": True,
                "inline_video": False,
                "realtime": False,
                "web_search": True,
                "image_generation": True,
            },
        }

    client._request = fake_request  # type: ignore[method-assign]

    details = await client.read_session("s1")
    assert isinstance(details, SessionDetails)
    assert details.session_id == "s1"
    assert details.model == "claude-sonnet-4-6"
    assert details.provider == "anthropic"
    assert details.last_assistant_text == "hello"
    assert details.resolved_capabilities == {
        "vision": True,
        "image_input": True,
        "image_tool_results": True,
        "inline_video": False,
        "realtime": False,
        "web_search": True,
        "image_generation": True,
    }


@pytest.mark.asyncio
async def test_client_models_catalog_and_schedule_wrappers_use_expected_rpc_methods():
    client = MeerkatClient()
    calls = []

    async def fake_request(method, params):
        calls.append((method, params))
        if method == "help/ask":
            return {
                "session_id": "help-1",
                "text": "rkat mcp add ...",
                "turns": 1,
                "tool_calls": 0,
                "usage": {"input_tokens": 10, "output_tokens": 20},
            }
        if method == "models/catalog":
            return {
                "contract_version": {"major": 0, "minor": 5, "patch": 1},
                "providers": [
                    {
                        "provider": "anthropic",
                        "default_model_id": "claude-sonnet-4-6",
                        "models": [
                            {
                                "id": "claude-sonnet-4-6",
                                "profile": {
                                    "model_family": "claude",
                                    "vision": True,
                                    "image_input": True,
                                    "image_tool_results": True,
                                    "inline_video": False,
                                    "realtime": False,
                                    "supports_web_search": True,
                                    "image_generation": True,
                                    "supports_temperature": True,
                                    "supports_thinking": True,
                                    "supports_reasoning": True,
                                    "params_schema": {},
                                },
                            }
                        ],
                    }
                ],
            }
        if method == "schedule/list":
            return {"schedules": []}
        if method == "schedule/occurrences":
            return {"occurrences": []}
        if method == "schedule/tools":
            return {"tools": [{"name": "meerkat_schedule_list"}]}
        return {"ok": True}

    client._request = fake_request  # type: ignore[method-assign]

    help_result = await client.ask_help(
        "How do I add an MCP server?",
        prompt="Write a game",
        execution_mode="plan_execution",
        model="claude-sonnet-4-6",
        provider="anthropic",
        max_tokens=512,
    )
    assert help_result.text == "rkat mcp add ..."

    models = await client.get_models_catalog()
    assert models["contract_version"] == {"major": 0, "minor": 5, "patch": 1}
    profile = models["providers"][0]["models"][0]["profile"]
    assert profile["supports_web_search"] is True
    assert "web_search" not in profile
    assert profile["image_generation"] is True

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
        "help/ask",
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
    assert calls[0][1] == {
        "question": "How do I add an MCP server?",
        "execution_mode": "plan_execution",
        "prompt": "Write a game",
        "model": "claude-sonnet-4-6",
        "provider": "anthropic",
        "max_tokens": 512,
    }
    assert calls[4][1] == {"labels": {"env": "test"}, "limit": 5, "offset": 2}


def test_create_session_params_forward_workgraph_override():
    params = MeerkatClient._build_create_params(
        "Hello",
        enable_schedule=True,
        enable_workgraph=True,
        enable_web_search=True,
    )

    assert params["enable_schedule"] is True
    assert params["enable_workgraph"] is True
    assert params["enable_web_search"] is True


@pytest.mark.asyncio
async def test_client_workgraph_wrappers_use_expected_rpc_methods():
    client = MeerkatClient()
    calls = []
    timestamp = "2026-05-12T12:00:00Z"
    item = {
        "id": "prep-dentist-ride",
        "realm_id": "homecore",
        "namespace": "family/appointments",
        "title": "Prep A for non-preferred dentist car",
        "status": "open",
        "priority": "high",
        "labels": ["autism-support", "dentist"],
        "revision": 1,
        "created_at": timestamp,
        "updated_at": timestamp,
        "external_refs": [{"kind": "calendar_event", "id": "dentist-visit"}],
        "evidence_refs": [{"kind": "message_draft", "id": "draft-1"}],
    }

    async def fake_request(method, params):
        calls.append((method, params))
        if method == "workgraph/get":
            return item
        if method in {"workgraph/list", "workgraph/ready"}:
            return {"items": [item]}
        if method == "workgraph/snapshot":
            return {
                "realm_id": "homecore",
                "namespace": "family/appointments",
                "all_namespaces": False,
                "captured_at": timestamp,
                "event_high_water_mark": 7,
                "items": [item],
                "edges": [],
                "ready_item_ids": ["prep-dentist-ride"],
            }
        if method == "workgraph/events":
            return {
                "events": [
                    {
                        "seq": 7,
                        "realm_id": "homecore",
                        "namespace": "family/appointments",
                        "item_id": "prep-dentist-ride",
                        "kind": "created",
                        "at": timestamp,
                        "payload": {},
                    }
                ]
            }
        raise AssertionError(f"unexpected method {method}")

    client._request = fake_request  # type: ignore[method-assign]

    fetched = await client.get_workgraph_item(
        "prep-dentist-ride",
        realm_id="homecore",
        namespace="family/appointments",
    )
    listed = await client.list_workgraph_items({"realm_id": "homecore", "limit": 5})
    ready = await client.list_ready_workgraph_items(
        {"namespace": "family/appointments", "limit": 3}
    )
    snapshot = await client.get_workgraph_snapshot(
        {"realm_id": "homecore", "namespace": "family/appointments"}
    )
    events = await client.list_workgraph_events({"realm_id": "homecore", "limit": 10})

    assert fetched["id"] == "prep-dentist-ride"
    assert listed["items"][0]["priority"] == "high"
    assert ready["items"][0]["status"] == "open"
    assert snapshot["ready_item_ids"] == ["prep-dentist-ride"]
    assert events["events"][0]["kind"] == "created"
    assert [method for method, _ in calls] == [
        "workgraph/get",
        "workgraph/list",
        "workgraph/ready",
        "workgraph/snapshot",
        "workgraph/events",
    ]
    assert calls[0][1] == {
        "id": "prep-dentist-ride",
        "realm_id": "homecore",
        "namespace": "family/appointments",
    }
    assert calls[1][1] == {"realm_id": "homecore", "limit": 5}
    assert calls[2][1] == {"namespace": "family/appointments", "limit": 3}


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
            "server_name": params.get("server_name")
            or params.get("server_config", {}).get("name"),
            "applied_at_turn": None,
        }

    client._request = fake_request  # type: ignore[method-assign]

    add = await client.mcp_add("s1", {"name": "filesystem", "command": "npx"})
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
        await client.mcp_add("s1", {"name": "filesystem", "command": "npx"})
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
        await client.mcp_add("s1", {"name": "filesystem", "command": "npx"})


@pytest.mark.asyncio
async def test_mob_turn_start_wrapper_uses_typed_prompt_and_overrides():
    client = MeerkatClient()
    calls = []

    async def fake_request(method: str, params: dict[str, object]) -> dict[str, object]:
        calls.append((method, params))
        return {"status": "started"}

    client._request = fake_request  # type: ignore[method-assign]

    await client.mob_turn_start(
        "mob-1",
        "worker-1",
        [{"type": "text", "text": "continue"}],
        skill_refs=[
            SkillKey(
                source_uuid="00000000-0000-4000-8000-000000000001",
                skill_name="read",
            )
        ],
        flow_tool_overlay={"allowed_tools": ["read"], "blocked_tools": []},
        additional_instructions=["stay concise"],
        keep_alive=True,
        model="gpt-test",
        provider="openai",
        max_tokens=128,
        system_prompt="system",
        output_schema={"type": "object"},
        structured_output_retries=2,
        provider_params={"temperature": 0.2},
        clear_provider_params=True,
        auth_binding={"realm": "dev", "binding": "default_openai"},
        clear_auth_binding=True,
    )

    assert calls == [
        (
            "mob/turn_start",
            {
                "mob_id": "mob-1",
                "agent_identity": "worker-1",
                "prompt": [{"type": "text", "text": "continue"}],
                "skill_refs": [
                    {
                        "kind": "structured",
                        "source_uuid": "00000000-0000-4000-8000-000000000001",
                        "skill_name": "read",
                    }
                ],
                "flow_tool_overlay": {
                    "allowed_tools": ["read"],
                    "blocked_tools": [],
                },
                "additional_instructions": ["stay concise"],
                "keep_alive": True,
                "model": "gpt-test",
                "provider": "openai",
                "max_tokens": 128,
                "system_prompt": "system",
                "output_schema": {"type": "object"},
                "structured_output_retries": 2,
                "provider_params": {"temperature": 0.2},
                "clear_provider_params": True,
                "auth_binding": {"realm": "dev", "binding": "default_openai"},
                "clear_auth_binding": True,
            },
        )
    ]

    with pytest.raises(TypeError):
        await client.mob_turn_start(
            "mob-1",
            "worker-1",
            "continue",
            unexpected_override=True,
        )


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
                "resolved_capabilities": {
                    "vision": False,
                    "image_input": False,
                    "image_tool_results": False,
                    "inline_video": False,
                    "realtime": True,
                    "web_search": False,
                    "image_generation": False,
                },
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
        initial_message=[{"type": "text", "text": "hello"}],
        runtime_mode="autonomous_host",
        backend="session",
        labels={"role": "planner"},
        context={"ticket": "LUC-134"},
        additional_instructions=["stay focused"],
        binding={"kind": "session"},
        shell_env={"TEST_MODE": "1"},
        auto_wire_parent=True,
        launch_mode={"mode": "fresh"},
        tool_access_policy={"type": "allow_list", "value": ["grep"]},
        budget_split_policy={"type": "remaining"},
        inherited_tool_filter={"Allow": ["grep"]},
        override_profile={
            "model": "claude-sonnet-4-6",
            "tools": {"shell": True},
        },
        auth_binding={"realm": "dev", "binding": "default_anthropic"},
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
    assert status["resolved_capabilities"]["realtime"] is True

    client._request = fake_request  # type: ignore[method-assign]
    await client.respawn_mob_member("mob-1", "agent-a", "hello")
    await client.wire_mob_members("mob-1", "a", "b")
    await client.unwire_mob_members(
        "mob-1",
        "a",
        {
            "external": {
                "name": "remote",
                "address": "inproc://remote",
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
                },
            }
        },
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
    assert calls[8][1] == {"mob_id": "mob-1", "member": "a", "peer": {"local": "b"}}
    assert calls[9][1] == {
        "mob_id": "mob-1",
        "member": "a",
        "peer": {
            "external": {
                "name": "remote",
                "address": "inproc://remote",
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
                },
            }
        },
    }
    assert calls[11][1] == {
        "mob_id": "mob-1",
        "member_ids": ["agent-a"],
        "timeout_ms": 1234,
    }
    assert calls[12][1] == {"mob_id": "mob-1", "timeout_ms": 99}
    assert calls[4][1] == {
        "mob_id": "mob-1",
        "profile": "planner",
        "agent_identity": "agent-a",
        "initial_message": [{"type": "text", "text": "hello"}],
        "runtime_mode": "autonomous_host",
        "backend": "session",
        "labels": {"role": "planner"},
        "context": {"ticket": "LUC-134"},
        "additional_instructions": ["stay focused"],
        "binding": {"kind": "session"},
        "shell_env": {"TEST_MODE": "1"},
        "auto_wire_parent": True,
        "launch_mode": {"mode": "fresh"},
        "tool_access_policy": {"type": "allow_list", "value": ["grep"]},
        "budget_split_policy": {"type": "remaining"},
        "inherited_tool_filter": {"Allow": ["grep"]},
        "override_profile": {
            "model": "claude-sonnet-4-6",
            "tools": {"shell": True},
        },
        "auth_binding": {"realm": "dev", "binding": "default_anthropic"},
    }


@pytest.mark.asyncio
async def test_mob_status_rejects_missing_status():
    client = MeerkatClient()

    async def fake_request(_method, _params):
        return {"mob_id": "mob-1"}

    client._request = fake_request  # type: ignore[method-assign]

    with pytest.raises(MeerkatError, match="missing status"):
        await client.mob_status("mob-1")


@pytest.mark.asyncio
async def test_wait_mob_ready_rejects_malformed_member_snapshot():
    client = MeerkatClient()

    async def fake_request(_method, _params):
        return {
            "members": [
                {
                    "agent_identity": "lead",
                    "status": "active",
                    "tokens_used": 42,
                }
            ]
        }

    client._request = fake_request  # type: ignore[method-assign]

    with pytest.raises(MeerkatError, match="is_final must be boolean"):
        await client.wait_mob_ready("mob-1")


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


# ---------------------------------------------------------------------------
# CC5/CC6: typed live-adapter capability + continuity wire mirrors.
# ---------------------------------------------------------------------------


def test_generated_wire_live_channel_capabilities_exposes_typed_booleans():
    """CC5: SDK consumers must see typed access to every capability boolean.

    Closes the verifier finding that `LiveChannelCapabilities` was projected
    as `serde_json::Value` and SDKs got raw JSON. The regenerated dataclass
    must expose every field as a typed `bool` with snake_case names so
    static type checkers (pyright/mypy) catch typos at field access.
    """
    from dataclasses import fields

    from meerkat.types import WireLiveChannelCapabilities

    field_names = {f.name for f in fields(WireLiveChannelCapabilities)}
    assert field_names == {
        "audio_in",
        "audio_out",
        "text_in",
        "text_out",
        "image_in",
        "video_in",
        "transcript_supported",
        "barge_in_supported",
        "provider_native_resume",
    }
    for f in fields(WireLiveChannelCapabilities):
        assert f.type in {"bool", bool}, (
            f"WireLiveChannelCapabilities.{f.name} must be typed `bool`, "
            f"got {f.type!r}"
        )


def test_generated_wire_live_channel_capabilities_constructs_for_gpt_realtime_2():
    """T8 + CC5: typed booleans represent `gpt-realtime-2` (image_in) and
    Gemini Live (video_in) without provider-specific fields."""
    from meerkat.types import WireLiveChannelCapabilities

    gpt_realtime_2 = WireLiveChannelCapabilities(
        audio_in=True,
        audio_out=True,
        text_in=True,
        text_out=True,
        image_in=True,
        video_in=False,
        transcript_supported=True,
        barge_in_supported=True,
        provider_native_resume=False,
    )
    gemini_live = WireLiveChannelCapabilities(
        audio_in=True,
        audio_out=True,
        text_in=True,
        text_out=True,
        image_in=False,
        video_in=True,
        transcript_supported=True,
        barge_in_supported=True,
        provider_native_resume=False,
    )
    # Typed access at the Python attribute level — closes the SDK boundary
    # for CC5.
    assert gpt_realtime_2.image_in is True
    assert gpt_realtime_2.video_in is False
    assert gemini_live.image_in is False
    assert gemini_live.video_in is True


def test_generated_wire_live_continuity_mode_is_tagged_union():
    """CC6: SDK consumers must see a tagged-union shape on `mode`.

    Each variant is a TypedDict with `mode: Literal[...]` so mypy/pyright
    can narrow on the discriminator. The `provider_native_resume` variant
    additionally carries `provider_session_id: str`.
    """
    from typing import Literal, get_args, get_type_hints

    from meerkat.types import (
        WireLiveContinuityModeDegraded,
        WireLiveContinuityModeFresh,
        WireLiveContinuityModeProviderNativeResume,
        WireLiveContinuityModeTranscriptOnly,
    )

    fresh_hints = get_type_hints(WireLiveContinuityModeFresh)
    assert get_args(fresh_hints["mode"]) == ("fresh",)

    transcript_hints = get_type_hints(WireLiveContinuityModeTranscriptOnly)
    assert get_args(transcript_hints["mode"]) == ("transcript_only",)

    degraded_hints = get_type_hints(WireLiveContinuityModeDegraded)
    assert get_args(degraded_hints["mode"]) == ("degraded",)

    resume_hints = get_type_hints(WireLiveContinuityModeProviderNativeResume)
    assert get_args(resume_hints["mode"]) == ("provider_native_resume",)
    # Payload field is typed `str`, not `Any` — closes the CC6 finding.
    assert resume_hints["provider_session_id"] is str


def test_generated_live_open_result_references_typed_capabilities_and_continuity():
    """CC5/CC6: `LiveOpenResult.capabilities` / `.continuity` are typed at
    the dataclass level — not erased to `Any` / `dict[str, Any]`.

    This is the verifier-facing acceptance test: SDK clients see typed
    access to image_in / video_in (via `WireLiveChannelCapabilities`) and
    can route on `result.continuity["mode"]` without parsing raw JSON.
    """
    from typing import get_type_hints

    from meerkat.types import (
        LiveOpenResult,
        WireLiveChannelCapabilities,
        WireLiveContinuityMode,
    )

    hints = get_type_hints(LiveOpenResult)
    assert hints["capabilities"] is WireLiveChannelCapabilities
    # `WireLiveContinuityMode` is a `Union[...]` alias; the dataclass field
    # type must be exactly this alias, not `Any`.
    assert hints["continuity"] == WireLiveContinuityMode


def test_generated_live_send_input_params_chunk_is_typed_union():
    """R5-10: `LiveSendInputParams.chunk` is the typed `LiveInputChunkWire`
    union, not opaque `dict[str, Any]`.

    Before R5-10, schema codegen erased `LiveInputChunkWire` at the
    `LiveSendInputParams.chunk` boundary and emitted `chunk: dict[str, Any]`.
    The fix promotes `LiveInputChunkWire` into the codegen lookup root so
    the `LiveSendInputParams` dataclass references the typed union by name.
    """
    from dataclasses import fields
    from typing import get_type_hints

    from meerkat.types import (
        LiveInputChunkWire,
        LiveInputChunkWireAudio,
        LiveInputChunkWireImage,
        LiveInputChunkWireText,
        LiveInputChunkWireVideoFrame,
        LiveSendInputParams,
    )

    hints = get_type_hints(LiveSendInputParams)
    # The `chunk` field is exactly the `LiveInputChunkWire` union — not
    # `dict[str, Any]` and not `Any`.
    assert hints["chunk"] == LiveInputChunkWire

    field_names = {f.name for f in fields(LiveSendInputParams)}
    assert field_names == {"channel_id", "chunk"}

    # All four typed variants must be constructible and assignable to the
    # `chunk` slot — proves the union is the discriminated `kind`-tagged
    # shape, not a structurally-empty placeholder.
    audio = LiveInputChunkWireAudio(
        kind="audio", data="AQID", sample_rate_hz=24_000, channels=1
    )
    text = LiveInputChunkWireText(kind="text", text="hello")
    image = LiveInputChunkWireImage(
        kind="image", mime="image/png", data="iVBORw0KGgo="
    )
    video_frame = LiveInputChunkWireVideoFrame(
        kind="video_frame", codec="vp8", data="AQID", timestamp_ms=1_234
    )

    for chunk in (audio, text, image, video_frame):
        params = LiveSendInputParams(channel_id="live_1", chunk=chunk)
        assert params.channel_id == "live_1"
        assert params.chunk is chunk


def test_generated_wire_live_adapter_observation_is_typed_union():
    """FIX-SDK-OBS: `WireLiveAdapterObservation` is a discriminated
    union over the `observation` tag — not opaque `dict[str, Any]`.

    Closes the verifier gap that left R5-4 identity fields (`item_id`,
    `response_id`, `content_index` on `assistant_audio_chunk`) and the
    R5-9 `command_rejected` typed channel-survives error invisible at
    the SDK boundary. Browser/Python clients can now type-narrow on the
    `observation` discriminator and read the typed payload fields with
    static-checker support.
    """
    from typing import get_args, get_type_hints

    from meerkat.types import (
        WireLiveAdapterErrorCode,
        WireLiveAdapterObservation,
        WireLiveAdapterObservationAssistantAudioChunk,
        WireLiveAdapterObservationCommandRejected,
        WireLiveAdapterObservationError,
        WireLiveAdapterObservationReady,
        WireLiveAdapterObservationStatusChanged,
        WireLiveAdapterObservationTurnInterrupted,
    )

    # The union must include every variant the adapter can emit.
    union_members = set(get_args(WireLiveAdapterObservation))
    assert WireLiveAdapterObservationReady in union_members
    assert WireLiveAdapterObservationAssistantAudioChunk in union_members
    assert WireLiveAdapterObservationCommandRejected in union_members
    assert WireLiveAdapterObservationError in union_members
    assert WireLiveAdapterObservationStatusChanged in union_members
    assert WireLiveAdapterObservationTurnInterrupted in union_members
    # Sanity: each variant is tagged on `observation` with the snake_case
    # discriminator value.
    audio_hints = get_type_hints(WireLiveAdapterObservationAssistantAudioChunk)
    assert get_args(audio_hints["observation"]) == ("assistant_audio_chunk",)
    rejected_hints = get_type_hints(WireLiveAdapterObservationCommandRejected)
    assert get_args(rejected_hints["observation"]) == ("command_rejected",)
    # R5-4: identity fields are visible (typed Optional[str] / int) at
    # the SDK boundary so browser clients can drive `live/truncate`.
    assert "item_id" in audio_hints
    assert "response_id" in audio_hints
    assert "content_index" in audio_hints
    # R5-9: typed `code` field on the rejection variant — not a free-form
    # blob.
    assert rejected_hints["code"] == WireLiveAdapterErrorCode


def test_generated_wire_assistant_block_variant_data_is_typed_typeddict():
    """R7-1 Python lift: each inline-object variant payload on
    ``WireAssistantBlock`` must surface as a named ``TypedDict`` — not as
    opaque ``dict[str, Any]`` — closing the TS/Python codegen asymmetry
    that R7-1+2 opened (TS already narrows the `data` shape to a typed
    structural object; Python now mirrors with a named TypedDict per
    variant payload).

    Pins both:

    * the *shape*: ``data`` on each variant references a per-variant
      ``WireAssistantBlock*Data`` TypedDict, with the same field set the
      TS path emits;
    * the *required/optional* discipline: e.g. ``WireTranscriptSource``
      is ``Required`` on ``WireAssistantBlockTranscriptData`` and
      ``meta`` is ``NotRequired`` (matches the JSON-schema `required`
      list and prevents regression to the pre-R7-1 opaque-dict shape).
    """
    from typing import get_type_hints

    from meerkat.generated.types import (
        WireAssistantBlockImage,
        WireAssistantBlockImageData,
        WireAssistantBlockReasoning,
        WireAssistantBlockReasoningData,
        WireAssistantBlockServerToolContent,
        WireAssistantBlockServerToolContentData,
        WireAssistantBlockText,
        WireAssistantBlockTextData,
        WireAssistantBlockToolUse,
        WireAssistantBlockToolUseData,
        WireAssistantBlockTranscript,
        WireAssistantBlockTranscriptData,
        WireTranscriptSource,
        WireTranscriptSourceSpoken,
        WireTranscriptSourceUnknown,
    )

    # Every typed-payload variant binds `data` to its named TypedDict —
    # NOT `dict[str, Any]`. The string check pins the typed reference so
    # a regression to the opaque shape fails this test loudly.
    variants_with_data = [
        (WireAssistantBlockText, WireAssistantBlockTextData, "WireAssistantBlockTextData"),
        (WireAssistantBlockTranscript, WireAssistantBlockTranscriptData, "WireAssistantBlockTranscriptData"),
        (WireAssistantBlockReasoning, WireAssistantBlockReasoningData, "WireAssistantBlockReasoningData"),
        (WireAssistantBlockToolUse, WireAssistantBlockToolUseData, "WireAssistantBlockToolUseData"),
        (WireAssistantBlockServerToolContent, WireAssistantBlockServerToolContentData, "WireAssistantBlockServerToolContentData"),
        (WireAssistantBlockImage, WireAssistantBlockImageData, "WireAssistantBlockImageData"),
    ]
    for variant_td, data_td, data_name in variants_with_data:
        hints = get_type_hints(variant_td, include_extras=True)
        assert "data" in hints, f"{variant_td.__name__} missing `data` field"
        rendered = str(hints["data"])
        assert data_name in rendered, (
            f"{variant_td.__name__}.data should reference {data_name}, got {rendered!r}"
        )
        assert "dict[str, Any]" not in rendered, (
            f"{variant_td.__name__}.data regressed to opaque dict[str, Any]"
        )

    # Field-level shape pinning on the Transcript payload (R7-1 closes
    # WireAssistantBlock::Transcript first; the rest follow the same
    # mechanical pattern).
    transcript_data_hints = get_type_hints(
        WireAssistantBlockTranscriptData, include_extras=True
    )
    assert "text" in transcript_data_hints
    assert "source" in transcript_data_hints
    assert "meta" in transcript_data_hints
    # `source` is typed against the discriminated `WireTranscriptSource`
    # union (a PEP-604 `Spoken | Unknown` alias). It must NOT be a
    # free-form dict — callers must be able to type-narrow on
    # `source["kind"]`. The annotation comes through as the unwrapped
    # union (TypedDict alias inlining), so we check by string contents.
    rendered_source = str(transcript_data_hints["source"])
    assert "WireTranscriptSourceSpoken" in rendered_source
    assert "WireTranscriptSourceUnknown" in rendered_source
    assert "dict[str, Any]" not in rendered_source
    # Reference `WireTranscriptSource` so the import isn't unused — the
    # union alias is the public name the SDK exports, even though the
    # resolved TypedDict annotation inlines its members.
    assert set(get_args(WireTranscriptSource)) == {
        WireTranscriptSourceSpoken,
        WireTranscriptSourceUnknown,
    }
    # Required/Optional discipline matches the JSON-schema `required`
    # list: `text` + `source` Required, `meta` NotRequired. Inspect raw
    # annotations because the generated TypedDict is declared with
    # `total=False` + explicit `Required[...]`/`NotRequired[...]`
    # wrappers — Python's runtime `__required_keys__` flips everything
    # to optional under `total=False`, so the source of truth for
    # required-ness is the annotation string itself.
    raw_annotations = WireAssistantBlockTranscriptData.__annotations__
    assert "Required" in str(raw_annotations["text"])
    assert "Required" in str(raw_annotations["source"])
    assert "NotRequired" in str(raw_annotations["meta"])
    # Constructing a typed value through the named TypedDict succeeds —
    # this is the ergonomic surface the R7-1 deferral note flagged.
    sample: WireAssistantBlockTranscriptData = {
        "text": "hello world",
        "source": {"kind": "spoken"},
    }
    assert sample["text"] == "hello world"


@pytest.mark.asyncio
async def test_client_live_refresh_returns_typed_result():
    """R4-5 (P3): `live_refresh` must return a typed
    :class:`LiveRefreshResult` carrying both the typed ``status``
    discriminator and the legacy ``refresh_enqueued`` boolean.

    The wire payload mirrors the server-side `handle_live_refresh` Ok arm:
    ``{"status": "queued", "refresh_enqueued": true}`` (from
    `LiveRefreshResult::queued()`). Both fields must round-trip into the
    typed dataclass at the SDK boundary so callers can route on
    ``status`` (forward-compatible enum) without dropping back-compat with
    R7-era clients that pattern-match on ``refresh_enqueued``.
    """
    from meerkat.generated.types import LiveRefreshResult

    client = MeerkatClient()

    captured: list[tuple[str, dict[str, object]]] = []

    async def fake_request(method, params):
        captured.append((method, params))
        # Mirror exactly what `handle_live_refresh` ships on the wire after
        # `host.send_command` accepts the queued command.
        return {"status": "queued", "refresh_enqueued": True}

    client._request = fake_request  # type: ignore[method-assign]

    result = await client.live_refresh("live_channel_42")

    assert captured == [("live/refresh", {"channel_id": "live_channel_42"})]
    assert isinstance(result, LiveRefreshResult)
    # New typed routing surface.
    assert result.status == "queued"
    # Back-compat field for R7-era clients.
    assert result.refresh_enqueued is True


@pytest.mark.asyncio
async def test_client_live_refresh_preserves_unknown_status():
    """R4-5 (P3): the wire enum is open (`#[non_exhaustive]`) so a future
    server may return a previously-unknown ``status`` value (e.g.
    ``applied_sync``). The Python SDK must surface the raw string rather
    than coercing it back to ``"queued"`` — coercion would silently lie
    about the refresh outcome to forward-compatible clients that route on
    the discriminator.
    """
    from meerkat.generated.types import LiveRefreshResult

    client = MeerkatClient()

    async def fake_request(method, params):
        # Simulate a future-server reply that this SDK build does not yet
        # know about. The typed-status field is `Literal['queued']` today,
        # but the runtime payload must round-trip the unknown variant
        # untouched.
        return {"status": "applied_sync", "refresh_enqueued": True}

    client._request = fake_request  # type: ignore[method-assign]

    result = await client.live_refresh("live_channel_43")

    assert isinstance(result, LiveRefreshResult)
    assert result.status == "applied_sync"  # type: ignore[comparison-overlap]
    assert result.refresh_enqueued is True


@pytest.mark.asyncio
async def test_client_live_open_omits_turning_mode_when_default():
    """R4-2 (P2): when the caller does not pass ``turning_mode``, the
    Python SDK must omit the field on the wire — the server's
    ``LiveOpenParams.turning_mode`` is ``Option<RealtimeTurningMode>``
    with ``#[serde(skip_serializing_if = "Option::is_none")]`` and treats
    omitted as ``ProviderManaged`` for back-compat with R3-1-era clients.
    """
    client = MeerkatClient()

    captured: list[tuple[str, dict[str, object]]] = []

    async def fake_request(method, params):
        captured.append((method, params))
        return {
            "channel_id": "live_42",
            "transport": {"transport": "websocket", "url": "ws://x", "token": "t"},
            "capabilities": {
                "audio_in": True,
                "audio_out": True,
                "text_in": True,
                "text_out": True,
                "image_in": False,
                "video_in": False,
                "transcript_supported": True,
                "barge_in_supported": True,
                "provider_native_resume": False,
            },
            "continuity": {"mode": "fresh"},
        }

    client._request = fake_request  # type: ignore[method-assign]

    await client.live_open("session-42")

    assert captured == [("live/open", {"session_id": "session-42"})]
    assert "turning_mode" not in captured[0][1]


@pytest.mark.asyncio
async def test_client_live_open_passes_explicit_commit_turning_mode():
    """R4-2 (P2): explicit-commit on the wire so the G9 typed text-only
    ``live_commit_input(response_modality="text")`` path is reachable.
    Without R4-2, the Python SDK had no path to open a channel in
    ``ExplicitCommit`` mode — callers saw the modality knob on
    ``live_commit_input`` but couldn't actually open in the matching
    turning mode.
    """
    client = MeerkatClient()

    captured: list[tuple[str, dict[str, object]]] = []

    async def fake_request(method, params):
        captured.append((method, params))
        return {
            "channel_id": "live_43",
            "transport": {"transport": "websocket", "url": "ws://x", "token": "t"},
            "capabilities": {
                "audio_in": True,
                "audio_out": True,
                "text_in": True,
                "text_out": True,
                "image_in": False,
                "video_in": False,
                "transcript_supported": True,
                "barge_in_supported": True,
                "provider_native_resume": False,
            },
            "continuity": {"mode": "fresh"},
        }

    client._request = fake_request  # type: ignore[method-assign]

    await client.live_open("session-43", turning_mode="explicit_commit")

    assert captured == [
        (
            "live/open",
            {"session_id": "session-43", "turning_mode": "explicit_commit"},
        )
    ]


@pytest.mark.asyncio
async def test_client_live_open_passes_provider_managed_turning_mode():
    """R4-2 (P2): explicit ``provider_managed`` matches the server-side
    default but lets callers pin the wire shape rather than relying on
    the omitted-field default. Useful for callers that want their request
    log to carry the explicit decision.
    """
    client = MeerkatClient()

    captured: list[tuple[str, dict[str, object]]] = []

    async def fake_request(method, params):
        captured.append((method, params))
        return {
            "channel_id": "live_44",
            "transport": {"transport": "websocket", "url": "ws://x", "token": "t"},
            "capabilities": {
                "audio_in": True,
                "audio_out": True,
                "text_in": True,
                "text_out": True,
                "image_in": False,
                "video_in": False,
                "transcript_supported": True,
                "barge_in_supported": True,
                "provider_native_resume": False,
            },
            "continuity": {"mode": "fresh"},
        }

    client._request = fake_request  # type: ignore[method-assign]

    await client.live_open("session-44", turning_mode="provider_managed")

    assert captured == [
        (
            "live/open",
            {"session_id": "session-44", "turning_mode": "provider_managed"},
        )
    ]
