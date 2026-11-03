"""A keyed send: ``send(..., idempotency_key=...)`` and ``send_status``.

The gateway commits a keyed send as a member continuation under the key and
answers the continuation's receipt with its delivery status; a replay answers
the first receipt. A gateway that predates keyed sends would ignore the field
and send unkeyed, so the SDK refuses a key, typed, unless the gateway
advertises ``meta/send_status``, the read that ships with keyed sends. A
keyed steer (``handling_mode="steer"``) also needs the
``keyed_send_handling_mode`` feature capability: a gateway without it would
ignore the mode and queue.
"""
import asyncio

import pytest

from meerkat_meta import (
    CapabilityUnavailableError,
    ContinuationStatus,
    InputState,
    KeyedSendResult,
    SendStatusResult,
)
from meerkat_meta.runtime import MetaRuntime

from .test_identity_first_turn_tickets import _make_runtime

RECEIPT = {
    "delivery_id": "delivery-1",
    "committed_at_ms": 1_761_000_000_000,
    "status": {"state": "admitted", "session_id": "session-1", "input_id": "input-1"},
}


class KeyedTransport:
    """Answers like a gateway that serves keyed sends when ``keyed`` is set,
    and like one that predates them otherwise."""

    def __init__(self, *, keyed: bool, steer: bool = False):
        self.calls: list[dict] = []
        self.request_timeout = 60.0
        self.keyed = keyed
        self.steer = steer

    def sent(self, method: str) -> list[dict]:
        return [call["params"] for call in self.calls if call.get("method") == method]

    async def send_async(self, request, *, timeout=None):
        await asyncio.sleep(0)
        self.calls.append(request)
        method = request.get("method")
        params = request.get("params") or {}
        if method == "meta/capabilities":
            methods = ["meta/send", "meta/status_identity"]
            if self.keyed:
                methods.append("meta/send_status")
            features = ["keyed_send_handling_mode"] if self.steer else []
            result = {
                "contract_version": "0.5.0",
                "methods": methods,
                "feature_capabilities": features,
                "loaded_modules": [],
            }
        elif method == "meta/send":
            result = {"continuation": RECEIPT} if "idempotency_key" in params else {"fencing_token": 1}
        elif method == "meta/send_status":
            if params["input_id"] == "unknown":
                result = {}
            else:
                result = {
                    "state": {
                        "input_id": params["input_id"],
                        "current_state": "consumed",
                        "terminal_outcome": "completed",
                        "attempt_count": 1,
                        "recovery_count": 0,
                        "created_at": "2026-10-23T03:00:00Z",
                        "updated_at": "2026-10-23T03:00:01Z",
                    }
                }
        else:
            result = {}
        return {"jsonrpc": "2.0", "id": request.get("id"), "result": result}

    def is_running(self):
        return True

    def start(self):
        pass

    def stop(self):
        pass

    def set_callback_handler(self, handler):
        pass


@pytest.mark.asyncio
async def test_a_keyed_send_forwards_the_key_and_returns_the_receipt():
    transport = KeyedTransport(keyed=True)
    runtime: MetaRuntime = _make_runtime(transport)

    sent = await runtime.send("keeper", "Ingest item 1.", idempotency_key="item-1")

    assert isinstance(sent, KeyedSendResult)
    assert sent.delivery_id == "delivery-1"
    assert sent.committed_at_ms == 1_761_000_000_000
    assert sent.status == ContinuationStatus(
        state="admitted", session_id="session-1", input_id="input-1"
    )
    assert transport.sent("meta/send") == [
        {"identity": "keeper", "content": "Ingest item 1.", "idempotency_key": "item-1"}
    ]


@pytest.mark.asyncio
async def test_the_agent_handle_forwards_the_key_and_the_status_read():
    transport = KeyedTransport(keyed=True)
    runtime: MetaRuntime = _make_runtime(transport)
    agent = runtime.agent("keeper")

    sent = await agent.send("Ingest item 2.", idempotency_key="item-2")
    read = await agent.send_status(sent.status.session_id, sent.status.input_id)

    assert sent.delivery_id == "delivery-1"
    assert read.state is not None and read.state.input_id == "input-1"
    assert transport.sent("meta/send_status") == [
        {"identity": "keeper", "session_id": "session-1", "input_id": "input-1"}
    ]


@pytest.mark.asyncio
async def test_a_gateway_that_does_not_serve_keyed_sends_refuses_the_key_typed():
    transport = KeyedTransport(keyed=False)
    runtime: MetaRuntime = _make_runtime(transport)

    with pytest.raises(CapabilityUnavailableError) as refused:
        await runtime.send("keeper", "Ingest item 3.", idempotency_key="item-3")

    assert refused.value.method == "meta/send"
    assert transport.sent("meta/send") == [], "nothing is sent unkeyed"
    # An unkeyed send needs no capability read.
    await runtime.send("keeper", "Ingest item 3.")
    assert transport.sent("meta/send") == [{"identity": "keeper", "content": "Ingest item 3."}]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "extra", [{"track_turn": True}, {"expected_spec_digest": "sha256:aaaa"}]
)
async def test_a_key_does_not_combine_with_a_tracked_turn_or_an_authored_row(extra):
    transport = KeyedTransport(keyed=True)
    runtime: MetaRuntime = _make_runtime(transport)

    with pytest.raises(ValueError, match="idempotency_key"):
        await runtime.send("keeper", "Ingest item 4.", idempotency_key="item-4", **extra)

    assert transport.calls == [], "refused before any call"


