"""Run-fenced Stop wrappers (`turn/stop_run`, `mob/stop_member_run`) send the
exact RPC literals with snake_case params and validate the typed receipt
union on its `outcome` discriminator."""

from __future__ import annotations

from types import SimpleNamespace
from typing import Any

import pytest

from meerkat import MeerkatClient
from meerkat.errors import MeerkatError
from meerkat.mob import Mob
from meerkat.session import Session

RUN_ID = "01936f8b-0000-7000-8000-000000000042"


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
async def test_session_stop_run_returns_the_stopped_receipt() -> None:
    receipt = {
        "outcome": "stopped",
        "run_id": RUN_ID,
        "contributors": [
            {"input_id": "i-1", "completion": "cancelled", "terminal": "cancelled"},
            {"input_id": "i-2", "completion": "runtime_terminated", "terminal": "cancelled"},
        ],
    }
    client, calls = fake_client(
        {"turn/stop_run": {"session_id": "s-1", "receipt": receipt}}
    )
    session = Session(client, SimpleNamespace(session_id="s-1", session_ref=None))  # type: ignore[arg-type]

    result = await session.stop_run(RUN_ID, reason="user pressed stop")

    assert calls == [
        (
            "turn/stop_run",
            {"session_id": "s-1", "run_id": RUN_ID, "reason": "user pressed stop"},
        )
    ]
    assert result.session_id == "s-1"
    assert result.receipt == receipt


@pytest.mark.asyncio
async def test_stale_stop_is_a_not_current_receipt_not_an_error() -> None:
    receipt = {"outcome": "not_current", "run_id": RUN_ID, "current_run_id": "r-2"}
    client, _calls = fake_client(
        {"turn/stop_run": {"session_id": "s-1", "receipt": receipt}}
    )
    result = await client._stop_run("s-1", RUN_ID, "late")
    assert result.receipt["outcome"] == "not_current"
    assert result.receipt == receipt


@pytest.mark.asyncio
async def test_not_stoppable_receipt_carries_the_runtime_state() -> None:
    receipt = {"outcome": "not_stoppable", "run_id": RUN_ID, "state": "stopped"}
    client, _calls = fake_client(
        {"turn/stop_run": {"session_id": "s-1", "receipt": receipt}}
    )
    result = await client._stop_run("s-1", RUN_ID, "stop")
    assert result.receipt == receipt


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "receipt",
    [
        {"outcome": "stopped", "run_id": RUN_ID},
        {"outcome": "not_stoppable", "run_id": RUN_ID},
        {"outcome": "mystery", "run_id": RUN_ID},
        {"outcome": "not_current"},
    ],
)
async def test_malformed_receipts_fail_closed(receipt: dict[str, Any]) -> None:
    client, _calls = fake_client(
        {"turn/stop_run": {"session_id": "s-1", "receipt": receipt}}
    )
    with pytest.raises(MeerkatError):
        await client._stop_run("s-1", RUN_ID, "stop")


@pytest.mark.asyncio
async def test_mob_stop_member_run_sends_identity_and_run() -> None:
    receipt = {"outcome": "not_current", "run_id": RUN_ID}
    client, calls = fake_client(
        {
            "mob/stop_member_run": {
                "mob_id": "mob-1",
                "agent_identity": "worker-1",
                "receipt": receipt,
            }
        }
    )
    result = await Mob(client, "mob-1").stop_member_run(
        "worker-1", RUN_ID, reason="stop"
    )
    assert calls == [
        (
            "mob/stop_member_run",
            {
                "mob_id": "mob-1",
                "agent_identity": "worker-1",
                "run_id": RUN_ID,
                "reason": "stop",
            },
        )
    ]
    assert result.mob_id == "mob-1"
    assert result.agent_identity == "worker-1"
    assert result.receipt == receipt
