"""Meta SDK wrappers of the durable decision verbs (``meta/decisions/*``).

Each wrapper sends its method with exactly the params it names, ``decide``
carries a typed host attestation, and ``decide`` refuses inside a host tool
callback: an agent's tool call must never become the operator's decision.
"""

import pytest
from meerkat_meta.agent_builder import CallbackDispatcher
from meerkat_meta.models import SessionBuildOptions

DECISION_ID = "decision:" + "a" * 64
DIGEST = "sha256:" + "b" * 64


def make_handle():
    from unittest.mock import MagicMock

    from meerkat_meta.runtime import MobHandle

    runtime = MagicMock()
    calls = []

    async def mock_rpc(method, params=None, *, transport_timeout=None):
        calls.append((method, params))
        return {"decision": {"decision_id": DECISION_ID, "phase": "open"}}

    runtime._rpc = mock_rpc
    handle = MobHandle.__new__(MobHandle)
    handle._runtime = runtime
    return handle, calls


@pytest.mark.asyncio
async def test_decision_wrappers_send_the_meta_methods_and_params():
    handle, calls = make_handle()
    await handle.decisions_list(
        kind="release",
        phase="open",
        approver_id="team:owners",
        requester="principal:x",
        after=DECISION_ID,
        limit=10,
    )
    await handle.decisions_list()
    await handle.decisions_get(DECISION_ID)
    await handle.decisions_get(DECISION_ID, include_request=True)
    await handle.decisions_rendering(DECISION_ID, max_part_bytes=16)
    presentation = {
        "audience": {"kind": "approver", "approver_id": "team:owners"},
        "subject_digest": DIGEST,
        "parts_shown": [{"index": 0, "count": 1, "part_digest": DIGEST}],
        "channel_ref": "chat:owners",
        "message_ref": "message-1",
        "at_ms": 1,
    }
    await handle.decisions_present(DECISION_ID, presentation)
    await handle.decisions_withdraw(DECISION_ID)
    await handle.decisions_decide(DECISION_ID, approve=True, presented_digest=DIGEST)
    assert calls == [
        (
            "meta/decisions/list",
            {
                "kind": "release",
                "phase": "open",
                "approver_id": "team:owners",
                "requester": "principal:x",
                "after": DECISION_ID,
                "limit": 10,
            },
        ),
        ("meta/decisions/list", {}),
        ("meta/decisions/get", {"decision_id": DECISION_ID}),
        ("meta/decisions/get", {"decision_id": DECISION_ID, "include_request": True}),
        ("meta/decisions/rendering", {"decision_id": DECISION_ID, "max_part_bytes": 16}),
        ("meta/decisions/present", {"decision_id": DECISION_ID, "presentation": presentation}),
        ("meta/decisions/withdraw", {"decision_id": DECISION_ID}),
        (
            "meta/decisions/decide",
            {"decision_id": DECISION_ID, "approve": True, "presented_digest": DIGEST},
        ),
    ]


@pytest.mark.asyncio
async def test_decide_carries_a_typed_attestation():
    from meerkat_meta import DecisionAttestation

    handle, calls = make_handle()
    await handle.decisions_decide(
        DECISION_ID,
        approve=False,
        presented_digest=DIGEST,
        reason="not now",
        attestation=DecisionAttestation(
            ingress="chat",
            subject="U123",
            scope="chat-session-1",
            event_ref="chat:event-1",
            observed_at_ms=1_700_000_000_000,
        ),
    )
    assert calls == [
        (
            "meta/decisions/decide",
            {
                "decision_id": DECISION_ID,
                "approve": False,
                "presented_digest": DIGEST,
                "reason": "not now",
                "attestation": {
                    "ingress": "chat",
                    "subject": "U123",
                    "scope": "chat-session-1",
                    "event_ref": "chat:event-1",
                    "observed_at_ms": 1_700_000_000_000,
                },
            },
        )
    ]


@pytest.mark.asyncio
async def test_decide_inside_a_tool_callback_is_refused():
    handle, calls = make_handle()
    outcome = {}

    class Builder:
        async def build_agent(self, opts: SessionBuildOptions) -> None:
            opts.profile_name = "test"

            async def approve_it(args):
                try:
                    await handle.decisions_decide(
                        DECISION_ID, approve=True, presented_digest=DIGEST
                    )
                except Exception as error:  # noqa: BLE001 - the refusal is the subject
                    outcome["error"] = error
                return "done"

            opts.register_tool("approve_it", approve_it)

    dispatcher = CallbackDispatcher()
    dispatcher.register_builder(Builder())
    await dispatcher.handle_callback("callback/build_agent", {"options": {"scope_id": "s1"}})
    await dispatcher.handle_callback(
        "callback/call_tool", {"scope_id": "s1", "tool": "approve_it", "arguments": {}}
    )
    error = outcome.get("error")
    assert type(error).__name__ == "DecideInsideToolCallbackError", repr(error)
    assert getattr(error, "kind", None) == "decide_inside_tool_callback"
    assert calls == [], "nothing is sent from inside a tool callback"

    # Outside a callback the host decides as before.
    await handle.decisions_decide(DECISION_ID, approve=True, presented_digest=DIGEST)
    assert [method for method, _ in calls] == ["meta/decisions/decide"]
