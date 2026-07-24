from __future__ import annotations

import ast
import base64
import inspect
import json
from pathlib import Path
import textwrap
from unittest.mock import AsyncMock, MagicMock

import pytest

from meerkat.client import MeerkatClient
from meerkat.errors import MeerkatError
from meerkat.mob import Mob
from meerkat.session import DeferredSession, Session


def _make_member_ref(mob_id: str, agent_identity: str) -> str:
    """Build an opaque wire `member_ref` token using the same shape the
    server emits (``base64url({"m": mob_id, "a": agent_identity})``).
    """
    payload = json.dumps({"m": mob_id, "a": agent_identity}, separators=(",", ":"))
    return (
        base64.urlsafe_b64encode(payload.encode("utf-8")).rstrip(b"=").decode("ascii")
    )


RPC_PUBLIC_WRAPPERS: dict[str, tuple[type, str]] = {
    "initialize": (MeerkatClient, "connect"),
    "capabilities/get": (MeerkatClient, "connect"),
    "realm/list": (MeerkatClient, "list_realms"),
    "realm/get": (MeerkatClient, "get_realm"),
    "auth/profile/list": (MeerkatClient, "list_auth_profiles"),
    "auth/profile/get": (MeerkatClient, "get_auth_profile"),
    "auth/profile/create": (MeerkatClient, "create_auth_profile"),
    "auth/profile/delete": (MeerkatClient, "delete_auth_profile"),
    "auth/login/start": (MeerkatClient, "auth_login_start"),
    "auth/login/complete": (MeerkatClient, "auth_login_complete"),
    "auth/login/device_start": (MeerkatClient, "auth_login_device_start"),
    "auth/login/device_complete": (MeerkatClient, "auth_login_device_complete"),
    "auth/login/provision_api_key": (MeerkatClient, "auth_provision_api_key"),
    "auth/status/get": (MeerkatClient, "auth_status"),
    "auth/logout": (MeerkatClient, "auth_logout"),
    "session/create": (MeerkatClient, "create_session"),
    "session/list": (MeerkatClient, "list_sessions"),
    "session/read": (MeerkatClient, "read_session"),
    "session/history": (MeerkatClient, "read_session_history"),
    "session/fork_at": (MeerkatClient, "fork_session_at"),
    "session/fork_replace": (MeerkatClient, "fork_session_replace"),
    "session/rewrite_transcript": (MeerkatClient, "rewrite_session_transcript"),
    "session/transcript_revision": (MeerkatClient, "read_session_transcript_revision"),
    "session/transcript_revisions": (
        MeerkatClient,
        "list_session_transcript_revisions",
    ),
    "session/restore_transcript_revision": (
        MeerkatClient,
        "restore_session_transcript_revision",
    ),
    "session/archive": (Session, "archive"),
    "session/stream_open": (MeerkatClient, "subscribe_session_events"),
    "session/stream_close": (MeerkatClient, "subscribe_session_events"),
    "session/external_event": (MeerkatClient, "send_external_event"),
    "session/peer_response_terminal": (MeerkatClient, "send_peer_response_terminal"),
    "session/inject_context": (MeerkatClient, "inject_context"),
    "session/input_status": (MeerkatClient, "input_state"),
    "turn/start": (Session, "turn"),
    "turn/interrupt": (Session, "interrupt"),
    "help/ask": (MeerkatClient, "ask_help"),
    "blob/get": (MeerkatClient, "get_blob"),
    "runtime/host_info": (MeerkatClient, "get_runtime_host_info"),
    "runtime/capabilities": (MeerkatClient, "get_runtime_host_capabilities"),
    "runtime/health": (MeerkatClient, "get_runtime_host_health"),
    "jobs/get": (MeerkatClient, "jobs_get"),
    "jobs/list": (MeerkatClient, "jobs_list"),
    "jobs/cancel": (MeerkatClient, "jobs_cancel"),
    "jobs/progress": (MeerkatClient, "jobs_progress"),
    "jobs/result": (MeerkatClient, "jobs_result"),
    "jobs/artifacts": (MeerkatClient, "jobs_artifacts"),
    "jobs/retry": (MeerkatClient, "jobs_retry"),
    "jobs/health": (MeerkatClient, "jobs_health"),
    "jobs/subscribe": (MeerkatClient, "jobs_subscribe"),
    "jobs/unsubscribe": (MeerkatClient, "jobs_unsubscribe"),
    "monitors/start": (MeerkatClient, "monitors_start"),
    "mobkit/jobs/heartbeat": (MeerkatClient, "mobkit_job_heartbeat"),
    "mobkit/jobs/progress": (MeerkatClient, "mobkit_job_progress"),
    "mobkit/jobs/checkpoint": (MeerkatClient, "mobkit_job_checkpoint"),
    "mobkit/jobs/complete": (MeerkatClient, "mobkit_job_complete"),
    "mobkit/jobs/fail": (MeerkatClient, "mobkit_job_fail"),
    "mobkit/jobs/cancel_ack": (MeerkatClient, "mobkit_job_cancel_ack"),
    "approval/request": (MeerkatClient, "request_approval"),
    "approval/list": (MeerkatClient, "list_approvals"),
    "approval/get": (MeerkatClient, "get_approval"),
    "approval/decide": (MeerkatClient, "decide_approval"),
    "artifact/list": (MeerkatClient, "list_artifacts"),
    "artifact/get": (MeerkatClient, "get_artifact"),
    "artifact/download": (MeerkatClient, "download_artifact"),
    "events/latest_cursor": (MeerkatClient, "latest_event_cursor"),
    "events/list_since": (MeerkatClient, "list_events_since"),
    "events/snapshot": (MeerkatClient, "event_snapshot"),
    "config/get": (MeerkatClient, "get_config"),
    "config/set": (MeerkatClient, "set_config"),
    "config/patch": (MeerkatClient, "patch_config"),
    "skills/list": (MeerkatClient, "list_skills"),
    "mcp/add": (MeerkatClient, "mcp_add"),
    "mcp/remove": (MeerkatClient, "mcp_remove"),
    "mcp/reload": (MeerkatClient, "mcp_reload"),
    "comms/send": (Session, "send"),
    "comms/peers": (Session, "peers"),
    "live/open": (MeerkatClient, "live_open"),
    "live/status": (MeerkatClient, "live_status"),
    "live/close": (MeerkatClient, "live_close"),
    "live/send_input": (MeerkatClient, "live_send_input_text"),
    "live/commit_input": (MeerkatClient, "live_commit_input"),
    "live/interrupt": (MeerkatClient, "live_interrupt"),
    "live/truncate": (MeerkatClient, "live_truncate"),
    "live/refresh": (MeerkatClient, "live_refresh"),
    "live/webrtc/answer": (MeerkatClient, "live_webrtc_answer"),
    "mob/create": (MeerkatClient, "create_mob"),
    "mob/list": (MeerkatClient, "list_mobs"),
    "mob/status": (Mob, "status"),
    "mob/members": (Mob, "members"),
    "mob/spawn": (Mob, "spawn"),
    "mob/spawn_many": (Mob, "spawn_many"),
    "mob/retire": (Mob, "retire"),
    "mob/respawn": (Mob, "respawn"),
    "mob/wire": (Mob, "wire"),
    "mob/wire_members_batch": (Mob, "wire_members_batch"),
    "mob/unwire": (Mob, "unwire"),
    "mob/lifecycle": (Mob, "lifecycle"),
    "mob/events": (Mob, "read_events"),
    "mob/stream_open": (MeerkatClient, "subscribe_mob_events"),
    "mob/stream_close": (MeerkatClient, "subscribe_mob_events"),
    "mob/member_send": (MeerkatClient, "send_mob_member_content"),
    "mob/ingress_interaction": (MeerkatClient, "mob_ingress_interaction"),
    "mob/append_system_context": (Mob, "append_system_context"),
    "mob/flows": (Mob, "flows"),
    "mob/run": (Mob, "run"),
    "mob/flow_run": (Mob, "run_flow"),
    "mob/flow_status": (Mob, "flow_status"),
    "mob/run_result": (Mob, "run_result"),
    "mob/flow_cancel": (Mob, "cancel_flow"),
    "mob/spawn_helper": (Mob, "spawn_helper"),
    "mob/fork_helper": (Mob, "fork_helper"),
    "mob/force_cancel": (Mob, "force_cancel"),
    "mob/member_status": (Mob, "member_status"),
    "mob/grant_scopes": (Mob, "grant_scopes"),
    "mob/revoke_scopes": (Mob, "revoke_scopes"),
    "mob/grants": (Mob, "grants"),
    "mob/member_history": (Mob, "member_history"),
    "mob/hosts": (Mob, "hosts"),
    "mob/route_installs": (Mob, "route_installs"),
    "mob/bind_host": (Mob, "bind_host"),
    "mob/revoke_host": (Mob, "revoke_host"),
    "mob/hard_cancel_member": (Mob, "hard_cancel"),
    "mob/member_live_open": (Mob, "member_live_open"),
    "mob/member_live_close": (Mob, "member_live_close"),
    "mob/member_live_status": (Mob, "member_live_status"),
    "mob/member_live_control": (Mob, "member_live_control"),
    "mob/wait_kickoff": (Mob, "wait_for_kickoff_complete"),
    "mob/wait_ready": (Mob, "wait_for_ready"),
    "mob/turn_start": (MeerkatClient, "mob_turn_start"),
    "mob/ensure_member": (MeerkatClient, "mob_ensure_member"),
    "mob/reconcile": (MeerkatClient, "mob_reconcile"),
    "mob/list_members_matching": (MeerkatClient, "mob_list_members_matching"),
    "mob/snapshot": (MeerkatClient, "mob_snapshot"),
    "mob/destroy": (MeerkatClient, "mob_destroy"),
    "mob/rotate_supervisor": (MeerkatClient, "mob_rotate_supervisor"),
    "mob/submit_work": (MeerkatClient, "mob_submit_work"),
    "mob/conclude_objective": (MeerkatClient, "mob_conclude_objective"),
    "mob/cancel_work": (MeerkatClient, "mob_cancel_work"),
    "mob/cancel_all_work": (MeerkatClient, "mob_cancel_all_work"),
    "mob/profile/create": (MeerkatClient, "create_mob_profile"),
    "mob/profile/get": (MeerkatClient, "get_mob_profile"),
    "mob/profile/list": (MeerkatClient, "list_mob_profiles"),
    "mob/profile/update": (MeerkatClient, "update_mob_profile"),
    "mob/profile/delete": (MeerkatClient, "delete_mob_profile"),
    "schedule/create": (MeerkatClient, "create_schedule"),
    "schedule/get": (MeerkatClient, "get_schedule"),
    "schedule/list": (MeerkatClient, "list_schedules"),
    "schedule/update": (MeerkatClient, "update_schedule"),
    "schedule/pause": (MeerkatClient, "pause_schedule"),
    "schedule/resume": (MeerkatClient, "resume_schedule"),
    "schedule/delete": (MeerkatClient, "delete_schedule"),
    "schedule/occurrences": (MeerkatClient, "list_schedule_occurrences"),
    "schedule/tools": (MeerkatClient, "list_schedule_tools"),
    "schedule/call": (MeerkatClient, "call_schedule_tool"),
    "models/catalog": (MeerkatClient, "get_models_catalog"),
    "workgraph/get": (MeerkatClient, "get_workgraph_item"),
    "workgraph/list": (MeerkatClient, "list_workgraph_items"),
    "workgraph/ready": (MeerkatClient, "list_ready_workgraph_items"),
    "workgraph/snapshot": (MeerkatClient, "get_workgraph_snapshot"),
    "workgraph/events": (MeerkatClient, "list_workgraph_events"),
    "workgraph/goal/status": (MeerkatClient, "get_workgraph_goal_status"),
    "workgraph/attention/list": (MeerkatClient, "list_workgraph_attention"),
    "tools/register": (MeerkatClient, "tool"),
}

