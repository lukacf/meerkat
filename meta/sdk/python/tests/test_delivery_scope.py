"""Scope-bound dispatch for the Python SDK.

A host reads an identity's ``DeliveryScope`` from ``status()``, persists it,
dispatches with ``expected_scope`` and recovers a lost reply from the scope's
ORIGINAL session with ``recover_delivery``. The SDK carries the scope
byte-exact, refuses scope versions it cannot read, raises ``StaleScopeError``
for a moved scope, and keeps every recovery class distinct.
"""
import asyncio
import copy
import json

import pytest

from meerkat_meta import (
    DELIVERY_SCOPE_VERSION,
    STALE_DELIVERY_SCOPE_CODE,
    ContractMismatchError,
    DeliveryScope,
    RpcError,
    ScopedDispatchResult,
    ScopedRecovery,
    ScopedRecoveryState,
    StaleScopeError,
)
from meerkat_meta.identity_first_models import DispatchInput, IdentityStatus
from meerkat_meta.runtime import IdentityAgentHandle, MetaRuntime

SCOPE = {
    "version": 1,
    "identity": "personal:alice",
    "agent_runtime_id": "rt-alice-1",
    "generation": 2,
    "lease_fencing_token": 5,
    "member": {
        "version": 1,
        "runtime_id": {"identity": "personal:alice", "generation": 3},
        "fence_token": 7,
        "session_id": "019245f0-0000-7000-8000-000000000001",
    },
}


class ScriptedTransport:
    """Answers each RPC with the next scripted reply for its method."""

    def __init__(self, replies):
        self.calls: list[dict] = []
        self.request_timeout = 60.0
        self._replies = {method: list(items) for method, items in replies.items()}

    def send_sync(self, request):
        self.calls.append(request)
        reply = self._replies[request["method"]].pop(0)
        if "error" in reply:
            return {"jsonrpc": "2.0", "id": request.get("id"), "error": reply["error"]}
        return {"jsonrpc": "2.0", "id": request.get("id"), "result": reply["result"]}

    async def send_async(self, request, *, timeout=None):
        await asyncio.sleep(0)
        return self.send_sync(request)

    def is_running(self):
        return True

    def start(self):
        pass

    def stop(self):
        pass

    def set_callback_handler(self, handler):
        pass

    def params_of(self, method: str) -> list[dict]:
        return [call.get("params") or {} for call in self.calls if call["method"] == method]


def _make_runtime(transport) -> MetaRuntime:
    rt = MetaRuntime.__new__(MetaRuntime)
    rt._config = None
    rt._transport = transport
    rt._running = True
    rt._rust_http_base = None
    rt._lifecycle_lock = asyncio.Lock()
    rt._shutdown_task = None
    from meerkat_meta.agent_builder import CallbackDispatcher

    rt._dispatcher = CallbackDispatcher()
    return rt


def _status(**extra) -> dict:
    return {"identity": "personal:alice", "state": "active", **extra}


class TestDeliveryScopeModel:
    def test_round_trips_the_persisted_form_byte_exact(self):
        scope = DeliveryScope.from_dict(json.loads(json.dumps(SCOPE)))
        assert scope.version == DELIVERY_SCOPE_VERSION == 1
        assert scope.session_id == SCOPE["member"]["session_id"]
        assert scope.to_dict() == SCOPE
        assert DeliveryScope.from_dict(scope.to_dict()) == scope

    def test_the_member_scope_is_carried_opaque_and_never_shared(self):
        raw = copy.deepcopy(SCOPE)
        scope = DeliveryScope.from_dict(raw)
        raw["member"]["fence_token"] = 99
        assert scope.to_dict()["member"]["fence_token"] == 7
        out = scope.to_dict()
        out["member"]["fence_token"] = 42
        assert scope.member["fence_token"] == 7

    def test_an_unknown_version_is_a_contract_mismatch_never_a_guess(self):
        future = {**SCOPE, "version": 2}
        with pytest.raises(ContractMismatchError, match="version 2"):
            DeliveryScope.from_dict(future)

    @pytest.mark.parametrize(
        "patch",
        [
            {"version": "1"},
            {"version": True},
            {"identity": ""},
            {"agent_runtime_id": 7},
            {"generation": -1},
            {"lease_fencing_token": True},
            {"member": "opaque"},
        ],
    )
    def test_a_malformed_scope_is_refused(self, patch):
        with pytest.raises(ValueError):
            DeliveryScope.from_dict({**SCOPE, **patch})

    def test_status_carries_the_scope_or_the_reason_it_is_unavailable(self):
        status = IdentityStatus.from_dict(_status(delivery_scope=SCOPE))
        assert status.delivery_scope == DeliveryScope.from_dict(SCOPE)
        assert status.delivery_scope_unavailable is None
        assert status.to_dict()["delivery_scope"] == SCOPE

        unavailable = IdentityStatus.from_dict(
            _status(
                delivery_scope=None,
                delivery_scope_unavailable={
                    "kind": "scoped_delivery_unsupported",
                    "reason": "custom bridge",
                },
            )
        )
        assert unavailable.delivery_scope is None
        assert unavailable.delivery_scope_unavailable["kind"] == "scoped_delivery_unsupported"

    def test_a_future_scope_in_status_never_breaks_the_rest_of_status(self):
        status = IdentityStatus.from_dict(
            _status(session_id="s-1", delivery_scope={**SCOPE, "version": 3})
        )
        assert status.session_id == "s-1"
        assert status.delivery_scope is None
        assert status.delivery_scope_unavailable["kind"] == "unsupported_delivery_scope_version"


