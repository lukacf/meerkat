"""Scoped tool grant wrappers (`tool_grants/*`) send the exact RPC literals
with snake_case params, and `grant_gated_tools` reaches `session/create` only
on a server whose contract takes it."""

from __future__ import annotations

from typing import Any
from unittest.mock import MagicMock

import pytest

from meerkat import MeerkatClient
from meerkat.errors import CapabilityUnavailableError

SESSION_ID = "019a2b3c-0000-7000-8000-000000000001"
GRANT_ID = "0b6f9f5e-8f3d-4a51-9a52-3c1f6d1e2a10"
SUBJECT = {"kind": "session", "session_id": SESSION_ID}

GRANT = {
    "grant_id": GRANT_ID,
    "subject": SUBJECT,
    "target": {"kind": "family", "family": "shell", "names": ["shell"]},
    "issuer": {"kind": "operator"},
    "status": "active",
    "issued_at_ms": 1_000,
    "expires_at_ms": 61_000,
}

RUN_RESULT = {
    "session_id": SESSION_ID,
    "text": "ok",
    "turns": 1,
    "tool_calls": 0,
    "usage": {"input_tokens": 1, "output_tokens": 1},
}


def fake_client(
    results: dict[str, dict[str, Any]],
    contract_version: str = "0.9.8",
) -> tuple[MeerkatClient, list[tuple[str, dict[str, Any]]]]:
    client = MeerkatClient()
    client._server_contract_version = contract_version
    calls: list[tuple[str, dict[str, Any]]] = []

    async def fake_request(method: str, params: dict[str, Any]) -> dict[str, Any]:
        calls.append((method, params))
        return results[method]

    client._request = fake_request  # type: ignore[assignment]
    return client, calls


@pytest.mark.asyncio
async def test_grant_tool_sends_a_family_target_and_its_window() -> None:
    client, calls = fake_client({"tool_grants/grant": {"grant": GRANT}})

    result = await client.grant_tool(
        SESSION_ID, family="shell", duration_ms=60_000, reason="incident"
    )

    assert calls == [
        (
            "tool_grants/grant",
            {
                "subject": SUBJECT,
                "target": {"kind": "family", "family": "shell"},
                "duration_ms": 60_000,
                "reason": "incident",
            },
        )
    ]
    assert result == {"grant": GRANT}


@pytest.mark.asyncio
async def test_grant_tool_sends_a_tool_target_with_an_absolute_deadline() -> None:
    client, calls = fake_client({"tool_grants/grant": {"grant": GRANT}})

    await client.grant_tool(SESSION_ID, tool="deploy", expires_at_ms=61_000)

    assert calls == [
        (
            "tool_grants/grant",
            {
                "subject": SUBJECT,
                "target": {"kind": "tool", "name": "deploy"},
                "expires_at_ms": 61_000,
            },
        )
    ]


@pytest.mark.asyncio
async def test_grant_tool_needs_exactly_one_of_tool_and_family() -> None:
    client, calls = fake_client({})
    with pytest.raises(ValueError):
        await client.grant_tool(SESSION_ID, duration_ms=1)
    with pytest.raises(ValueError):
        await client.grant_tool(
            SESSION_ID, tool="deploy", family="shell", duration_ms=1
        )
    assert calls == []


@pytest.mark.asyncio
async def test_grant_tool_needs_exactly_one_of_duration_and_deadline() -> None:
    client, calls = fake_client({})
    with pytest.raises(ValueError):
        await client.grant_tool(SESSION_ID, family="shell")
    with pytest.raises(ValueError):
        await client.grant_tool(
            SESSION_ID, family="shell", duration_ms=1, expires_at_ms=61_000
        )
    assert calls == []