_REPO_ROOT = Path(__file__).resolve().parents[3]
_RPC_METHODS_SCHEMA = _REPO_ROOT / "artifacts" / "schemas" / "rpc-methods.json"


def _authoritative_rpc_methods() -> set[str]:
    document = json.loads(_RPC_METHODS_SCHEMA.read_text(encoding="utf-8"))
    return {method["name"] for method in document["methods"]}


def _wrapper_routes_rpc(
    owner: type,
    method_name: str,
    rpc_method: str,
    seen: frozenset[tuple[type, str]] = frozenset(),
) -> bool:
    """Prove that this exact public wrapper reaches this exact RPC literal.

    Direct wrappers must call ``_request`` with the literal. Delegating
    ``Session``/``Mob`` wrappers are followed through their concrete
    ``self._client.<method>`` target, and private client shims are followed
    through ``self.<method>``. Stream wrappers route their two literal method
    arguments through ``_open_event_subscription``.

    The search is scoped to the declared owner/method call graph. A matching
    literal in an unrelated public method therefore cannot satisfy the gate.
    """

    key = (owner, method_name)
    if key in seen or not hasattr(owner, method_name):
        return False
    function = ast.parse(
        textwrap.dedent(inspect.getsource(getattr(owner, method_name)))
    ).body[0]
    if not isinstance(function, (ast.FunctionDef, ast.AsyncFunctionDef)):
        return False

    calls = [node for node in ast.walk(function) if isinstance(node, ast.Call)]
    for call in calls:
        if not isinstance(call.func, ast.Attribute):
            continue
        if call.func.attr == "_request" and call.args:
            literal = call.args[0]
            if isinstance(literal, ast.Constant) and literal.value == rpc_method:
                return True
        if call.func.attr == "_open_event_subscription" and any(
            isinstance(argument, ast.Constant) and argument.value == rpc_method
            for argument in call.args
        ):
            return True

    next_seen = seen | {key}
    for call in calls:
        if not isinstance(call.func, ast.Attribute):
            continue
        receiver = call.func.value
        target_owner: type | None = None
        if isinstance(receiver, ast.Name) and receiver.id == "self":
            target_owner = owner
        elif (
            isinstance(receiver, ast.Attribute)
            and isinstance(receiver.value, ast.Name)
            and receiver.value.id == "self"
            and receiver.attr == "_client"
        ):
            target_owner = MeerkatClient
        if target_owner is None or call.func.attr in {
            "_request",
            "_open_event_subscription",
        }:
            continue
        if _wrapper_routes_rpc(
            target_owner,
            call.func.attr,
            rpc_method,
            next_seen,
        ):
            return True
    return False


