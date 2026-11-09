"""Mock-RPC tests for the non-destructive reload verb and the member health read.

`reload_member` is the repair for a durability-degraded member (meerkat's
`RecoveryRepairBlocked`); it must hit `meta/reload_member`, never the
destructive `meta/respawn_member`. `member_health` is the in-process read
that answers while the mob actor loop is stalled.
"""

import pytest

from meerkat_meta import MemberHealth, MemberReloadResult

from .test_rpc_method_names import make_mock_mob_handle


@pytest.mark.asyncio
async def test_reload_member_rpc_name_and_typed_result():
    handle, calls = make_mock_mob_handle({
        "meta/reload_member": {
            "reloaded": True,
            "disposition": "discarded",
            "session_id": "sess-1",
            "generation": 4,
            "identity": "review:singleton",
            "identity_first": True,
        }
    })
    result = await handle.reload_member("review:singleton")
    assert calls[0] == ("meta/reload_member", {"member_id": "review:singleton"})
    assert isinstance(result, MemberReloadResult)
    assert result.reloaded is True
    assert result.disposition == "discarded"
    assert result.session_id == "sess-1"
    assert result.generation == 4
    assert result.identity == "review:singleton"


@pytest.mark.asyncio
async def test_reload_member_never_calls_respawn():
    handle, calls = make_mock_mob_handle({
        "meta/reload_member": {"reloaded": False, "disposition": "not_current"}
    })
    result = await handle.reload_member("review:singleton")
    assert [method for method, _ in calls] == ["meta/reload_member"]
    assert result.reloaded is False
    assert result.disposition == "not_current"
    assert result.session_id is None
    assert result.generation is None


@pytest.mark.asyncio
async def test_reload_member_tolerates_unknown_disposition():
    handle, _ = make_mock_mob_handle({
        "meta/reload_member": {"reloaded": False, "disposition": "some_future_value"}
    })
    result = await handle.reload_member("x")
    assert result.disposition == "some_future_value"


@pytest.mark.asyncio
async def test_member_health_rpc_name_and_typed_result():
    handle, calls = make_mock_mob_handle({
        "meta/member_health": {
            "identity": "review:singleton",
            "member_id": "rt:review:singleton:4",
            "state": "active",
            "bootstrap_state": "active",
            "materialization_in_flight": False,
            "session_id": "sess-1",
            "generation": 4,
            "last_delivery_error": {
                "class": "reload_required",
                "detail": "Runtime recovery is repair-blocked",
                "at_unix_ms": 1_700_000_000_000,
            },
            "actor_loop": {"state": "stalled", "stall_id": 7, "stalled_for_secs": 42},
            "open_stall_id": 7,
        }
    })
    health = await handle.member_health("review:singleton")
    assert calls[0] == ("meta/member_health", {"member_id": "review:singleton"})
    assert isinstance(health, MemberHealth)
    assert health.identity == "review:singleton"
    assert health.member_id == "rt:review:singleton:4"
    assert health.state == "active"
    assert health.bootstrap_state == "active"
    assert health.materialization_in_flight is False
    assert health.session_id == "sess-1"
    assert health.generation == 4
    assert health.last_delivery_error["class"] == "reload_required"
    assert health.actor_loop["state"] == "stalled"
    assert health.open_stall_id == 7
    # Absent until the gateway can read meerkat's durability state.
    assert health.durability is None
    assert health.last_reload is None


@pytest.mark.asyncio
async def test_member_health_carries_last_reload():
    handle, _ = make_mock_mob_handle({
        "meta/member_health": {
            "identity": "review:singleton",
            "state": "active",
            "materialization_in_flight": False,
            "actor_loop": {"state": "live"},
            "last_reload": {
                "outcome": "refused",
                "detail": "durable resume authority unreadable: HTTP 0",
                "at_unix_ms": 1_700_000_000_000,
            },
        }
    })
    health = await handle.member_health("review:singleton")
    assert health.last_reload["outcome"] == "refused"
    assert "HTTP 0" in health.last_reload["detail"]


@pytest.mark.asyncio
async def test_member_health_carries_durability_when_present():
    handle, _ = make_mock_mob_handle({
        "meta/member_health": {
            "identity": "review:singleton",
            "state": "active",
            "materialization_in_flight": False,
            "actor_loop": {"state": "live"},
            "durability": {
                "reload_required": {"operation": "completed_boundary_commit", "reason": "HTTP 0"}
            },
        }
    })
    health = await handle.member_health("review:singleton")
    assert health.durability == {
        "reload_required": {"operation": "completed_boundary_commit", "reason": "HTTP 0"}
    }
    assert health.open_stall_id is None
    assert health.last_delivery_error is None
    assert health.actor_loop == {"state": "live"}


@pytest.mark.asyncio
async def test_member_health_degrades_on_missing_actor_loop():
    handle, _ = make_mock_mob_handle({"meta/member_health": {"identity": "x", "state": "dormant"}})
    health = await handle.member_health("x")
    assert health.actor_loop == {"state": "unobserved"}
    assert health.materialization_in_flight is False


@pytest.mark.asyncio
@pytest.mark.parametrize("stage", ["actor_command_admission", "actor_command_reply"])
async def test_timeout_observations_survive_health_and_rpc_errors(stage):
    from meerkat_meta.errors import RpcError

    data = {
        "kind": "mob_actor_command_timed_out",
        "command_kind": "SubmitWork",
        "stage": stage,
        "deadline_reached": True,
    }
    handle, _ = make_mock_mob_handle({
        "meta/member_health": {
            "identity": "x",
            "state": "active",
            "last_delivery_error": {
                "class": "admission_timeout",
                "detail": "execution fate not established",
                "data": data,
                "at_unix_ms": 1,
            },
        }
    })
    health = await handle.member_health("x")
    assert health.last_delivery_error["data"] == data
    error = RpcError(-32603, "observation deadline", data=data)
    calls = []

    async def timed_out_rpc(method, params=None):
        calls.append(method)
        raise error

    handle._runtime._rpc = timed_out_rpc
    with pytest.raises(RpcError) as caught:
        await handle.send("x", "turn")
    assert caught.value.data == data
    assert calls == ["meta/send_message"]
    assert "executed" not in error.data
    assert "retryable" not in error.data
