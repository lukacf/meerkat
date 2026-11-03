"""A quarantined persisted mob: ``mob_quarantined``, ``list_quarantined_mobs``
and ``mob_restore_retry``.

A persisted mob whose store did not restore is quarantined: ``mob/list``
names it under ``quarantined``, and every call that addresses its id fails
with ``error.data.kind`` ``"mob_quarantined"`` (``MobError::MobQuarantined``
in crates/meerkat-mob/src/error.rs). ``mob/restore_retry`` re-attempts its
restore.
"""

from __future__ import annotations

import asyncio
import json
from typing import Any

import pytest

from meerkat import MeerkatClient
from meerkat.errors import MeerkatError
from meerkat.streaming import _StdoutDispatcher

# The refusal's data, as a server sends it for a store that could not serve.
_REFUSAL: dict[str, Any] = {
    "kind": "mob_quarantined",
    "code": "SESSION_RUNTIME_UNAVAILABLE",
    "mob_id": "broken",
    "class": "store_unavailable",
    "retryable": True,
    "content_retained": True,
    "cause": "session catalog unavailable",
}

# One `mob/list` quarantined row, as a server sends it.
_ROW: dict[str, Any] = {
    "mob_id": "broken",
    "class": "store_unreadable",
    "retryable": False,
    "retrying": False,
    "attempts": 1,
    "message": "mob 'broken' is quarantined (store_unreadable): file is not a database",
    "code": "INTERNAL_ERROR",
    "structured_data": {"kind": "mob_quarantined", "class": "store_unreadable"},
}


async def _rpc_error(data: dict[str, Any]) -> MeerkatError:
    """The error the client raises for a JSON-RPC error response carrying ``data``."""
    response = {
        "jsonrpc": "2.0",
        "id": 1,
        "error": {
            "code": -32603,
            "message": "mob 'broken' is quarantined (store_unavailable): session catalog unavailable",
            "data": data,
        },
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
    return exc_info.value


@pytest.mark.asyncio
async def test_the_quarantine_refusal_survives_the_rpc_transport_typed() -> None:
    from meerkat import MOB_QUARANTINED_KIND, MobQuarantinedRefusal, mob_quarantined

    error = await _rpc_error(_REFUSAL)
    assert MOB_QUARANTINED_KIND == "mob_quarantined"
    assert mob_quarantined(error) == MobQuarantinedRefusal(
        mob_id="broken",
        quarantine_class="store_unavailable",
        retryable=True,
        cause="session catalog unavailable",
        code="SESSION_RUNTIME_UNAVAILABLE",
    )
    assert error.details == _REFUSAL, "details stay as sent"


def test_a_class_this_sdk_does_not_know_is_kept_as_sent() -> None:
    from meerkat import MOB_QUARANTINE_CLASSES, mob_quarantined

    assert MOB_QUARANTINE_CLASSES == frozenset(
        {"store_unavailable", "store_unreadable", "resume_refused"}
    )
    data = {**_REFUSAL, "class": "store_migrating", "retryable": False}
    refusal = mob_quarantined(MeerkatError("INTERNAL_ERROR", "quarantined", data))
    assert refusal is not None
    assert refusal.quarantine_class == "store_migrating"
    assert refusal.retryable is False


def test_any_other_error_is_no_quarantine() -> None:
    from meerkat import mob_quarantined

    assert mob_quarantined(ValueError("not a meerkat error")) is None
    held = {"kind": "mob_held_before_activation", "mob_id": "child"}
    assert mob_quarantined(MeerkatError("SESSION_RUNTIME_UNAVAILABLE", "held", held)) is None
    malformed = {**_REFUSAL, "retryable": "yes"}
    assert mob_quarantined(MeerkatError("INTERNAL_ERROR", "x", malformed)) is None


@pytest.mark.asyncio
async def test_list_quarantined_mobs_reads_the_quarantined_rows() -> None:
    client = MeerkatClient()
    calls: list[str] = []

    async def fake_request(method, _params):
        calls.append(method)
        return {"mobs": [{"mob_id": "mob-1", "status": "Running"}], "quarantined": [_ROW]}

    client._request = fake_request  # type: ignore[method-assign]
    client.require_capability = lambda _cap: None  # type: ignore[method-assign]

    assert await client.list_quarantined_mobs() == [_ROW]
    assert await client.list_mobs() == [{"mob_id": "mob-1", "status": "Running"}]
    assert calls == ["mob/list", "mob/list"]


@pytest.mark.asyncio
async def test_list_quarantined_mobs_is_empty_when_the_answer_names_none() -> None:
    client = MeerkatClient()

    async def fake_request(_method, _params):
        return {"mobs": []}

    client._request = fake_request  # type: ignore[method-assign]
    client.require_capability = lambda _cap: None  # type: ignore[method-assign]

    assert await client.list_quarantined_mobs() == []


@pytest.mark.asyncio
async def test_list_quarantined_mobs_rejects_a_malformed_row() -> None:
    client = MeerkatClient()

    async def fake_request(_method, _params):
        return {"mobs": [], "quarantined": [{"mob_id": "broken"}]}

    client._request = fake_request  # type: ignore[method-assign]
    client.require_capability = lambda _cap: None  # type: ignore[method-assign]

    with pytest.raises(MeerkatError) as excinfo:
        await client.list_quarantined_mobs()
    assert excinfo.value.code == "INVALID_RESPONSE"


@pytest.mark.asyncio
async def test_mob_restore_retry_sends_the_mob_id_and_answers_the_outcome() -> None:
    client = MeerkatClient()
    sent: list[tuple[str, dict[str, Any]]] = []

    async def fake_request(method, params):
        sent.append((method, params))
        return {"mob_id": "broken", "outcome": "restored"}

    client._request = fake_request  # type: ignore[method-assign]

    assert await client.mob_restore_retry("broken") == {
        "mob_id": "broken",
        "outcome": "restored",
    }
    assert sent == [("mob/restore_retry", {"mob_id": "broken"})]