def test_python_public_wrapper_inventory_is_complete():
    authoritative = _authoritative_rpc_methods()
    assert set(RPC_PUBLIC_WRAPPERS) == authoritative
    for rpc_method, (owner, method_name) in RPC_PUBLIC_WRAPPERS.items():
        assert hasattr(owner, method_name), (
            f"missing wrapper {owner.__name__}.{method_name}"
        )
        assert _wrapper_routes_rpc(owner, method_name, rpc_method), (
            f"{owner.__name__}.{method_name} does not route {rpc_method}"
        )


def test_rpc_wrapper_route_check_is_method_specific():
    assert _wrapper_routes_rpc(
        MeerkatClient,
        "fork_session_at",
        "session/fork_at",
    )
    assert not _wrapper_routes_rpc(
        MeerkatClient,
        "fork_session_at",
        "session/fork_replace",
    )
    assert _wrapper_routes_rpc(Mob, "member_history", "mob/member_history")
    assert not _wrapper_routes_rpc(Mob, "member_history", "mob/hosts")


def test_rpc_inventory_is_loaded_from_authoritative_artifact():
    assert _RPC_METHODS_SCHEMA.is_file()
    assert _authoritative_rpc_methods()


@pytest.mark.asyncio
async def test_python_retired_runtime_session_wrappers_fail_before_transport():
    client = MeerkatClient()
    client._process = MagicMock()
    client._dispatcher = MagicMock()
    client._request = AsyncMock(
        side_effect=AssertionError("retired wrapper reached transport")
    )

    calls = [
        lambda: client.runtime_status("session-1"),
        lambda: client.runtime_submit("session-1", {"input_type": "prompt"}),
        lambda: client.runtime_submission("session-1", "input-1"),
        lambda: client.runtime_submissions("session-1"),
        lambda: client.runtime_retire("session-1"),
        lambda: client.runtime_reset("session-1"),
    ]
    for call in calls:
        with pytest.warns(
            DeprecationWarning, match="Retired runtime session control methods"
        ):
            with pytest.raises(MeerkatError) as exc_info:
                await call()
        assert exc_info.value.code == "METHOD_NOT_FOUND"
        assert "Retired runtime session control methods" in exc_info.value.message

    client._request.assert_not_called()


