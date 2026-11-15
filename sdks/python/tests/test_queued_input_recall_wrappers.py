"""Queued-input recall wrapper (`session/recall_input`) sends the exact RPC
literal with the `session/input_status` selector and validates the typed
answer union on its `outcome` discriminator."""

from __future__ import annotations

from types import SimpleNamespace
from typing import Any

import pytest

from meerkat import MeerkatClient
from meerkat.errors import MeerkatError, meerkat_error_from_jsonrpc_code
from meerkat.session import DeferredSession, Session

INPUT_ID = "01936f8b-0000-7000-8000-0000000000a1"
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


def session_over(client: MeerkatClient) -> Session:
    return Session(client, SimpleNamespace(session_id="s-1", session_ref=None))  # type: ignore[arg-type]


@pytest.mark.asyncio
async def test_recall_by_input_id_returns_the_abandoned_answer() -> None:
    recall = {"outcome": "abandoned", "input_id": INPUT_ID, "attempt_count": 0}
    client, calls = fake_client(
        {"session/recall_input": {"session_id": "s-1", "recall": recall}}
    )

    result = await session_over(client).recall_input(input_id=INPUT_ID)

    assert calls == [
        (
            "session/recall_input",
            {"session_id": "s-1", "selector": {"by": "input_id", "value": INPUT_ID}},
        )
    ]
    assert result.session_id == "s-1"
    assert result.recall == recall


@pytest.mark.asyncio
async def test_recall_by_idempotency_key_sends_the_key_selector() -> None:
    recall = {"outcome": "already_terminal", "input_id": INPUT_ID, "terminal": "cancelled"}
    client, calls = fake_client(
        {"session/recall_input": {"session_id": "s-1", "recall": recall}}
    )

    result = await session_over(client).recall_input(idempotency_key="inbox:msg-1")

    assert calls == [
        (
            "session/recall_input",
            {
                "session_id": "s-1",
                "selector": {"by": "idempotency_key", "value": "inbox:msg-1"},
            },
        )
    ]
    assert result.recall["outcome"] == "already_terminal"
    assert result.recall["terminal"] == "cancelled"


@pytest.mark.asyncio
async def test_not_queued_carries_the_phase_and_the_run() -> None:
    recall = {
        "outcome": "not_queued",
        "input_id": INPUT_ID,
        "phase": "staged",
        "run_id": RUN_ID,
    }
    client, _calls = fake_client(
        {"session/recall_input": {"session_id": "s-1", "recall": recall}}
    )

    result = await session_over(client).recall_input(input_id=INPUT_ID)

    assert result.recall == recall


@pytest.mark.asyncio
async def test_an_abandoned_answer_without_an_attempt_count_reads_zero() -> None:
    client, _calls = fake_client(
        {
            "session/recall_input": {
                "session_id": "s-1",
                "recall": {"outcome": "abandoned", "input_id": INPUT_ID},
            }
        }
    )

    result = await session_over(client).recall_input(input_id=INPUT_ID)

    assert result.recall["attempt_count"] == 0


@pytest.mark.asyncio
async def test_the_selector_is_exactly_one_of_input_id_and_key() -> None:
    client, calls = fake_client({})
    session = session_over(client)
    with pytest.raises(ValueError):
        await session.recall_input()
    with pytest.raises(ValueError):
        await session.recall_input(input_id=INPUT_ID, idempotency_key="inbox:msg-1")
    assert calls == []


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "result",
    [
        {"session_id": "s-1"},
        {"recall": {"outcome": "abandoned", "input_id": INPUT_ID}},
        {"session_id": "s-1", "recall": "abandoned"},
        {"session_id": "s-1", "recall": {"outcome": "abandoned"}},
        {"session_id": "s-1", "recall": {"outcome": 7, "input_id": INPUT_ID}},
        {"session_id": "s-1", "recall": {"outcome": "", "input_id": INPUT_ID}},
        {"session_id": "s-1", "recall": {"outcome": "not_queued", "input_id": INPUT_ID}},
        {
            "session_id": "s-1",
            "recall": {"outcome": "not_queued", "input_id": INPUT_ID, "phase": "staged", "run_id": 7},
        },
        {"session_id": "s-1", "recall": {"outcome": "already_terminal", "input_id": INPUT_ID}},
        {
            "session_id": "s-1",
            "recall": {"outcome": "abandoned", "input_id": INPUT_ID, "attempt_count": "0"},
        },
        {
            "session_id": "s-1",
            "recall": {"outcome": "abandoned", "input_id": INPUT_ID, "attempt_count": -1},
        },
    ],
)
async def test_malformed_recall_results_fail_closed(result: dict[str, Any]) -> None:
    client, _calls = fake_client({"session/recall_input": result})
    with pytest.raises(MeerkatError):
        await client._recall_input("s-1", input_id=INPUT_ID)


@pytest.mark.asyncio
async def test_an_outcome_this_sdk_does_not_know_is_kept_as_it_is() -> None:
    # The list of outcomes can grow: an outcome this SDK does not know is
    # returned unchanged, with every field the server sent, and raises nothing.
    recall = {"outcome": "kept_for_the_next_start", "input_id": INPUT_ID, "until": "later"}
    client, _calls = fake_client(
        {"session/recall_input": {"session_id": "s-1", "recall": recall}}
    )

    result = await session_over(client).recall_input(input_id=INPUT_ID)

    assert result.recall == recall


def test_the_deferred_session_exposes_the_same_wrapper() -> None:
    assert callable(getattr(DeferredSession, "recall_input", None))


@pytest.mark.asyncio
async def test_a_server_without_the_method_surfaces_method_not_found() -> None:
    # A server that does not serve the method answers method not found
    # (-32601). The wrapper sends the one call, reads nothing of the server
    # first, and the transport's error reaches the caller unchanged.
    client = MeerkatClient()
    calls: list[tuple[str, dict[str, Any]]] = []

    async def refusing_request(method: str, params: dict[str, Any]) -> dict[str, Any]:
        calls.append((method, params))
        raise meerkat_error_from_jsonrpc_code(
            -32601, None, "Method not found: session/recall_input"
        )

    client._request = refusing_request  # type: ignore[assignment]

    with pytest.raises(MeerkatError) as raised:
        await session_over(client).recall_input(input_id=INPUT_ID)

    assert raised.value.code == "-32601"
    assert raised.value.message == "Method not found: session/recall_input"
    assert [method for method, _ in calls] == ["session/recall_input"]
