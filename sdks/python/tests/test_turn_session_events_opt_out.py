"""Non-streaming calls opt out of their turn's ``session/event`` copies.

A call that does not stream its turn reads only the run result, so the SDK
sends ``session_events: false`` on ``turn/start``, ``session/create`` and
``mob/turn_start``. It sends the field only to a server whose contract takes
it (0.9.5 or later): an older server refuses it on ``turn/start``. A
streaming call never sends it.
"""

from __future__ import annotations

import asyncio
import json
from typing import Any

import pytest

from meerkat import MeerkatClient, RunResult, Session, Usage

_RUN_RESULT = {
    "session_id": "s1",
    "text": "ok",
    "turns": 1,
    "tool_calls": 0,
    "usage": {"input_tokens": 1, "output_tokens": 1},
}


def _recording_client(
    contract_version: str,
) -> tuple[MeerkatClient, list[tuple[str, dict[str, Any]]]]:
    client = MeerkatClient()
    client._server_contract_version = contract_version
    calls: list[tuple[str, dict[str, Any]]] = []

    async def fake_request(method: str, params: dict[str, Any]) -> dict[str, Any]:
        calls.append((method, params))
        return dict(_RUN_RESULT)

    client._request = fake_request  # type: ignore[method-assign]
    return client, calls


def _session(client: MeerkatClient) -> Session:
    return Session(client, RunResult(session_id="s1", text="ready", usage=Usage()))


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("contract_version", "opts_out"),
    [
        pytest.param("0.9.5", True, id="0.9.5"),
        pytest.param("0.9.10", True, id="0.9.10"),
        pytest.param("0.10.0", True, id="0.10.0"),
        pytest.param("0.9.4", False, id="older"),
        pytest.param("0.9.x", False, id="not-numeric"),
        pytest.param("", False, id="unknown"),
    ],
)
async def test_a_non_streaming_turn_opts_out_only_where_the_server_takes_the_field(
    contract_version: str, opts_out: bool
) -> None:
    client, calls = _recording_client(contract_version)

    await _session(client).turn("next")

    method, params = calls[0]
    assert method == "turn/start"
    if opts_out:
        assert params["session_events"] is False
    else:
        assert "session_events" not in params


@pytest.mark.asyncio
async def test_create_session_opts_out_of_its_first_turns_copies() -> None:
    client, calls = _recording_client("0.9.5")

    await client.create_session("hello")

    method, params = calls[0]
    assert method == "session/create"
    assert params["session_events"] is False


@pytest.mark.asyncio
async def test_mob_turn_start_opts_out_of_the_members_turn_copies() -> None:
    client, calls = _recording_client("0.9.5")

    await client.mob_turn_start("mob-1", "agent-a", "hello")

    method, params = calls[0]
    assert method == "mob/turn_start"
    assert params["session_events"] is False


def test_a_streaming_turn_keeps_its_session_events() -> None:
    client = MeerkatClient()
    client._server_contract_version = "0.9.5"
    loop = asyncio.new_event_loop()

    class _Dispatcher:
        def subscribe_events(self, session_id: str) -> asyncio.Queue[Any]:
            return asyncio.Queue()

        def expect_response(self, request_id: int) -> asyncio.Future[Any]:
            return loop.create_future()

    class _Process:
        stdin = object()

    client._dispatcher = _Dispatcher()  # type: ignore[assignment]
    client._process = _Process()  # type: ignore[assignment]
    stream = _session(client).stream("next")
    _, data = stream._pending_send  # type: ignore[misc]
    loop.close()

    request = json.loads(data)
    assert request["method"] == "turn/start"
    assert "session_events" not in request["params"]