@pytest.mark.asyncio
async def test_list_sessions_with_filters_forwards_labels_limit_and_offset():
    client = MeerkatClient()
    client._process = MagicMock()
    client._dispatcher = MagicMock()
    client._request = AsyncMock(return_value={"sessions": []})

    await client.list_sessions(labels={"env": "test"}, limit=50, offset=10)

    client._request.assert_called_with(
        "session/list",
        {"labels": {"env": "test"}, "limit": 50, "offset": 10},
    )


@pytest.mark.asyncio
async def test_create_session_with_extended_create_fields():
    client = MeerkatClient()
    client._process = MagicMock()
    client._dispatcher = MagicMock()
    client._request = AsyncMock(
        return_value={
            "session_id": "s1",
            "text": "hello",
            "turns": 1,
            "tool_calls": 0,
            "usage": {"input_tokens": 1, "output_tokens": 1},
        }
    )

    await client.create_session(
        "hi",
        labels={"foo": "bar"},
        additional_instructions=["You are concise."],
        app_context={"app": "sdk"},
        shell_env={"FOO": "BAR"},
        enable_web_search=True,
        external_tools=[
            {
                "name": "tool_a",
                "description": "Tool A",
                "input_schema": {"type": "object"},
            }
        ],
    )

    args, _kwargs = client._request.call_args
    assert args[0] == "session/create"
    assert args[1]["labels"] == {"foo": "bar"}
    assert args[1]["additional_instructions"] == ["You are concise."]
    assert args[1]["app_context"] == {"app": "sdk"}
    assert args[1]["shell_env"] == {"FOO": "BAR"}
    assert args[1]["enable_web_search"] is True
    assert args[1]["external_tools"][0]["name"] == "tool_a"


