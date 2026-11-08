"""Compaction request wrappers (`session/compact`, `mob/compact_member`) send
the exact RPC literals with snake_case params and validate the closed
`outcome` vocabulary."""

from __future__ import annotations

from types import SimpleNamespace
from typing import Any

import pytest

from meerkat import MeerkatClient
from meerkat.errors import MeerkatError
from meerkat.mob import Mob
from meerkat.session import Session


def fake_client(
    results: dict[str, dict[str, Any]],
) -> tuple[MeerkatClient, list[tuple[str, dict[str, Any]]]]:
    client = MeerkatClient()
    calls: list[tuple[str, dict[str, Any]]] = []

    async def fake_request(method: str, params: dict[str, Any]) -> dict[str, Any]:
        calls.append((method, params))
        return results[method]

    client._request = fake_request  # type: ignore[assignment]
    return client, calls


@pytest.mark.asyncio
@pytest.mark.parametrize("outcome", ["requested", "already_requested"])
async def test_session_compact_returns_the_outcome(outcome: str) -> None:
    client, calls = fake_client(
        {"session/compact": {"session_id": "s-1", "outcome": outcome}}
    )
    session = Session(client, SimpleNamespace(session_id="s-1", session_ref=None))  # type: ignore[arg-type]

    result = await session.compact()

    assert calls == [("session/compact", {"session_id": "s-1"})]
    assert result.session_id == "s-1"
    assert result.outcome == outcome


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "result",
    [
        {"session_id": "s-1", "outcome": "compacted"},
        {"session_id": "s-1"},
        {"outcome": "requested"},
    ],
)
async def test_malformed_compact_results_fail_closed(result: dict[str, Any]) -> None:
    client, _calls = fake_client({"session/compact": result})
    with pytest.raises(MeerkatError):
        await client._compact("s-1")


@pytest.mark.asyncio
async def test_mob_compact_member_sends_the_identity() -> None:
    client, calls = fake_client(
        {
            "mob/compact_member": {
                "mob_id": "mob-1",
                "agent_identity": "worker-1",
                "outcome": "requested",
            }
        }
    )
    result = await Mob(client, "mob-1").compact_member("worker-1")
    assert calls == [
        (
            "mob/compact_member",
            {"mob_id": "mob-1", "agent_identity": "worker-1"},
        )
    ]
    assert result.mob_id == "mob-1"
    assert result.agent_identity == "worker-1"
    assert result.outcome == "requested"


@pytest.mark.asyncio
async def test_mob_compact_member_refuses_an_unknown_outcome() -> None:
    client, _calls = fake_client(
        {
            "mob/compact_member": {
                "mob_id": "mob-1",
                "agent_identity": "worker-1",
                "outcome": "woken",
            }
        }
    )
    with pytest.raises(MeerkatError):
        await client.compact_mob_member("mob-1", "worker-1")
