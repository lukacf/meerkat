"""Durable decision wrappers (`decision/*`) send the exact RPC literals with
snake_case params and pass the server's answer through."""

from __future__ import annotations

from typing import Any

import pytest

from meerkat import MeerkatClient

DECISION_ID = "decision:" + "fe421f09" * 8
SUBJECT_DIGEST = "sha256:" + "55c20681" * 8

DECISION = {
    "decision_id": DECISION_ID,
    "kind": "release",
    "subject_key": "build-7",
    "operation_id": "release:build-7",
    "request_digest": "sha256:" + "7c02a5ba" * 8,
    "requester": 'principal:["personal_agent","example.org","builder"]',
    "approver_id": "team:owners",
    "phase": "open",
    "stage": "human",
    "notice": "not_due",
    "deadline_at_ms": 1_761_739_200_000,
    "created_at_ms": 1_761_735_600_000,
    "subject_digest": SUBJECT_DIGEST,
    "renderer": "render.v1",
    "permit_issued": False,
    "revision": 1,
}

PRESENTATION = {
    "audience": {"kind": "approver", "approver_id": "team:owners"},
    "subject_digest": SUBJECT_DIGEST,
    "parts_shown": [{"index": 0, "count": 1, "part_digest": SUBJECT_DIGEST}],
    "channel_ref": "chat:owners",
    "message_ref": "message-1",
    "at_ms": 2_000,
}


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
async def test_list_decisions_sends_only_the_filters_it_was_given() -> None:
    listing = {"decisions": [DECISION], "unreadable": [], "next_after": DECISION_ID}
    client, calls = fake_client({"decision/list": listing})

    assert await client.list_decisions() == listing
    result = await client.list_decisions(
        kind="release",
        phase="open",
        approver_id="team:owners",
        requester=DECISION["requester"],
        after=DECISION_ID,
        limit=50,
    )

    assert calls == [
        ("decision/list", {}),
        (
            "decision/list",
            {
                "kind": "release",
                "phase": "open",
                "approver_id": "team:owners",
                "requester": DECISION["requester"],
                "after": DECISION_ID,
                "limit": 50,
            },
        ),
    ]
    assert result == listing


@pytest.mark.asyncio
async def test_get_decision_asks_for_the_request_only_when_told() -> None:
    client, calls = fake_client({"decision/get": {"decision": DECISION}})

    assert await client.get_decision(DECISION_ID) == {"decision": DECISION}
    await client.get_decision(DECISION_ID, include_request=True)

    assert calls == [
        ("decision/get", {"decision_id": DECISION_ID}),
        ("decision/get", {"decision_id": DECISION_ID, "include_request": True}),
    ]


@pytest.mark.asyncio
async def test_decision_rendering_sends_its_part_size() -> None:
    rendering = {
        "decision_id": DECISION_ID,
        "rendering": "Release build 7 to production.",
        "renderer": "render.v1",
        "subject_digest": SUBJECT_DIGEST,
        "max_part_bytes": 16,
        "parts": [{"index": 0, "count": 2, "part_digest": SUBJECT_DIGEST}],
    }
    client, calls = fake_client({"decision/rendering": rendering})

    await client.decision_rendering(DECISION_ID)
    result = await client.decision_rendering(DECISION_ID, max_part_bytes=16)

    assert calls == [
        ("decision/rendering", {"decision_id": DECISION_ID}),
        ("decision/rendering", {"decision_id": DECISION_ID, "max_part_bytes": 16}),
    ]
    assert result == rendering


@pytest.mark.asyncio
async def test_present_decision_sends_the_presentation_unchanged() -> None:
    presented = dict(DECISION, first_presented_to_approver_at_ms=2_000)
    client, calls = fake_client({"decision/present": {"decision": presented}})

    result = await client.present_decision(DECISION_ID, PRESENTATION)

    assert calls == [
        (
            "decision/present",
            {"decision_id": DECISION_ID, "presentation": PRESENTATION},
        )
    ]
    assert result == {"decision": presented}


@pytest.mark.asyncio
async def test_withdraw_decision_names_only_the_decision() -> None:
    withdrawn = dict(DECISION, phase="withdrawn")
    client, calls = fake_client({"decision/withdraw": {"decision": withdrawn}})

    result = await client.withdraw_decision(DECISION_ID)

    assert calls == [("decision/withdraw", {"decision_id": DECISION_ID})]
    assert result == {"decision": withdrawn}