@pytest.mark.asyncio
async def test_python_auth_helpers_send_binding_scoped_params():
    client = MeerkatClient()
    client._process = MagicMock()
    client._dispatcher = MagicMock()
    client._request = AsyncMock(return_value={})

    await client.auth_login_complete(
        "anthropic",
        "code",
        "state",
        realm_id="prod",
        binding_id="claude-console",
    )
    client._request.assert_called_with(
        "auth/login/complete",
        {
            "provider": "anthropic",
            "code": "code",
            "state": "state",
            "realm_id": "prod",
            "binding_id": "claude-console",
            "redirect_uri": "http://127.0.0.1:0/callback",
        },
    )

    await client.auth_login_device_complete(
        "anthropic",
        "device-code",
        realm_id="prod",
        binding_id="claude-console",
    )
    client._request.assert_called_with(
        "auth/login/device_complete",
        {
            "provider": "anthropic",
            "device_code": "device-code",
            "realm_id": "prod",
            "binding_id": "claude-console",
        },
    )

    await client.auth_provision_api_key(
        "access-token",
        realm_id="prod",
        binding_id="claude-console",
    )
    client._request.assert_called_with(
        "auth/login/provision_api_key",
        {
            "access_token": "access-token",
            "realm_id": "prod",
            "binding_id": "claude-console",
        },
    )

    await client.auth_status("prod", "claude-console")
    client._request.assert_called_with(
        "auth/status/get",
        {"realm_id": "prod", "binding_id": "claude-console"},
    )
    await client.auth_status("prod", "claude-console", "primary")
    client._request.assert_called_with(
        "auth/status/get",
        {
            "realm_id": "prod",
            "binding_id": "claude-console",
            "profile_id": "primary",
        },
    )

    await client.auth_logout("prod", "claude-console")
    client._request.assert_called_with(
        "auth/logout",
        {"realm_id": "prod", "binding_id": "claude-console"},
    )
    await client.auth_logout("prod", "claude-console", "primary")
    client._request.assert_called_with(
        "auth/logout",
        {
            "realm_id": "prod",
            "binding_id": "claude-console",
            "profile_id": "primary",
        },
    )


