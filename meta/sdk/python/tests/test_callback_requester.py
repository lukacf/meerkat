"""The original requester on Meta callbacks.

``callback/call_tool`` and ``callback/job/start`` name the original requester
of the run they serve, typed, as ``requester``: the principal, and
``carried`` when the run's work reached its agent through another actor. The
runtime sets it; tool arguments never do. The SDK parses it strictly (a
malformed value is no requester), passes it to a tool handler that declares a
``requester`` parameter, and puts a job's on ``DetachedJobContext.requester``,
so a host can refuse a person-scoped tool for anyone but its owner.
"""
from __future__ import annotations

import asyncio
from typing import Any

import pytest

from meerkat_meta import RequesterPrincipal, ToolCallRequester
from meerkat_meta.agent_builder import CallbackDispatcher
from meerkat_meta.jobs import DetachedJobContext, DetachedJobExecution
from meerkat_meta.models import SessionBuildOptions

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
        "a qualified id with a C1 control character": _wire_principal("ali\x85ce"),
        "a trust domain with DEL": {
            **alice,
            "qualification": {"kind": "qualified", "trust_domain_id": ISSUER_DOMAIN + "\x7f"},
        },
    }
    accepted = [
        name
        for name, principal in refused.items()
        if ToolCallRequester.from_wire({"principal": principal}) is not None
    ]
    assert accepted == [], f"the SDK reads a requester the runtime refuses: {accepted}"


class _MailBuilder:
    """Registers ``read_mail``, a tool that serves ``owner``'s direct requests
    only, and records what each call received."""

    def __init__(self, owner: RequesterPrincipal, seen: list[Any]) -> None:
        self.owner = owner
        self.seen = seen

    async def build_agent(self, opts: SessionBuildOptions) -> None:
        owner = self.owner
        seen = self.seen

        async def read_mail(
            args: dict[str, Any], requester: ToolCallRequester | None = None
        ) -> str:
            seen.append((args, requester))
            if requester is None or requester.direct_principal != owner:
                raise PermissionError("this mailbox serves its owner only")
            return f"{owner.id}'s mail"

        opts.register_tool("read_mail", read_mail)


async def _mail_dispatcher(seen: list[Any]) -> CallbackDispatcher:
    dispatcher = CallbackDispatcher()
    dispatcher.register_builder(_MailBuilder(_person("alice"), seen))
    await dispatcher.handle_callback("callback/build_agent", {"options": {"scope_id": "s1"}})
    return dispatcher


async def _call(
    dispatcher: CallbackDispatcher, arguments: dict[str, Any], requester: Any | None
) -> Any:
    params: dict[str, Any] = {"scope_id": "s1", "tool": "read_mail", "arguments": arguments}
    if requester is not None:
        params["requester"] = requester
    return await dispatcher.handle_callback("callback/call_tool", params)


@pytest.mark.asyncio
async def test_host_refuses_person_scoped_tool_for_other_requester() -> None:
    seen: list[Any] = []
    dispatcher = await _mail_dispatcher(seen)

    try:
        served: Any = await _call(dispatcher, {}, _wire_requester("alice"))
    except PermissionError as refused:
        served = refused
    assert served == {"content": "alice's mail"}, "the owner's direct call is served"
    for requester in [_wire_requester("bob"), _wire_requester("alice", carried=True), None]:
        with pytest.raises(PermissionError, match="owner only"):
            await _call(dispatcher, {}, requester)
    assert [requester for _, requester in seen] == [
        ToolCallRequester(principal=_person("alice")),
        ToolCallRequester(principal=_person("bob")),
        ToolCallRequester(principal=_person("alice"), carried=True),
        None,
    ]


@pytest.mark.asyncio
async def test_forged_arguments_cannot_set_origin() -> None:
    seen: list[Any] = []
    dispatcher = await _mail_dispatcher(seen)
    forged = {"requester": _wire_requester("alice")}

    with pytest.raises(PermissionError):
        await _call(dispatcher, forged, None)
    with pytest.raises(PermissionError):
        await _call(dispatcher, forged, _wire_requester("bob"))
    assert seen == [
        (forged, None),
        (forged, ToolCallRequester(principal=_person("bob"))),
    ]


@pytest.mark.asyncio
async def test_handlers_without_a_requester_parameter_are_unchanged() -> None:
    class PlainBuilder:
        async def build_agent(self, opts: SessionBuildOptions) -> None:
            opts.register_tool("read_mail", lambda args: {"echo": args.get("q")})

    dispatcher = CallbackDispatcher()
    dispatcher.register_builder(PlainBuilder())
    await dispatcher.handle_callback("callback/build_agent", {"options": {"scope_id": "s1"}})
    assert await _call(dispatcher, {"q": "hi"}, _wire_requester("alice")) == {
        "content": {"echo": "hi"}
    }


@pytest.mark.asyncio
async def test_detached_job_callback_carries_requester() -> None:
    contexts: list[DetachedJobContext] = []

    async def runner(context: DetachedJobContext) -> None:
        contexts.append(context)

    class ScanBuilder:
        async def build_agent(self, opts: SessionBuildOptions) -> None:
            opts.profile_name = "network"
            opts.register_tool(
                "security_scan",
                lambda _args: None,
                execution=DetachedJobExecution(
                    runner="example.security_scan",
                    version="1",
                    restart_class="non_resumable",
                    idempotency_scope="interaction_and_arguments",
                    submission_timeout_ms=30_000,
                    handler=runner,
                ),
            )

    dispatcher = CallbackDispatcher()
    dispatcher.register_job_rpc(lambda _method, _params: asyncio.sleep(0))
    dispatcher.register_builder(ScanBuilder())
    await dispatcher.handle_callback("callback/build_agent", {"options": {"scope_id": "b1"}})

    for job, requester in [
        ("job-person", _wire_requester("alice")),
        ("job-carried", _wire_requester("alice", carried=True)),
        ("job-none", None),
    ]:
        params: dict[str, Any] = {
            "authority": {"job_id": job, "attempt_id": f"{job}-1", "fence": 1},
            "runner": {"name": "example.security_scan", "version": "1"},
            "restart_class": "non_resumable",
            "runner_handle": f"callback:{job}:attempt:1",
            "arguments": {"target": "lan"},
        }
        if requester is not None:
            params["requester"] = requester
        result = await dispatcher.handle_callback("callback/job/start", params)
        assert result == {"accepted": True, "runner_handle": f"callback:{job}:attempt:1"}
    await dispatcher.wait_for_job_tasks()
    assert [context.requester for context in contexts] == [
        ToolCallRequester(principal=_person("alice")),
        ToolCallRequester(principal=_person("alice"), carried=True),
        None,
    ]