class TestScopedDispatch:
    @pytest.mark.asyncio
    async def test_sends_the_exact_scope_and_returns_the_receipt(self):
        receipt = {
            "work_ref": "wr-1",
            "stage": "ingress_accepted",
            "session_id": SCOPE["member"]["session_id"],
        }
        transport = ScriptedTransport(
            {
                "meta/dispatch": [
                    {
                        "result": {
                            "receipt": receipt,
                            "delivery_scope": SCOPE,
                            "fencing_token": 5,
                        }
                    }
                ]
            }
        )
        rt = _make_runtime(transport)
        scope = DeliveryScope.from_dict(SCOPE)
        result = await rt.dispatch(
            "personal:alice",
            DispatchInput(
                content="occurrence", origin="system", idempotency_key="k-1", correlation_id="c-1"
            ),
            expected_scope=scope,
        )
        assert isinstance(result, ScopedDispatchResult)
        assert result.receipt.work_ref == "wr-1"
        assert result.receipt.stage == "ingress_accepted"
        assert result.receipt.session_id == scope.session_id
        assert result.delivery_scope == scope
        (params,) = transport.params_of("meta/dispatch")
        assert params["expected_scope"] == SCOPE
        assert "track_turn" not in params

    @pytest.mark.asyncio
    async def test_the_handle_forwards_the_scope(self):
        transport = ScriptedTransport(
            {
                "meta/dispatch": [
                    {
                        "result": {
                            "receipt": {"work_ref": "wr", "stage": "ingress_accepted", "session_id": "s"},
                            "delivery_scope": SCOPE,
                            "fencing_token": 5,
                        }
                    }
                ]
            }
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "personal:alice")
        await handle.dispatch_text(
            "text",
            idempotency_key="k",
            correlation_id="c",
            expected_scope=DeliveryScope.from_dict(SCOPE),
        )
        assert transport.params_of("meta/dispatch")[0]["expected_scope"] == SCOPE

    @pytest.mark.asyncio
    async def test_a_scope_never_combines_with_track_turn(self):
        transport = ScriptedTransport({})
        rt = _make_runtime(transport)
        with pytest.raises(ValueError, match="track_turn"):
            await rt.dispatch(
                "personal:alice",
                DispatchInput(content="x", origin="system", idempotency_key="k", correlation_id="c"),
                expected_scope=DeliveryScope.from_dict(SCOPE),
                track_turn=True,
            )
        assert transport.calls == []

    @pytest.mark.asyncio
    async def test_a_moved_scope_raises_stale_scope_error(self):
        transport = ScriptedTransport(
            {
                "meta/dispatch": [
                    {
                        "error": {
                            "code": STALE_DELIVERY_SCOPE_CODE,
                            "message": "stale delivery scope (generation): moved",
                            "data": {
                                "kind": "stale_delivery_scope",
                                "admission_possible": False,
                                "mismatch": "generation",
                            },
                        }
                    }
                ]
            }
        )
        rt = _make_runtime(transport)
        with pytest.raises(StaleScopeError) as raised:
            await rt.dispatch(
                "personal:alice",
                DispatchInput(content="x", origin="system", idempotency_key="k", correlation_id="c"),
                expected_scope=DeliveryScope.from_dict(SCOPE),
            )
        assert raised.value.code == STALE_DELIVERY_SCOPE_CODE == -32007
        assert raised.value.mismatch == "generation"
        assert raised.value.data["admission_possible"] is False
        assert isinstance(raised.value, RpcError)

    @pytest.mark.asyncio
    async def test_an_uncertain_admission_stays_a_typed_rpc_error(self):
        transport = ScriptedTransport(
            {
                "meta/dispatch": [
                    {
                        "error": {
                            "code": -32603,
                            "message": "scoped delivery outcome uncertain: timeout",
                            "data": {
                                "kind": "scoped_delivery_uncertain",
                                "admission_possible": True,
                            },
                        }
                    }
                ]
            }
        )
        rt = _make_runtime(transport)
        with pytest.raises(RpcError) as raised:
            await rt.dispatch(
                "personal:alice",
                DispatchInput(content="x", origin="system", idempotency_key="k", correlation_id="c"),
                expected_scope=DeliveryScope.from_dict(SCOPE),
            )
        assert not isinstance(raised.value, StaleScopeError)
        assert raised.value.data["kind"] == "scoped_delivery_uncertain"
        assert raised.value.data["admission_possible"] is True


