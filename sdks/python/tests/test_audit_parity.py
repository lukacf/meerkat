from __future__ import annotations

import base64
import json
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
    return base64.urlsafe_b64encode(payload.encode("utf-8")).rstrip(b"=").decode("ascii")


CANONICAL_APP_RPC_METHODS = {
    "session/create",
    "session/list",
    "session/read",
    "session/history",
    "session/fork_at",
    "session/fork_replace",
    "session/archive",
    "session/external_event",
    "session/peer_response_terminal",
    "session/inject_context",
    "turn/start",
    "turn/interrupt",
    "blob/get",
    "skills/list",
    "mcp/add",
    "mcp/remove",
    "mcp/reload",
    "comms/send",
    "comms/peers",
    "realtime/open_info",
    "realtime/status",
    "realtime/capabilities",
    "session/realtime_attachment_status",
    "mob/create",
    "mob/list",
    "mob/status",
    "mob/members",
    "mob/spawn",
    "mob/spawn_many",
    "mob/retire",
    "mob/respawn",
    "mob/wire",
    "mob/unwire",
    "mob/lifecycle",
    "mob/events",
    "mob/member_send",
    "mob/append_system_context",
    "mob/flows",
    "mob/flow_run",
    "mob/flow_status",
    "mob/flow_cancel",
    "mob/spawn_helper",
    "mob/fork_helper",
    "mob/force_cancel",
    "mob/member_status",
    "mob/wait_kickoff",
    "mob/wait_ready",
    "mob/profile/create",
    "mob/profile/get",
    "mob/profile/list",
    "mob/profile/update",
    "mob/profile/delete",
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
    "models/catalog",
}

RPC_PUBLIC_WRAPPERS: dict[str, tuple[type, str]] = {
    "session/create": (MeerkatClient, "create_session"),
    "session/list": (MeerkatClient, "list_sessions"),
    "session/read": (MeerkatClient, "read_session"),
    "session/history": (MeerkatClient, "read_session_history"),
    "session/fork_at": (MeerkatClient, "fork_session_at"),
    "session/fork_replace": (MeerkatClient, "fork_session_replace"),
    "session/archive": (Session, "archive"),
    "session/external_event": (MeerkatClient, "send_external_event"),
    "session/peer_response_terminal": (MeerkatClient, "send_peer_response_terminal"),
    "session/inject_context": (MeerkatClient, "inject_context"),
    "turn/start": (Session, "turn"),
    "turn/interrupt": (Session, "interrupt"),
    "blob/get": (MeerkatClient, "get_blob"),
    "skills/list": (MeerkatClient, "list_skills"),
    "mcp/add": (MeerkatClient, "mcp_add"),
    "mcp/remove": (MeerkatClient, "mcp_remove"),
    "mcp/reload": (MeerkatClient, "mcp_reload"),
    "comms/send": (Session, "send"),
    "comms/peers": (Session, "peers"),
    "realtime/open_info": (MeerkatClient, "realtime_open_info"),
    "realtime/status": (MeerkatClient, "realtime_status"),
    "realtime/capabilities": (MeerkatClient, "realtime_capabilities"),
    "session/realtime_attachment_status": (MeerkatClient, "runtime_realtime_attachment_status"),
    "mob/create": (MeerkatClient, "create_mob"),
    "mob/list": (MeerkatClient, "list_mobs"),
    "mob/status": (Mob, "status"),
    "mob/members": (Mob, "members"),
    "mob/spawn": (Mob, "spawn"),
    "mob/spawn_many": (Mob, "spawn_many"),
    "mob/retire": (Mob, "retire"),
    "mob/respawn": (Mob, "respawn"),
    "mob/wire": (Mob, "wire"),
    "mob/unwire": (Mob, "unwire"),
    "mob/lifecycle": (Mob, "lifecycle"),
    "mob/events": (Mob, "read_events"),
    "mob/member_send": (MeerkatClient, "send_mob_member_content"),
    "mob/append_system_context": (Mob, "append_system_context"),
    "mob/flows": (Mob, "flows"),
    "mob/flow_run": (Mob, "run_flow"),
    "mob/flow_status": (Mob, "flow_status"),
    "mob/flow_cancel": (Mob, "cancel_flow"),
    "mob/spawn_helper": (Mob, "spawn_helper"),
    "mob/fork_helper": (Mob, "fork_helper"),
    "mob/force_cancel": (Mob, "force_cancel"),
    "mob/member_status": (Mob, "member_status"),
    "mob/wait_kickoff": (Mob, "wait_for_kickoff_complete"),
    "mob/wait_ready": (Mob, "wait_for_ready"),
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
}

EXCLUSIONS = {
    "initialize",
    "tools/register",
    "session/stream_open",
    "session/stream_close",
    "mob/stream_open",
    "mob/stream_close",
}


def test_python_public_wrapper_inventory_is_complete():
    missing_from_map = CANONICAL_APP_RPC_METHODS - set(RPC_PUBLIC_WRAPPERS)
    assert missing_from_map == set()
    for rpc_method, (owner, method_name) in RPC_PUBLIC_WRAPPERS.items():
        assert rpc_method in CANONICAL_APP_RPC_METHODS
        assert hasattr(owner, method_name), f"missing wrapper {owner.__name__}.{method_name}"


def test_parity_exclusion_set_is_small_and_justified():
    assert EXCLUSIONS == {
        "initialize",
        "tools/register",
        "session/stream_open",
        "session/stream_close",
        "mob/stream_open",
        "mob/stream_close",
    }


@pytest.mark.asyncio
async def test_python_retired_runtime_session_wrappers_fail_before_transport():
    client = MeerkatClient()
    client._process = MagicMock()
    client._dispatcher = MagicMock()
    client._request = AsyncMock(side_effect=AssertionError("retired wrapper reached transport"))

    calls = [
        lambda: client.runtime_status("session-1"),
        lambda: client.runtime_submit("session-1", {"input_type": "prompt"}),
        lambda: client.runtime_submission("session-1", "input-1"),
        lambda: client.runtime_submissions("session-1"),
        lambda: client.runtime_retire("session-1"),
        lambda: client.runtime_reset("session-1"),
    ]
    for call in calls:
        with pytest.warns(DeprecationWarning, match="Retired runtime session control methods"):
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
        external_tools=[{"name": "tool_a", "description": "Tool A", "input_schema": {"type": "object"}}],
    )

    args, _kwargs = client._request.call_args
    assert args[0] == "session/create"
    assert args[1]["labels"] == {"foo": "bar"}
    assert args[1]["additional_instructions"] == ["You are concise."]
    assert args[1]["app_context"] == {"app": "sdk"}
    assert args[1]["shell_env"] == {"FOO": "BAR"}
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
    )
    client._request.assert_called_with(
        "mob/spawn_helper",
        {
            "mob_id": "mob1",
            "prompt": "hello",
            "agent_identity": None,
            "role_name": "worker",
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
    )
    client._request.assert_called_with(
        "mob/fork_helper",
        {
            "mob_id": "mob1",
            "source_member_id": "source-id",
            "prompt": "hello",
            "agent_identity": None,
            "role_name": "worker-role",
            "fork_context": None,
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