@pytest.mark.asyncio
async def test_send_status_reads_the_input_state_and_an_unknown_input_reads_as_none():
    transport = KeyedTransport(keyed=True)
    runtime: MetaRuntime = _make_runtime(transport)

    read = await runtime.send_status("keeper", "session-1", "input-1")
    unknown = await runtime.send_status("keeper", "session-1", "unknown")

    assert isinstance(read, SendStatusResult)
    assert isinstance(read.state, InputState)
    assert read.state.input_id == "input-1"
    assert read.state.current_state == "consumed"
    assert read.state.terminal_outcome == "completed"
    assert read.state.raw["attempt_count"] == 1
    assert unknown.state is None


def test_continuation_status_reads_every_state_and_tolerates_new_ones():
    assert ContinuationStatus.from_dict({"state": "pending"}) == ContinuationStatus(state="pending")
    assert ContinuationStatus.from_dict({"state": "stranded"}).state == "stranded"
    refused = ContinuationStatus.from_dict({"state": "refused", "reason": "member_retired"})
    assert refused.reason == "member_retired"
    assert ContinuationStatus.from_dict({"state": "later"}).state == "later"
    assert KeyedSendResult.from_dict({"continuation": RECEIPT}).to_dict() == {
        "continuation": RECEIPT
    }


@pytest.mark.asyncio
async def test_a_keyed_steer_forwards_its_handling_mode():
    transport = KeyedTransport(keyed=True, steer=True)
    runtime: MetaRuntime = _make_runtime(transport)

    sent = await runtime.send(
        "keeper", "Check the ledger too.", idempotency_key="steer-1", handling_mode="steer"
    )
    await runtime.agent("keeper").send(
        "Check the ledger too.", idempotency_key="steer-1", handling_mode="steer"
    )

    assert isinstance(sent, KeyedSendResult)
    assert sent.delivery_id == "delivery-1"
    assert transport.sent("meta/send") == [
        {
            "identity": "keeper",
            "content": "Check the ledger too.",
            "idempotency_key": "steer-1",
            "handling_mode": "steer",
        }
    ] * 2


@pytest.mark.asyncio
async def test_a_gateway_without_the_keyed_mode_refuses_a_steer_typed_and_still_queues():
    transport = KeyedTransport(keyed=True, steer=False)
    runtime: MetaRuntime = _make_runtime(transport)

    with pytest.raises(CapabilityUnavailableError) as refused:
        await runtime.send(
            "keeper", "Check the ledger too.", idempotency_key="steer-2", handling_mode="steer"
        )

    assert refused.value.method == "meta/send"
    assert refused.value.data == {"capability": "keyed_send_handling_mode"}
    assert transport.sent("meta/send") == [], "nothing is sent as a queued send"
    # A keyed queue needs no mode capability: such a gateway queues.
    await runtime.send(
        "keeper", "Check the ledger too.", idempotency_key="queue-2", handling_mode="queue"
    )
    assert transport.sent("meta/send") == [
        {
            "identity": "keeper",
            "content": "Check the ledger too.",
            "idempotency_key": "queue-2",
            "handling_mode": "queue",
        }
    ]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("extra", "match"),
    [
        ({"handling_mode": "steer"}, "idempotency_key"),
        ({"handling_mode": "sideways"}, "handling_mode"),
        ({"handling_mode": "sideways", "idempotency_key": "steer-3"}, "handling_mode"),
    ],
)
async def test_an_unkeyed_steer_or_an_unknown_mode_is_refused_before_any_call(extra, match):
    transport = KeyedTransport(keyed=True, steer=True)
    runtime: MetaRuntime = _make_runtime(transport)

    with pytest.raises(ValueError, match=match):
        await runtime.send("keeper", "Check the ledger too.", **extra)

    assert transport.calls == [], "refused before any call"


@pytest.mark.asyncio
async def test_an_unkeyed_queue_is_the_default_and_sends_as_before():
    transport = KeyedTransport(keyed=True, steer=False)
    runtime: MetaRuntime = _make_runtime(transport)

    await runtime.send("keeper", "File the report.", handling_mode="queue")
    await runtime.agent("keeper").send("File the report.", handling_mode="queue")

    assert transport.sent("meta/send") == [
        {"identity": "keeper", "content": "File the report."}
    ] * 2, "an unkeyed queue is the default send"
    assert transport.sent("meta/capabilities") == [], "an unkeyed send needs no capability"