@pytest.mark.asyncio
async def test_revoke_list_and_audit_send_their_params() -> None:
    client, calls = fake_client(
        {
            "tool_grants/revoke": {"grant": GRANT, "changed": True},
            "tool_grants/list": {"grants": [GRANT]},
            "tool_grants/audit": {"rows": [], "next_sequence": 4},
        }
    )

    revoked = await client.revoke_tool_grant(GRANT_ID, reason="done")
    listed = await client.list_tool_grants(SESSION_ID, include_ended=True)
    page = await client.tool_grant_audit(SESSION_ID, after_sequence=2, limit=10)

    assert calls == [
        ("tool_grants/revoke", {"grant_id": GRANT_ID, "reason": "done"}),
        ("tool_grants/list", {"subject": SUBJECT, "include_ended": True}),
        (
            "tool_grants/audit",
            {"subject": SUBJECT, "after_sequence": 2, "limit": 10},
        ),
    ]
    assert revoked["changed"] is True
    assert listed["grants"] == [GRANT]
    assert page["next_sequence"] == 4


@pytest.mark.asyncio
async def test_create_session_sends_its_grant_gate() -> None:
    client, calls = fake_client({"session/create": RUN_RESULT})

    await client.create_session(
        "hello", grant_gated_tools={"families": ["shell"], "tools": ["deploy"]}
    )

    method, params = calls[0]
    assert method == "session/create"
    assert params["grant_gated_tools"] == {"families": ["shell"], "tools": ["deploy"]}


@pytest.mark.asyncio
async def test_a_grant_gate_is_refused_before_an_older_server_could_drop_it() -> None:
    client, calls = fake_client(
        {"session/create": RUN_RESULT}, contract_version="0.9.7"
    )

    with pytest.raises(CapabilityUnavailableError):
        await client.create_session("hello", grant_gated_tools={"families": ["shell"]})
    with pytest.raises(CapabilityUnavailableError):
        await client.create_deferred_session(
            "hello", grant_gated_tools={"families": ["shell"]}
        )
    assert calls == []

    # A create that names no gate is unaffected.
    await client.create_session("hello")
    assert [method for method, _ in calls] == ["session/create"]
    assert "grant_gated_tools" not in calls[0][1]


# Released contract 0.9.8 has no grant gate: its session/create ignores
# `grant_gated_tools` and builds the session with the gated tools ungated.
RELEASED_0_9_8_REFUSAL = (
    r"grant_gated_tools needs a server contract >= 0\.9\.9; "
    r"this server reports 0\.9\.8"
)


@pytest.mark.asyncio
async def test_a_grant_gate_is_refused_before_a_released_0_9_8_server_could_drop_it() -> None:
    client, calls = fake_client(
        {"session/create": RUN_RESULT}, contract_version="0.9.8"
    )

    with pytest.raises(CapabilityUnavailableError, match=RELEASED_0_9_8_REFUSAL):
        await client.create_session("hello", grant_gated_tools={"families": ["shell"]})
    with pytest.raises(CapabilityUnavailableError, match=RELEASED_0_9_8_REFUSAL):
        await client.create_deferred_session(
            "hello", grant_gated_tools={"families": ["shell"]}
        )
    assert calls == []


def test_a_streaming_create_refuses_a_grant_gate_on_a_released_0_9_8_server() -> None:
    client, _ = fake_client({}, contract_version="0.9.8")
    client._process = MagicMock()
    client._dispatcher = MagicMock()

    with pytest.raises(CapabilityUnavailableError, match=RELEASED_0_9_8_REFUSAL):
        client.create_session_streaming(
            "hello", grant_gated_tools={"families": ["shell"]}
        )
    # Nothing was staged for sending.
    client._dispatcher.subscribe_pending_stream.assert_not_called()
    client._dispatcher.expect_response.assert_not_called()


def test_a_streaming_create_refuses_a_grant_gate_on_an_older_server() -> None:
    client, _ = fake_client({}, contract_version="0.9.7")
    client._process = MagicMock()
    client._dispatcher = MagicMock()

    with pytest.raises(CapabilityUnavailableError):
        client.create_session_streaming(
            "hello", grant_gated_tools={"families": ["shell"]}
        )
    # Nothing was staged for sending.
    client._dispatcher.subscribe_pending_stream.assert_not_called()
    client._dispatcher.expect_response.assert_not_called()

    # A streaming create that names no gate is unaffected.
    client.create_session_streaming("hello")
    client._dispatcher.subscribe_pending_stream.assert_called_once()
