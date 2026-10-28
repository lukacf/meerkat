"""Typed ``comms/send`` error data: ``CommsSendErrorData`` and ``comms_send_error``.

The codes come from ``CommsSendErrorData`` in artifacts/schemas/wire-types.json,
emitted from the Rust contract (crates/meerkat-contracts/src/wire/comms.rs).
"""

from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import Any

import pytest

from meerkat.errors import MeerkatError
from meerkat.streaming import _StdoutDispatcher

_WIRE_TYPES = (
    Path(__file__).resolve().parents[3] / "artifacts" / "schemas" / "wire-types.json"
)
_SCHEMA = json.loads(_WIRE_TYPES.read_text(encoding="utf-8"))["CommsSendErrorData"]
_WIRE_CODES = {arm["properties"]["code"]["const"] for arm in _SCHEMA["oneOf"]}


def test_comms_send_error_codes_are_the_wire_vocabulary() -> None:
    from meerkat import COMMS_SEND_ERROR_CODES

    assert COMMS_SEND_ERROR_CODES == _WIRE_CODES


@pytest.mark.parametrize("code", sorted(_WIRE_CODES))
def test_comms_send_error_returns_the_typed_data(code: str) -> None:
    from meerkat import comms_send_error

    data = {"code": code, "message": "comms/send failed"}

    assert comms_send_error(MeerkatError("INTERNAL_ERROR", "failed", data)) == data


@pytest.mark.parametrize(
    "details",
    [None, "send_failed", {"message": "no code"}, {"code": 1}, {"code": "not_a_code"}],
    ids=lambda value: json.dumps(value),
)
def test_comms_send_error_is_none_without_a_known_code(details: Any) -> None:
    from meerkat import comms_send_error

    assert comms_send_error(MeerkatError("INTERNAL_ERROR", "failed", details)) is None


def test_comms_send_error_is_none_for_other_exceptions() -> None:
    from meerkat import comms_send_error

    assert comms_send_error(ValueError("not a meerkat error")) is None


_ENVELOPE_ID = "00000000-0000-4000-8000-000000000001"
_AMBIGUOUS_MESSAGE = (
    f"delivery outcome for envelope {_ENVELOPE_ID} is unknown; it may already be "
    "delivered. Reconcile against durable work truth before any new attempt - a "
    "retry can duplicate. (ack timed out)"
)

# The JSON-RPC error responses `comms/send` returns
# (crates/meerkat-rpc/src/handlers/comms.rs): the typed data is `error.data`.
_WIRE_FAILURES = [
    pytest.param(
        -32603,
        "peer 'alice' is unreachable: transport_error",
        {
            "code": "peer_unreachable",
            "peer": "alice",
            "reason": "transport_error",
            "message": "peer 'alice' is unreachable: transport_error",
            "details": "connection refused",
        },
        id="peer_unreachable-transport_error",
    ),
    pytest.param(
        -32603,
        "peer 'alice' is unreachable: offline_or_no_ack",
        {
            "code": "peer_unreachable",
            "peer": "alice",
            "reason": "offline_or_no_ack",
            "message": "peer 'alice' is unreachable: offline_or_no_ack",
        },
        id="peer_unreachable-offline_or_no_ack",
    ),
    pytest.param(
        -32603,
        "peer 'alice' rejected envelope at ingress: inbox_full",
        {
            "code": "peer_admission_dropped",
            "peer": "alice",
            "reason": "inbox_full",
            "message": "peer 'alice' rejected envelope at ingress: inbox_full",
        },
        id="peer_admission_dropped",
    ),
    pytest.param(
        -32603,
        f"peer 'alice' was not woken for envelope {_ENVELOPE_ID} (mob_not_running)",
        {
            "code": "peer_wake_refused",
            "peer": "alice",
            "envelope_id": _ENVELOPE_ID,
            "refusal": {"kind": "mob_not_running"},
            "retryable": True,
            "message": f"peer 'alice' was not woken for envelope {_ENVELOPE_ID} (mob_not_running)",
        },
        id="peer_wake_refused-mob_not_running",
    ),
    pytest.param(
        -32603,
        f"peer 'alice' was not woken for envelope {_ENVELOPE_ID} (member_broken)",
        {
            "code": "peer_wake_refused",
            "peer": "alice",
            "envelope_id": _ENVELOPE_ID,
            "refusal": {
                "kind": "member_broken",
                "reason": "missing durable session snapshot",
                "hold": "tail_held_for_recovery",
            },
            "retryable": False,
            "message": f"peer 'alice' was not woken for envelope {_ENVELOPE_ID} (member_broken)",
        },
        id="peer_wake_refused-member_broken",
    ),
    pytest.param(
        -32603,
        _AMBIGUOUS_MESSAGE,
        {
            "code": "send_ambiguous",
            "envelope_id": _ENVELOPE_ID,
            "message": _AMBIGUOUS_MESSAGE,
            "retry_safe": False,
            "required_action": "reconcile",
        },
        id="send_ambiguous",
    ),
    pytest.param(
        -32602,
        "Command validation failed",
        {"code": "invalid_command", "message": "body must not be empty"},
        id="invalid_command",
    ),
]


@pytest.mark.asyncio
@pytest.mark.parametrize(("rpc_code", "rpc_message", "data"), _WIRE_FAILURES)
async def test_comms_send_error_survives_the_rpc_transport(
    rpc_code: int, rpc_message: str, data: dict[str, Any]
) -> None:
    from meerkat import comms_send_error

    response = {
        "jsonrpc": "2.0",
        "id": 1,
        "error": {"code": rpc_code, "message": rpc_message, "data": data},
    }
    reader = asyncio.StreamReader()
    reader.feed_data((json.dumps(response) + "\n").encode())
    reader.feed_eof()
    dispatcher = _StdoutDispatcher(reader)
    dispatcher.start()
    try:
        with pytest.raises(MeerkatError) as exc_info:
            await asyncio.wait_for(dispatcher.expect_response(1), timeout=1.0)
    finally:
        await dispatcher.stop()

    assert exc_info.value.code == data["code"]
    assert comms_send_error(exc_info.value) == data


def test_peer_wake_refusal_kinds_are_the_wire_vocabulary() -> None:
    """The refusal carried by ``peer_wake_refused`` is a closed vocabulary.

    The kinds come from ``PeerWakeRefusal`` in wire-types.json; ``sender_waking``
    and ``materialization_lost`` are among them, and a peer that is not a member
    is never a wake refusal (it stays ``peer_not_found_or_not_trusted``).
    """
    arm = next(
        arm
        for arm in _SCHEMA["oneOf"]
        if arm["properties"]["code"]["const"] == "peer_wake_refused"
    )
    assert set(arm["required"]) >= {"peer", "envelope_id", "refusal", "retryable", "message"}
    refusal = _SCHEMA["$defs"]["PeerWakeRefusal"]
    kinds = {variant["properties"]["kind"]["const"] for variant in refusal["oneOf"]}
    assert kinds == {
        "member_broken",
        "member_retiring",
        "mob_not_running",
        "admission_closed",
        "wake_timed_out",
        "mob_unavailable",
        "materialization_lost",
        "non_authoritative_composition",
        "host_preparation_pending",
        "resume_gate_unavailable",
        "host_owned",
        "lifecycle_origin_fenced",
        "sender_waking",
    }
