from __future__ import annotations

from unittest.mock import AsyncMock, MagicMock

import pytest

from meerkat.client import MeerkatClient
from meerkat.mob import Mob
from meerkat.session import DeferredSession, Session


CANONICAL_APP_RPC_METHODS = {
    "session/create",
    "session/list",
    "session/read",
    "session/history",
    "session/archive",
    "session/external_event",
    "session/inject_context",
    "turn/start",
    "turn/interrupt",
    "blob/get",
    "skills/list",
    "skills/inspect",
    "mcp/add",
    "mcp/remove",
    "mcp/reload",
    "comms/send",
    "comms/peers",
    "runtime/state",
    "runtime/accept",
    "runtime/retire",
    "runtime/reset",
    "input/state",
    "input/list",
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
    "session/archive": (Session, "archive"),
    "session/external_event": (MeerkatClient, "send_external_event"),
    "session/inject_context": (MeerkatClient, "inject_context"),
    "turn/start": (Session, "turn"),
    "turn/interrupt": (Session, "interrupt"),
    "blob/get": (MeerkatClient, "get_blob"),
    "skills/list": (MeerkatClient, "list_skills"),
    "skills/inspect": (MeerkatClient, "inspect_skill"),
    "mcp/add": (MeerkatClient, "mcp_add"),
    "mcp/remove": (MeerkatClient, "mcp_remove"),
    "mcp/reload": (MeerkatClient, "mcp_reload"),
    "comms/send": (Session, "send"),
    "comms/peers": (Session, "peers"),
    "runtime/state": (MeerkatClient, "runtime_state"),
    "runtime/accept": (MeerkatClient, "runtime_accept"),
    "runtime/retire": (MeerkatClient, "runtime_retire"),
    "runtime/reset": (MeerkatClient, "runtime_reset"),
    "input/state": (MeerkatClient, "input_state"),
    "input/list": (MeerkatClient, "input_list"),
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
    client._request = AsyncMock(return_value={"session_id": "s1", "text": "hello"})

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
async def test_spawn_many_uses_explicit_rpc_method():
    client = MeerkatClient()
    client._process = MagicMock()
    client._dispatcher = MagicMock()
    client._request = AsyncMock(return_value=[{"ok": True, "meerkat_id": "m1"}])

    mob = Mob(client, "mob1")
    specs = [{"profile": "p1", "meerkat_id": "m1"}]
    results = await mob.spawn_many(specs)

    client._request.assert_called_with(
        "mob/spawn_many",
        {
            "mob_id": "mob1",
            "specs": specs,
        },
    )
    assert results[0]["meerkat_id"] == "m1"


@pytest.mark.asyncio
async def test_role_name_is_canonical_for_helper_calls_with_profile_alias():
    client = MeerkatClient()
    client._process = MagicMock()
    client._dispatcher = MagicMock()
    client._request = AsyncMock(return_value={"output": "ok", "tokens_used": 1})

    await client.spawn_mob_helper(
        "mob1",
        "hello",
        profile_name="legacy-role",
    )
    client._request.assert_called_with(
        "mob/spawn_helper",
        {
            "mob_id": "mob1",
            "prompt": "hello",
            "meerkat_id": None,
            "role_name": "legacy-role",
            "runtime_mode": None,
            "backend": None,
        },
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
            "meerkat_id": None,
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