RECOVERIES = [
    ({"state": "absent"}, ScopedRecoveryState.ABSENT),
    (
        {"state": "in_flight", "input_id": "in-1", "phase": "queued", "durable_witness": True},
        ScopedRecoveryState.IN_FLIGHT,
    ),
    (
        {
            "state": "completed",
            "input_id": "in-1",
            "output_status": "text",
            "output": "answer",
            "output_truncated": True,
        },
        ScopedRecoveryState.COMPLETED,
    ),
    ({"state": "failed", "input_id": "in-1", "error": "boom"}, ScopedRecoveryState.FAILED),
    (
        {
            "state": "terminal_without_run",
            "input_id": "in-1",
            "terminal": {"outcome_type": "abandoned"},
            "last_run_id": None,
        },
        ScopedRecoveryState.TERMINAL_WITHOUT_RUN,
    ),
    ({"state": "broken", "input_id": None, "reason": "inconsistent"}, ScopedRecoveryState.BROKEN),
    (
        {"state": "unresolved", "cause": "original_owner_unavailable", "detail": "store down"},
        ScopedRecoveryState.UNRESOLVED,
    ),
]


class TestRecoverDelivery:
    @pytest.mark.asyncio
    @pytest.mark.parametrize("wire,state", RECOVERIES)
    async def test_every_recovery_class_stays_distinct(self, wire, state):
        transport = ScriptedTransport(
            {
                "meta/recover_delivery": [
                    {
                        "result": {
                            "identity": "personal:alice",
                            "delivery_scope": SCOPE,
                            "recovery": wire,
                        }
                    }
                ]
            }
        )
        rt = _make_runtime(transport)
        recovery = await rt.recover_delivery(
            "personal:alice",
            DeliveryScope.from_dict(SCOPE),
            idempotency_key="k-1",
            correlation_id="c-1",
            timeout=2.5,
        )
        assert isinstance(recovery, ScopedRecovery)
        assert recovery.state is state
        assert recovery.delivery_scope == DeliveryScope.from_dict(SCOPE)
        (params,) = transport.params_of("meta/recover_delivery")
        assert params == {
            "identity": "personal:alice",
            "scope": SCOPE,
            "idempotency_key": "k-1",
            "correlation_id": "c-1",
            "timeout_ms": 2500,
        }
        if state is ScopedRecoveryState.IN_FLIGHT:
            assert recovery.durable_witness is True
            assert recovery.phase == "queued"
        if state is ScopedRecoveryState.COMPLETED:
            assert (recovery.output_status, recovery.output, recovery.output_truncated) == (
                "text",
                "answer",
                True,
            )
        if state is ScopedRecoveryState.UNRESOLVED:
            assert recovery.cause == "original_owner_unavailable"
            assert recovery.detail == "store down"

    def test_an_unknown_state_is_unresolved_never_absent(self):
        recovery = ScopedRecovery.from_dict({"recovery": {"state": "superseded"}})
        assert recovery.state is ScopedRecoveryState.UNRESOLVED
        assert recovery.cause == "unknown_state:superseded"

    @pytest.mark.asyncio
    async def test_the_handle_recovers_its_own_identity(self):
        transport = ScriptedTransport(
            {"meta/recover_delivery": [{"result": {"recovery": {"state": "absent"}}}]}
        )
        handle = IdentityAgentHandle(_make_runtime(transport), "personal:alice")
        recovery = await handle.recover_delivery(
            DeliveryScope.from_dict(SCOPE), idempotency_key="k", correlation_id="c"
        )
        assert recovery.state is ScopedRecoveryState.ABSENT
        (params,) = transport.params_of("meta/recover_delivery")
        assert params["identity"] == "personal:alice"
        assert "timeout_ms" not in params
