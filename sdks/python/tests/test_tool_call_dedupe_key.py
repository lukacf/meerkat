"""Callback tool dedupe key delivery.

A ``tool/execute`` request that an agent-loop round dispatched carries the
call's ``dedupe_key``. The SDK parses it tolerantly (unknown fields ignored,
a missing or malformed key is no key) and passes it to a handler that
declares a ``dedupe_key`` parameter.

Unrun until the candidate's validation.
"""

import asyncio
import json
from typing import Any

import pytest

from meerkat import ToolCallDedupeKey
from meerkat.streaming import _StdoutDispatcher
from meerkat.tools import ToolRegistry


def _wire_key() -> dict[str, Any]:
    return {
        "run_id": "0b9f3c55-6b8e-4f5c-9a51-2a7d1c4e8f10",
        "round": 2,
        "call_id": "call-1",
        "call_digest": "a" * 64,
    }


def _parsed_key() -> ToolCallDedupeKey:
    return ToolCallDedupeKey(
        run_id="0b9f3c55-6b8e-4f5c-9a51-2a7d1c4e8f10",
        round=2,
        call_id="call-1",
        call_digest="a" * 64,
    )


def test_a_complete_key_parses_and_unknown_fields_are_ignored() -> None:
    assert ToolCallDedupeKey.from_wire(_wire_key()) == _parsed_key()
    assert ToolCallDedupeKey.from_wire({**_wire_key(), "later_field": True}) == _parsed_key()


@pytest.mark.parametrize(
    "value",
    [
        None,
        "call-1",
        [],
        {},
        {**_wire_key(), "round": -1},
        {**_wire_key(), "round": True},
        {**_wire_key(), "round": "2"},
        {key: value for key, value in _wire_key().items() if key != "call_digest"},
        {**_wire_key(), "run_id": 7},
    ],
)
def test_an_absent_or_malformed_key_is_no_key(value: Any) -> None:
    assert ToolCallDedupeKey.from_wire(value) is None


@pytest.mark.asyncio
async def test_the_key_reaches_only_handlers_that_declare_it() -> None:
    registry = ToolRegistry()
    seen: list[ToolCallDedupeKey | None] = []

    async def keyed(arguments: dict[str, Any], dedupe_key: ToolCallDedupeKey | None = None) -> str:
        seen.append(dedupe_key)
        return "keyed"

    async def plain(arguments: dict[str, Any]) -> str:
        return "plain"

    registry.register("keyed", keyed)
    registry.register("plain", plain)

    assert await registry.handle("keyed", {}, dedupe_key=_parsed_key()) == ("keyed", False)
    assert await registry.handle("keyed", {}) == ("keyed", False)
    assert seen == [_parsed_key(), None]
    assert await registry.handle("plain", {}, dedupe_key=_parsed_key()) == ("plain", False)


class _CapturingWriter:
    def __init__(self) -> None:
        self.frames: list[dict[str, Any]] = []

    def write(self, data: bytes) -> None:
        self.frames.append(json.loads(data.decode()))


@pytest.mark.asyncio
@pytest.mark.parametrize("with_key", [True, False])
async def test_a_tool_execute_request_with_and_without_a_dedupe_key(with_key: bool) -> None:
    registry = ToolRegistry()
    seen: list[ToolCallDedupeKey | None] = []

    async def handler(arguments: dict[str, Any], dedupe_key: ToolCallDedupeKey | None = None) -> str:
        seen.append(dedupe_key)
        return f"searched {arguments['q']}"

    registry.register("search", handler)
    dispatcher = _StdoutDispatcher(asyncio.StreamReader())
    dispatcher.set_tool_handler(registry)
    writer = _CapturingWriter()
    dispatcher._stdin_writer = writer  # type: ignore[assignment]

    params: dict[str, Any] = {
        "tool_use_id": "call-1",
        "name": "search",
        "arguments": {"q": "meerkat"},
    }
    if with_key:
        params["dedupe_key"] = _wire_key()
    await dispatcher._handle_callback_request(
        {"jsonrpc": "2.0", "id": "srv-0", "method": "tool/execute", "params": params}
    )

    assert seen == [_parsed_key() if with_key else None]
    assert writer.frames == [
        {
            "jsonrpc": "2.0",
            "id": "srv-0",
            "result": {"content": "searched meerkat", "is_error": False},
        }
    ]