def test_typescript_auth_status_logout_helpers_send_profile_scoped_params():
    import pathlib

    client_ts = pathlib.Path(__file__).parents[2] / "typescript" / "src" / "client.ts"
    source = client_ts.read_text()
    for method_name in ["authStatusGet", "authLogout"]:
        start = source.index(f"async {method_name}")
        end = source.index("\n  }\n", start)
        body = source[start:end]
        assert "bindingId" in body
        assert "binding_id" in body
        assert "profileId?: string" in body
        assert "profileId !== undefined" in body
        assert "params.profile_id = profileId" in body


def test_web_auth_status_logout_helpers_send_profile_scoped_params():
    import pathlib

    auth_ts = pathlib.Path(__file__).parents[2] / "web" / "src" / "auth.ts"
    source = auth_ts.read_text()
    for method_name in ["status", "logout"]:
        start = source.index(f"async {method_name}(")
        end = source.index("\n  }\n", start)
        body = source[start:end]
        assert "binding_id" in body
        assert "profile_id?: string" in body
        assert "profile_id !== undefined" in body
        assert "params.profile_id = profile_id" in body


@pytest.mark.asyncio
async def test_spawn_many_uses_explicit_rpc_method():
    client = MeerkatClient()
    client._process = MagicMock()
    client._dispatcher = MagicMock()
    # Server-resolved `member_ref` is the sole identity handle on
    # `mob/spawn_many` responses (dogma #10). Binding-era
    # `agent_runtime_id` / `fence_token` are retired from the app-facing
    # wire shape.
    client._request = AsyncMock(
        return_value={
            "results": [
                {
                    "status": "spawned",
                    "result": {
                        "agent_identity": "m1",
                        "member_ref": _make_member_ref("mob1", "m1"),
                    },
                }
            ]
        }
    )

    mob = Mob(client, "mob1")
    specs = [{"profile": "p1", "agent_identity": "m1"}]
    results = await mob.spawn_many(specs)

    client._request.assert_called_with(
        "mob/spawn_many",
        {
            "mob_id": "mob1",
            "specs": specs,
        },
    )
    assert results.results[0].status == "spawned"
    assert results.results[0].result.agent_identity == "m1"
    assert results.results[0].result.member_ref == _make_member_ref("mob1", "m1")


@pytest.mark.asyncio
async def test_high_level_spawn_forwards_model_override():
    client = MagicMock()
    client.spawn_mob_member = AsyncMock(
        return_value={
            "mob_id": "mob1",
            "agent_identity": "m1",
            "member_ref": _make_member_ref("mob1", "m1"),
        }
    )

    mob = Mob(client, "mob1")
    await mob.spawn(
        profile="worker",
        agent_identity="m1",
        model_override="gpt-5.4",
    )

    client.spawn_mob_member.assert_awaited_once_with(
        "mob1",
        profile="worker",
        agent_identity="m1",
        initial_message=None,
        runtime_mode=None,
        backend=None,
        labels=None,
        context=None,
        additional_instructions=None,
        placement=None,
        binding=None,
        shell_env=None,
        auto_wire_parent=None,
        launch_mode=None,
        tool_access_policy=None,
        inherited_tool_filter=None,
        override_profile=None,
        model_override="gpt-5.4",
        auth_binding=None,
    )


