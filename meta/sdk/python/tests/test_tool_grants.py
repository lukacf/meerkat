"""Meta SDK wrappers of the scoped tool grant methods (``meta/tool_grants/*``).

Each wrapper sends its method with exactly the params it names; a grant names
exactly one target (a tool or a family); a grant's ``applies`` and a list's
``gate`` reach the caller as the gateway answers them; and every refusal of
the methods is the typed ``tool_grant_request_refused`` error, whose kinds
are a closed vocabulary.
"""

import asyncio

import pytest

from meerkat_meta.runtime import MetaRuntime, _rpc_error_from_payload


class FakeTransport:
    """Records RPC calls for assertion."""

    def __init__(self, result=None):
        self.calls: list[dict] = []
        self.request_timeout = 60.0
        self._result = result or {}

    def send_sync(self, request):
        self.calls.append(request)
        return {"jsonrpc": "2.0", "id": request.get("id"), "result": self._result}

    async def send_async(self, request, *, timeout=None):
        return self.send_sync(request)

    def is_running(self):
        return True

    def start(self):
        pass

    def stop(self):
        pass

    def set_callback_handler(self, handler):
        pass


def make_runtime(result=None):
    from meerkat_meta.agent_builder import CallbackDispatcher

    transport = FakeTransport(result=result)
    runtime = MetaRuntime.__new__(MetaRuntime)
    runtime._config = None
    runtime._transport = transport
    runtime._running = True
    runtime._rust_http_base = None
    runtime._lifecycle_lock = asyncio.Lock()
    runtime._shutdown_task = None
    runtime._dispatcher = CallbackDispatcher()
    return runtime, transport


@pytest.mark.asyncio
async def test_grant_tool_sends_the_identity_target_window_and_decision_ref():
    runtime, transport = make_runtime(result={"grant": {"grant_id": "g1"}})
    await runtime.grant_tool(
        "agent-a",
        tool="datetime",
        duration_ms=600_000,
        reason="approved",
        decision_ref="approval-7",
    )
    await runtime.grant_tool("agent-a", family="shell", expires_at_ms=1_900_000_000_000)
    assert [(call["method"], call["params"]) for call in transport.calls] == [
        (
            "meta/tool_grants/grant",
            {
                "identity": "agent-a",
                "target": {"kind": "tool", "name": "datetime"},
                "duration_ms": 600_000,
                "reason": "approved",
                "decision_ref": "approval-7",
            },
        ),
        (
            "meta/tool_grants/grant",
            {
                "identity": "agent-a",
                "target": {"kind": "family", "family": "shell"},
                "expires_at_ms": 1_900_000_000_000,
            },
        ),
    ]


@pytest.mark.asyncio
async def test_grant_tool_names_exactly_one_target():
    runtime, transport = make_runtime()
    with pytest.raises(ValueError):
        await runtime.grant_tool("agent-a", duration_ms=1_000)
    with pytest.raises(ValueError):
        await runtime.grant_tool("agent-a", tool="datetime", family="shell", duration_ms=1_000)
    assert transport.calls == []


@pytest.mark.asyncio
async def test_revoke_list_and_audit_send_their_params():
    runtime, transport = make_runtime()
    await runtime.revoke_tool_grant("g1", reason="done")
    await runtime.revoke_tool_grant("g2")
    await runtime.list_tool_grants("agent-a")
    await runtime.list_tool_grants("agent-a", include_ended=True)
    await runtime.tool_grant_audit("agent-a")
    await runtime.tool_grant_audit("agent-a", after_sequence=4, limit=10)
    assert [(call["method"], call["params"]) for call in transport.calls] == [
        ("meta/tool_grants/revoke", {"grant_id": "g1", "reason": "done"}),
        ("meta/tool_grants/revoke", {"grant_id": "g2"}),
        ("meta/tool_grants/list", {"identity": "agent-a", "include_ended": False}),
        ("meta/tool_grants/list", {"identity": "agent-a", "include_ended": True}),
        ("meta/tool_grants/audit", {"identity": "agent-a"}),
        (
            "meta/tool_grants/audit",
            {"identity": "agent-a", "after_sequence": 4, "limit": 10},
        ),
    ]


@pytest.mark.asyncio
async def test_grant_and_list_answer_applies_and_the_member_gate_as_the_gateway_does():
    granted = {"grant": {"grant_id": "g1"}, "applies": "at_next_build"}
    runtime, _ = make_runtime(result=granted)
    assert (
        await runtime.grant_tool("agent-a", tool="datetime", duration_ms=600_000)
        == granted
    )

    listed = {
        "grants": [{"grant_id": "g1"}],
        "gate": {
            "running": {"gated": {"tools": ["datetime"]}, "not_mounted": {}},
            "next_build": {"gated": {"tools": ["datetime"]}, "not_mounted": {}},
        },
    }
    runtime, _ = make_runtime(result=listed)
    assert await runtime.list_tool_grants("agent-a") == listed


def test_a_grant_request_refusal_is_a_typed_error_naming_its_kind():
    from meerkat_meta import ToolGrantRequestRefusedError

    error = _rpc_error_from_payload(
        {
            "code": -32602,
            "message": "the subject's grant gate does not name the target",
            "data": {
                "kind": "tool_grant_request_refused",
                "tool_grant_request_refusal": {"kind": "target_not_gated"},
            },
        },
        request_id="1",
        method="meta/tool_grants/grant",
    )
    assert isinstance(error, ToolGrantRequestRefusedError)
    assert error.refusal_kind == "target_not_gated"
    assert error.refusal == {"kind": "target_not_gated"}

    plain = _rpc_error_from_payload(
        {"code": -32602, "message": "Invalid params"},
        request_id="2",
        method="meta/tool_grants/grant",
    )
    assert not isinstance(plain, ToolGrantRequestRefusedError)


def test_the_refusal_kinds_are_a_closed_vocabulary():
    from meerkat_meta import TOOL_GRANT_REQUEST_REFUSAL_KINDS

    assert TOOL_GRANT_REQUEST_REFUSAL_KINDS == frozenset(
        {
            "subject_unknown",
            "target_not_gated",
            "server_not_installed",
            "no_live_grant",
            "invalid_wait",
            "invalid_window",
            "empty_target",
            "not_found",
            "invalid_decision_ref",
            "invalid_page_limit",
        }
    )
