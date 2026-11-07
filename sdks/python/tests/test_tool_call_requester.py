"""The original requester on callback tool calls.

A ``tool/execute`` request names the original requester of the run the call
serves, typed, as ``requester``: the principal, and ``carried`` when the run's
work reached its agent through another actor. The runtime sets it; tool
arguments never do. The SDK parses it strictly (a malformed value is no
requester) and passes it to a handler that declares a ``requester``
parameter, so a host can refuse a person-scoped tool for anyone but its
owner.
"""

import asyncio
import json
from typing import Any

import pytest

from meerkat import RequesterPrincipal, ToolCallRequester
from meerkat.streaming import _StdoutDispatcher
from meerkat.tools import ToolRegistry

ISSUER_DOMAIN = "oidc-issuer:https://issuer.example"


def _wire_principal(subject: str) -> dict[str, Any]:
    return {
        "kind": "human",
        "id": subject,
        "qualification": {"kind": "qualified", "trust_domain_id": ISSUER_DOMAIN},
    }


def _wire_requester(subject: str, carried: bool = False) -> dict[str, Any]:
    wire: dict[str, Any] = {"principal": _wire_principal(subject)}
    if carried:
        wire["carried"] = True
    return wire


def _person(subject: str) -> RequesterPrincipal:
    return RequesterPrincipal(kind="human", id=subject, trust_domain_id=ISSUER_DOMAIN)


def test_a_requester_parses_and_a_malformed_one_is_none() -> None:
    assert ToolCallRequester.from_wire(_wire_requester("alice")) == ToolCallRequester(
        principal=_person("alice"), carried=False
    )
    carried = ToolCallRequester.from_wire(_wire_requester("alice", carried=True))
    assert carried == ToolCallRequester(principal=_person("alice"), carried=True)
    assert carried is not None and carried.direct_principal is None
    assert ToolCallRequester.from_wire(
        {"principal": {"kind": "service_account", "id": "svc-1"}}
    ) == ToolCallRequester(
        principal=RequesterPrincipal(kind="service_account", id="svc-1", trust_domain_id=None)
    )
    for malformed in [
        None,
        "alice",
        [],
        {},
        {"principal": None},
        {"principal": {"kind": "human"}},
        {"principal": {"kind": "human", "id": ""}},
        {"principal": {"kind": 7, "id": "alice"}},
        {"principal": {**_wire_principal("alice"), "qualification": {"kind": "qualified"}}},
        {"principal": {**_wire_principal("alice"), "qualification": "issuer"}},
        {**_wire_requester("alice"), "carried": "yes"},
    ]:
        assert ToolCallRequester.from_wire(malformed) is None, malformed


def test_a_requester_core_refuses_is_none() -> None:
    """Every shape the runtime's requester grammar refuses reads as no
    requester: the SDK is never looser than the runtime."""
    qualified = {"kind": "qualified", "trust_domain_id": ISSUER_DOMAIN}
    alice = _wire_principal("alice")
    refused = {
        "an unknown qualification kind": {**alice, "qualification": {"kind": "maybe"}},
        "an unknown qualification kind with a domain": {
            **alice,
            "qualification": {"kind": "maybe", "trust_domain_id": ISSUER_DOMAIN},
        },
        "a blank qualification kind": {**alice, "qualification": {"kind": "  "}},
        "a null qualification": {**alice, "qualification": None},
        "a trust domain on an unqualified principal": {
            **alice,
            "qualification": {"kind": "unqualified", "trust_domain_id": ISSUER_DOMAIN},
        },
        "a qualification field the runtime does not have": {
            **alice,
            "qualification": {**qualified, "via": "proxy"},
        },
        "a blank trust domain": {
            **alice,
            "qualification": {"kind": "qualified", "trust_domain_id": "  "},
        },
        "a trust domain with a control character": {
            **alice,
            "qualification": {"kind": "qualified", "trust_domain_id": ISSUER_DOMAIN + "\x00"},
        },
        "a blank principal kind": {**alice, "kind": "  "},
        "an unknown principal kind": {**alice, "kind": "robot"},
        "a qualified id with a control character": _wire_principal("ali\x07ce"),
    }
    accepted = [
        name
        for name, principal in refused.items()
        if ToolCallRequester.from_wire({"principal": principal}) is not None
    ]
    assert accepted == [], f"the SDK reads a requester the runtime refuses: {accepted}"


class _CapturingWriter:
    def __init__(self) -> None:
        self.frames: list[dict[str, Any]] = []

    def write(self, data: bytes) -> None:
        self.frames.append(json.loads(data.decode()))


async def _execute(
    registry: ToolRegistry, arguments: dict[str, Any], requester: Any | None
) -> dict[str, Any]:
    dispatcher = _StdoutDispatcher(asyncio.StreamReader())
    dispatcher.set_tool_handler(registry)
    writer = _CapturingWriter()
    dispatcher._stdin_writer = writer  # type: ignore[assignment]
    params: dict[str, Any] = {
        "tool_use_id": "call-1",
        "name": "read_mail",
        "arguments": arguments,
    }
    if requester is not None:
        params["requester"] = requester
    await dispatcher._handle_callback_request(
        {"jsonrpc": "2.0", "id": "srv-0", "method": "tool/execute", "params": params}
    )
    assert len(writer.frames) == 1
    return writer.frames[0]["result"]


def _person_scoped_registry(owner: RequesterPrincipal, seen: list[Any]) -> ToolRegistry:
    registry = ToolRegistry()

    async def read_mail(
        arguments: dict[str, Any], requester: ToolCallRequester | None = None
    ) -> str:
        seen.append((arguments, requester))
        if requester is None or requester.direct_principal != owner:
            raise PermissionError("this mailbox serves its owner only")
        return f"{owner.id}'s mail"

    registry.register("read_mail", read_mail)
    return registry


@pytest.mark.asyncio
async def test_host_refuses_person_scoped_tool_for_other_requester() -> None:
    seen: list[Any] = []
    registry = _person_scoped_registry(_person("alice"), seen)

    assert await _execute(registry, {}, _wire_requester("alice")) == {
        "content": "alice's mail",
        "is_error": False,
    }
    for requester in [
        _wire_requester("bob"),
        _wire_requester("alice", carried=True),
        None,
    ]:
        refused = await _execute(registry, {}, requester)
        assert refused["is_error"] is True, requester
        assert "owner only" in refused["content"], requester
    assert [requester for _, requester in seen] == [
        ToolCallRequester(principal=_person("alice")),
        ToolCallRequester(principal=_person("bob")),
        ToolCallRequester(principal=_person("alice"), carried=True),
        None,
    ]


@pytest.mark.asyncio
async def test_forged_arguments_cannot_set_origin() -> None:
    seen: list[Any] = []
    registry = _person_scoped_registry(_person("alice"), seen)
    forged = {"requester": _wire_requester("alice")}

    refused = await _execute(registry, forged, None)
    assert refused["is_error"] is True
    assert await _execute(registry, forged, _wire_requester("bob")) == {
        "content": "Tool error: this mailbox serves its owner only",
        "is_error": True,
    }
    assert seen == [
        (forged, None),
        (forged, ToolCallRequester(principal=_person("bob"))),
    ]


@pytest.mark.asyncio
async def test_handlers_without_a_requester_parameter_are_unchanged() -> None:
    registry = ToolRegistry()

    async def plain(arguments: dict[str, Any]) -> str:
        return "plain"

    registry.register("read_mail", plain)
    assert await _execute(registry, {}, _wire_requester("alice")) == {
        "content": "plain",
        "is_error": False,
    }