@pytest.mark.asyncio
async def test_role_name_is_canonical_for_helper_calls():
    client = MeerkatClient()
    client._process = MagicMock()
    client._dispatcher = MagicMock()
    client._request = AsyncMock(
        return_value={
            "output": "ok",
            "tokens_used": 1,
            "agent_identity": "helper-1",
            "member_ref": _make_member_ref("mob1", "helper-1"),
        }
    )

    await client.spawn_mob_helper(
        "mob1",
        "hello",
        role_name="worker",
        model_override="gpt-helper",
    )
    client._request.assert_called_with(
        "mob/spawn_helper",
        {
            "mob_id": "mob1",
            "prompt": "hello",
            "agent_identity": None,
            "role_name": "worker",
            "model_override": "gpt-helper",
            "auth_binding": None,
            "runtime_mode": None,
            "backend": None,
        },
    )

    client._request = AsyncMock(
        return_value={
            "output": "ok",
            "tokens_used": 1,
            "agent_identity": "fork-1",
            "member_ref": _make_member_ref("mob1", "fork-1"),
        }
    )
    await client.fork_mob_helper(
        "mob1",
        "source-id",
        "hello",
        role_name="worker-role",
        model_override="gpt-fork",
    )
    client._request.assert_called_with(
        "mob/fork_helper",
        {
            "mob_id": "mob1",
            "source_member_id": "source-id",
            "prompt": "hello",
            "agent_identity": None,
            "role_name": "worker-role",
            "model_override": "gpt-fork",
            "fork_context": None,
            "auth_binding": None,
            "runtime_mode": None,
            "backend": None,
        },
    )


@pytest.mark.asyncio
async def test_get_mob_profile_not_found_returns_none():
    client = MeerkatClient()
    client._process = MagicMock()
    client._dispatcher = MagicMock()
    client._request = AsyncMock(return_value={"not_found": True, "name": "worker"})

    result = await client.get_mob_profile("worker")
    assert result is None


def test_sdk_never_uses_deprecated_runtime_or_input_method_strings():
    """Row 29 regression gate: the public Python SDK must not re-introduce
    the infrastructure-shaped `runtime/*` or `input/*` method verbs.
    """
    import pathlib

    sdk_src = pathlib.Path(__file__).parent.parent / "meerkat"
    forbidden = [
        '"runtime/state"',
        '"runtime/accept"',
        '"runtime/retire"',
        '"runtime/reset"',
        '"runtime/realtime_attachment_status"',
        '"runtime/realtime_attachment_statuses"',
        '"input/state"',
        '"input/list"',
    ]
    for path in sdk_src.rglob("*.py"):
        if "generated" in path.parts:
            continue
        text = path.read_text()
        for needle in forbidden:
            assert needle not in text, (
                f"Deprecated method string {needle!r} must not appear in {path}"
            )


def test_sdk_never_uses_retired_public_method_strings():
    """Surface-alignment regression gate: methods retired from the RPC
    catalog must not linger as SDK wrapper strings.
    """
    import pathlib

    retired_methods = [
        '"auth/profile/test"',
        '"runtime/session_status"',
        '"runtime/session_submission"',
        '"runtime/session_submissions"',
        '"runtime/session_submit"',
        '"runtime/session_retire"',
        '"runtime/session_reset"',
        '"realtime/open_info"',
        '"realtime/status"',
        '"realtime/capabilities"',
        '"session/realtime_attachment_status"',
        '"session/realtime_attachment_statuses"',
        '"skills/inspect"',
    ]
    sdk_roots = [
        pathlib.Path(__file__).parent.parent / "meerkat",
        pathlib.Path(__file__).parents[2] / "typescript" / "src",
    ]
    for root in sdk_roots:
        for path in root.rglob("*"):
            if path.suffix not in {".py", ".ts"}:
                continue
            if "generated" in path.parts:
                continue
            text = path.read_text()
            for needle in retired_methods:
                assert needle not in text, (
                    f"Retired method string {needle!r} must not appear in {path}"
                )


def test_mob_public_types_do_not_expose_incarnation_atoms():
    """Public mob SDK types use ``member_ref`` or ``current_session_id``;
    binding-era incarnation atoms stay out of the Python surface.
    """

    from meerkat.mob import MobMemberSnapshot, MobSpawnResult

    for typed_dict in (MobMemberSnapshot, MobSpawnResult):
        fields = set(typed_dict.__annotations__)
        assert "agent_runtime_id" not in fields
        assert "fence_token" not in fields
