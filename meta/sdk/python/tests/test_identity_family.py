"""Meta SDK calls of the long-term agent family (``meta/identity/*``).

Each call sends its method with exactly the params it names, in the one
spelling every surface writes (a create names ``agents: [{name, ...}]`` flat,
a change of one agent names it ``name``); each answer is typed; an outcome
is one of ``done``, ``pending_decision``, ``pending_preparation`` and
``refused``; every refusal is the one typed error, whose kinds are a closed
vocabulary mirrored from the gateway's; and an advisory value this SDK does
not know is kept as the gateway sent it.
"""

import asyncio
import dataclasses
import re
from pathlib import Path

import pytest

from meerkat_meta.errors import ContractMismatchError, RpcError
from meerkat_meta.runtime import MetaRuntime, _rpc_error_from_payload

DIGEST = "sha256:" + "a" * 64
NEXT_DIGEST = "sha256:" + "b" * 64
OPERATION_ID = "host." + "c" * 32 + ".req-1"
DECISION_ID = "decision:" + "d" * 64

_IDENTITY_CREATION = (
    Path(__file__).resolve().parents[3]
    / "crates"
    / "meerkat-meta"
    / "src"
    / "identity_creation"
)


class ScriptedTransport:
    """Records each request and answers it from a script: a result, or a
    JSON-RPC error, per method."""

    def __init__(self, results=None, errors=None):
        self.calls: list[tuple[str, dict]] = []
        self.request_timeout = 60.0
        self._results = dict(results or {})
        self._errors = dict(errors or {})

    def send_sync(self, request):
        method = request["method"]
        self.calls.append((method, request.get("params")))
        if method in self._errors:
            return {
                "jsonrpc": "2.0",
                "id": request.get("id"),
                "error": self._errors[method],
            }
        return {
            "jsonrpc": "2.0",
            "id": request.get("id"),
            "result": self._results.get(method, {}),
        }

    async def send_async(self, request, *, timeout=None):
        return self.send_sync(request)

    def is_running(self):
        return True

    def start(self):
        pass

    def stop(self):
        pass

    def set_callback_handler(self, handler):
        pass


def make_identities(results=None, errors=None):
    from meerkat_meta.agent_builder import CallbackDispatcher
    from meerkat_meta.runtime import IdentitiesHandle

    transport = ScriptedTransport(results=results, errors=errors)
    runtime = MetaRuntime.__new__(MetaRuntime)
    runtime._config = None
    runtime._transport = transport
    runtime._running = True
    runtime._rust_http_base = None
    runtime._lifecycle_lock = asyncio.Lock()
    runtime._shutdown_task = None
    runtime._dispatcher = CallbackDispatcher()
    return IdentitiesHandle(runtime), transport


def summary(name="scout", **overrides):
    row = {
        "name": name,
        "profile": "researcher",
        "spec_digest": DIGEST,
        "archived": False,
        "readiness": "dormant",
    }
    row.update(overrides)
    return row


def done(*agents, operation_id=OPERATION_ID):
    return {"outcome": "done", "operation_id": operation_id, "agents": list(agents)}


# ---------------------------------------------------------------------------
# Request spelling: one golden per call
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_adopt_get_list_and_operation_send_their_params():
    identities, transport = make_identities(
        results={
            "meta/identity/adopt": done({"name": "scout", "state": "committed"}),
            "meta/identity/get": summary(instructions=[], labels={}, peers=[]),
            "meta/identity/list": {"agents": []},
            "meta/identity/operation": done({"name": "scout", "state": "committed"}),
        }
    )
    await identities.adopt("scout", expected_spec_digest=DIGEST, request_id="req-1")
    await identities.get("scout")
    await identities.list()
    await identities.list(profile="researcher", include_archived=True, after="scout")
    await identities.operation(request_id="req-1")
    await identities.operation(operation_id=OPERATION_ID)
    assert transport.calls == [
        (
            "meta/identity/adopt",
            {"request_id": "req-1", "name": "scout", "expected_spec_digest": DIGEST},
        ),
        ("meta/identity/get", {"name": "scout"}),
        ("meta/identity/list", {"include_archived": False}),
        (
            "meta/identity/list",
            {"profile": "researcher", "include_archived": True, "after": "scout"},
        ),
        ("meta/identity/operation", {"request_id": "req-1"}),
        ("meta/identity/operation", {"operation_id": OPERATION_ID}),
    ]


@pytest.mark.asyncio
async def test_create_names_its_agents_flat_by_name():
    from meerkat_meta import AgentSpec, SourceEvidence

    identities, transport = make_identities(
        results={"meta/identity/create": done({"name": "scout", "state": "committed"})}
    )
    await identities.create(
        [
            AgentSpec(
                name="scout",
                profile="researcher",
                display_name="Scout",
                instructions=["Find sources.", "Cite each one."],
                context={"team": "research"},
                labels={"area": "research"},
                first_message="Introduce yourself.",
                peers=["editor"],
            ),
            AgentSpec(name="editor", profile="writer"),
        ],
        request_id="req-create",
        proposer="planner",
        source_evidence=SourceEvidence(
            attestor="host",
            subject="ticket-7",
            source_ref="tracker:7",
            observed_at_ms=1_900_000_000_000,
        ),
    )
    assert transport.calls == [
        (
            "meta/identity/create",
            {
                "request_id": "req-create",
                "agents": [
                    {
                        "name": "scout",
                        "profile": "researcher",
                        "display_name": "Scout",
                        "instructions": ["Find sources.", "Cite each one."],
                        "context": {"team": "research"},
                        "labels": {"area": "research"},
                        "first_message": "Introduce yourself.",
                        "peers": ["editor"],
                    },
                    {"name": "editor", "profile": "writer"},
                ],
                "proposer": "planner",
                "source_evidence": {
                    "attestor": "host",
                    "subject": "ticket-7",
                    "source_ref": "tracker:7",
                    "observed_at_ms": 1_900_000_000_000,
                },
            },
        )
    ]


@pytest.mark.asyncio
async def test_update_names_the_agent_what_it_sets_and_its_peers():
    from meerkat_meta import AgentSet

    identities, transport = make_identities(
        results={"meta/identity/update": done({"name": "scout", "state": "committed"})}
    )
    await identities.update(
        "scout",
        expected_spec_digest=DIGEST,
        request_id="req-update",
        set=AgentSet(
            display_name="Scout II",
            instructions=["Find primary sources."],
            context={"team": "research"},
            labels={"area": "archives"},
            labels_remove=["trial"],
            profile="senior-researcher",
        ),
        peers_add=["librarian"],
        peers_remove=["editor"],
    )
    await identities.update("scout", expected_spec_digest=DIGEST, request_id="req-noop")
    assert transport.calls == [
        (
            "meta/identity/update",
            {
                "request_id": "req-update",
                "name": "scout",
                "expected_spec_digest": DIGEST,
                "set": {
                    "display_name": "Scout II",
                    "instructions": ["Find primary sources."],
                    "context": {"team": "research"},
                    "labels": {"area": "archives"},
                    "labels_remove": ["trial"],
                    "profile": "senior-researcher",
                },
                "peers_add": ["librarian"],
                "peers_remove": ["editor"],
            },
        ),
        (
            "meta/identity/update",
            {
                "request_id": "req-noop",
                "name": "scout",
                "expected_spec_digest": DIGEST,
                "set": {},
            },
        ),
    ]


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["archive", "unarchive", "remove", "adopt"])
async def test_a_change_of_one_agent_names_it_and_the_spec_it_read(change):
    from meerkat_meta import SourceEvidence

    method = f"meta/identity/{change}"
    identities, transport = make_identities(
        results={method: done({"name": "scout", "state": "committed"})}
    )
    call = getattr(identities, change)
    await call("scout", expected_spec_digest=DIGEST, request_id="req-1")
    await call(
        "scout",
        expected_spec_digest=DIGEST,
        request_id="req-2",
        proposer="planner",
        source_evidence=SourceEvidence(
            attestor="host", subject="s", source_ref="r", observed_at_ms=7
        ),
    )
    assert transport.calls == [
        (method, {"request_id": "req-1", "name": "scout", "expected_spec_digest": DIGEST}),
        (
            method,
            {
                "request_id": "req-2",
                "name": "scout",
                "expected_spec_digest": DIGEST,
                "proposer": "planner",
                "source_evidence": {
                    "attestor": "host",
                    "subject": "s",
                    "source_ref": "r",
                    "observed_at_ms": 7,
                },
            },
        ),
    ]


@pytest.mark.asyncio
async def test_withdraw_changes_hold_and_directory_send_their_params():
    identities, transport = make_identities(
        results={
            "meta/identity/withdraw": {
                "operation_id": OPERATION_ID,
                "decision_id": DECISION_ID,
            },
            "meta/identity/changes_hold": {"held": True},
            "meta/identity/directory": {"profiles": [], "peers": []},
        }
    )
    await identities.withdraw(operation_id=OPERATION_ID)
    await identities.withdraw(request_id="req-1")
    await identities.changes_hold(True, reason="deploy window")
    await identities.changes_hold(False)
    await identities.directory()
    await identities.directory(query="res", after="researcher")
    assert transport.calls == [
        ("meta/identity/withdraw", {"operation_id": OPERATION_ID}),
        ("meta/identity/withdraw", {"request_id": "req-1"}),
        ("meta/identity/changes_hold", {"held": True, "reason": "deploy window"}),
        ("meta/identity/changes_hold", {"held": False}),
        ("meta/identity/directory", {}),
        ("meta/identity/directory", {"query": "res", "after": "researcher"}),
    ]


@pytest.mark.asyncio
async def test_plan_sends_the_change_it_plans():
    from meerkat_meta import AgentRef, AgentSpec, CreateAgents, UpdateAgent

    plan = {
        "outcome": "require_approval",
        "change": "adopt",
        "approver_id": "team:owners",
        "timeout_ms": 600_000,
        "request_digest": DIGEST,
        "subject_digest": NEXT_DIGEST,
        "rendering": "adopt scout",
    }
    identities, transport = make_identities(results={"meta/identity/plan": plan})
    await identities.plan(
        "adopt", AgentRef(name="scout", expected_spec_digest=DIGEST, request_id="req-1")
    )
    await identities.plan(
        "create",
        CreateAgents(
            agents=[AgentSpec(name="scout", profile="researcher")], request_id="req-2"
        ),
    )
    await identities.plan(
        "update",
        UpdateAgent(name="scout", expected_spec_digest=DIGEST, request_id="req-3"),
    )
    assert transport.calls == [
        (
            "meta/identity/plan",
            {
                "change": "adopt",
                "request_id": "req-1",
                "name": "scout",
                "expected_spec_digest": DIGEST,
            },
        ),
        (
            "meta/identity/plan",
            {
                "change": "create",
                "request_id": "req-2",
                "agents": [{"name": "scout", "profile": "researcher"}],
            },
        ),
        (
            "meta/identity/plan",
            {
                "change": "update",
                "request_id": "req-3",
                "name": "scout",
                "expected_spec_digest": DIGEST,
                "set": {},
            },
        ),
    ]


@pytest.mark.asyncio
async def test_a_call_that_cannot_be_spelled_sends_nothing():
    from meerkat_meta import AgentRef, CreateAgents

    identities, transport = make_identities()
    with pytest.raises(ValueError):
        await identities.operation()
    with pytest.raises(ValueError):
        await identities.operation(request_id="req-1", operation_id=OPERATION_ID)
    with pytest.raises(ValueError):
        await identities.withdraw()
    with pytest.raises(ValueError):
        await identities.withdraw(request_id="req-1", operation_id=OPERATION_ID)
    with pytest.raises(ValueError):
        await identities.plan(
            "rename", AgentRef(name="scout", expected_spec_digest=DIGEST, request_id="r")
        )
    with pytest.raises(ValueError):
        await identities.plan("adopt", CreateAgents(agents=[], request_id="r"))
    # Host code names each change: a blank or missing request id is not sent.
    for request_id in ("", "  ", None):
        with pytest.raises(ValueError):
            await identities.adopt(
                "scout", expected_spec_digest=DIGEST, request_id=request_id
            )
        with pytest.raises(ValueError):
            await identities.create([], request_id=request_id)
    assert transport.calls == []


# ---------------------------------------------------------------------------
# Typed answers
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_identity_family_round_trip():
    from meerkat_meta import (
        AgentOutcome,
        AgentPage,
        AgentProvenance,
        AgentRecord,
        AgentSummary,
        IdentityDone,
    )

    record = summary(
        display_name="Scout",
        spec_revision=3,
        provenance={
            "entered": "adopted",
            "requester_kind": "host",
            "requester": "stdio",
            "at_ms": 1_900_000_000_000,
            "consent": {"form": "allowed"},
        },
        open_operations=[OPERATION_ID],
        instructions=["Find sources."],
        context={"team": "research"},
        labels={"area": "research"},
        peers=["editor"],
    )
    identities, _ = make_identities(
        results={
            "meta/identity/get": record,
            "meta/identity/list": {
                "agents": [summary(), summary("editor", archived=True, readiness="unseated")],
                "next": "editor",
            },
            "meta/identity/adopt": done(
                {
                    "name": "scout",
                    "state": "committed",
                    "spec_digest": NEXT_DIGEST,
                    "spec_revision": 1,
                    "session_id": "session-1",
                }
            ),
            "meta/identity/operation": done(
                {"name": "scout", "state": "committed", "spec_digest": NEXT_DIGEST}
            ),
        }
    )

    read = await identities.get("scout")
    assert read == AgentRecord(
        name="scout",
        profile="researcher",
        spec_digest=DIGEST,
        archived=False,
        readiness="dormant",
        display_name="Scout",
        spec_revision=3,
        provenance=AgentProvenance(
            entered="adopted",
            requester_kind="host",
            requester="stdio",
            at_ms=1_900_000_000_000,
            consent={"form": "allowed"},
        ),
        open_operations=(OPERATION_ID,),
        instructions=("Find sources.",),
        context={"team": "research"},
        labels={"area": "research"},
        peers=("editor",),
    )
    assert read.provenance.consent_form == "allowed"

    page = await identities.list()
    assert page == AgentPage(
        agents=(
            AgentSummary(
                name="scout",
                profile="researcher",
                spec_digest=DIGEST,
                archived=False,
                readiness="dormant",
            ),
            AgentSummary(
                name="editor",
                profile="researcher",
                spec_digest=DIGEST,
                archived=True,
                readiness="unseated",
            ),
        ),
        next="editor",
    )

    adopted = await identities.adopt(
        "scout", expected_spec_digest=read.spec_digest, request_id="req-1"
    )
    assert adopted == IdentityDone(
        operation_id=OPERATION_ID,
        agents=(
            AgentOutcome(
                name="scout",
                state="committed",
                spec_digest=NEXT_DIGEST,
                spec_revision=1,
                session_id="session-1",
            ),
        ),
    )
    assert adopted.outcome == "done"

    again = await identities.operation(operation_id=adopted.operation_id)
    assert isinstance(again, IdentityDone)
    assert again.agents[0].spec_digest == NEXT_DIGEST


def test_each_outcome_is_parsed_to_its_type():
    from meerkat_meta import (
        AgentOutcome,
        IdentityDone,
        IdentityPendingDecision,
        IdentityPendingPreparation,
        IdentityRefused,
        InstructionOutcome,
        identity_outcome_from_dict,
    )

    assert identity_outcome_from_dict(
        {
            "outcome": "done",
            "operation_id": OPERATION_ID,
            "agents": [
                {"name": "scout", "state": "committed", "seat": "seated"},
                {"name": "editor", "state": "committed", "seat": "owed"},
            ],
        }
    ) == IdentityDone(
        operation_id=OPERATION_ID,
        agents=(
            AgentOutcome(name="scout", state="committed", seat="seated"),
            AgentOutcome(name="editor", state="committed", seat="owed"),
        ),
    )

    held = identity_outcome_from_dict(
        {
            "outcome": "pending_decision",
            "operation_id": OPERATION_ID,
            "decision_id": DECISION_ID,
            "deadline_ms": 1_900_000_600_000,
        }
    )
    assert held == IdentityPendingDecision(
        operation_id=OPERATION_ID,
        decision_id=DECISION_ID,
        deadline_ms=1_900_000_600_000,
    )
    assert held.outcome == "pending_decision"

    preparing = identity_outcome_from_dict(
        {
            "outcome": "pending_preparation",
            "operation_id": OPERATION_ID,
            "agents": [{"name": "scout", "state": "pending_preparation"}],
        }
    )
    assert preparing == IdentityPendingPreparation(
        operation_id=OPERATION_ID,
        agents=(AgentOutcome(name="scout", state="pending_preparation"),),
    )

    refused = identity_outcome_from_dict(
        {
            "outcome": "refused",
            "kind": "stale_spec",
            "field": "expected_spec_digest",
            "retryable": False,
            "detail": "The agent changed since you read it.",
            "current_digest": NEXT_DIGEST,
            "current_revision": 4,
        }
    )
    assert isinstance(refused, IdentityRefused)
    assert refused.outcome == "refused"
    assert refused.refusal.kind == "stale_spec"
    assert refused.refusal.field == "expected_spec_digest"
    assert refused.refusal.retryable is False
    assert refused.refusal.detail == "The agent changed since you read it."
    assert refused.refusal.details == {
        "current_digest": NEXT_DIGEST,
        "current_revision": 4,
    }

    for kind, reason in [
        ("applies_at_next_run", None),
        ("applies_after_compaction", None),
        ("unchanged", None),
        ("applies_at_next_session", "prepared_authorization_required"),
        ("applies_at_next_session", "slot_limit_exceeded"),
        ("applies_at_next_session", "active_set_budget_exceeded"),
        ("applies_at_next_session", "lowering_unavailable"),
        ("applies_at_next_session", "body_too_large"),
        ("applies_at_next_session", "record_unavailable"),
    ]:
        instructions = {"kind": kind}
        if reason is not None:
            instructions["reason"] = reason
        updated = identity_outcome_from_dict(
            {
                "outcome": "done",
                "operation_id": OPERATION_ID,
                "agents": [
                    {"name": "scout", "state": "committed", "instructions": instructions}
                ],
            }
        )
        assert updated.agents[0].instructions == InstructionOutcome(kind=kind, reason=reason)


def test_an_answer_that_is_no_outcome_is_a_contract_mismatch():
    from meerkat_meta import identity_outcome_from_dict

    for answer in (
        None,
        [],
        {},
        {"outcome": "reviewed", "operation_id": OPERATION_ID},
        {"outcome": "done", "agents": []},
        {"outcome": "done", "operation_id": OPERATION_ID},
        {"outcome": "pending_preparation", "operation_id": OPERATION_ID},
        {"outcome": "done", "operation_id": OPERATION_ID, "agents": [{"state": "committed"}]},
        {"outcome": "pending_decision", "operation_id": OPERATION_ID},
        {
            "outcome": "pending_decision",
            "operation_id": OPERATION_ID,
            "decision_id": DECISION_ID,
            "deadline_ms": True,
        },
        {"outcome": "refused"},
    ):
        with pytest.raises(ContractMismatchError):
            identity_outcome_from_dict(answer)


@pytest.mark.asyncio
async def test_a_held_decision_is_an_answer_and_the_operation_is_read_later():
    from meerkat_meta import (
        IdentityDone,
        IdentityOperationUnknown,
        IdentityPendingDecision,
        IdentityRefused,
    )

    held = {
        "outcome": "pending_decision",
        "operation_id": OPERATION_ID,
        "decision_id": DECISION_ID,
        "deadline_ms": 1_900_000_600_000,
    }
    identities, transport = make_identities(
        results={"meta/identity/adopt": held, "meta/identity/operation": held}
    )
    outcome = await identities.adopt(
        "scout", expected_spec_digest=DIGEST, request_id="req-1"
    )
    assert isinstance(outcome, IdentityPendingDecision)
    assert outcome.decision_id == DECISION_ID
    later = await identities.operation(operation_id=outcome.operation_id)
    assert later == outcome
    assert transport.calls[-1] == (
        "meta/identity/operation",
        {"operation_id": OPERATION_ID},
    )

    identities, _ = make_identities(
        results={
            "meta/identity/operation": done({"name": "scout", "state": "committed"})
        }
    )
    assert isinstance(await identities.operation(request_id="req-1"), IdentityDone)

    identities, _ = make_identities(
        results={
            "meta/identity/operation": {
                "outcome": "unknown",
                "operation_id": OPERATION_ID,
            }
        }
    )
    unknown = await identities.operation(request_id="req-never-asked")
    assert unknown == IdentityOperationUnknown(operation_id=OPERATION_ID)
    assert unknown.outcome == "unknown"
    assert unknown.operation_id == OPERATION_ID

    # A refusal an operation ended with is what the read answers: the read
    # itself succeeded, so nothing is raised.
    identities, _ = make_identities(
        results={
            "meta/identity/operation": {
                "outcome": "refused",
                "kind": "decision_closed",
                "state": "denied",
                "retryable": False,
                "detail": "The decision has ended: ask again.",
            }
        }
    )
    ended = await identities.operation(operation_id=OPERATION_ID)
    assert isinstance(ended, IdentityRefused)
    assert ended.refusal.kind == "decision_closed"
    assert ended.refusal.details == {"state": "denied"}

    # An unknown operation names the operation it read.
    identities, _ = make_identities(
        results={"meta/identity/operation": {"outcome": "unknown"}}
    )
    with pytest.raises(ContractMismatchError):
        await identities.operation(request_id="req-2")


@pytest.mark.asyncio
async def test_withdraw_changes_hold_directory_and_plan_answers_are_typed():
    from meerkat_meta import (
        AgentDirectoryPage,
        AgentRef,
        DirectoryPeer,
        DirectoryProfile,
        IdentityChangePlan,
        IdentityChangesHold,
        IdentityWithdrawn,
    )

    plan = {
        "outcome": "require_approval",
        "change": "adopt",
        "approver_id": "team:owners",
        "timeout_ms": 600_000,
        "request_digest": DIGEST,
        "subject_digest": NEXT_DIGEST,
        "rendering": "adopt scout",
    }
    identities, _ = make_identities(
        results={
            "meta/identity/withdraw": {
                "operation_id": OPERATION_ID,
                "decision_id": DECISION_ID,
            },
            "meta/identity/changes_hold": {"held": True},
            "meta/identity/directory": {
                "profiles": [
                    {"profile": "researcher", "verdict": "allowed"},
                    {
                        "profile": "operator",
                        "verdict": "needs_decision",
                        "lines": ["An operator can run shell commands."],
                    },
                    {"profile": "admin", "verdict": "not_allowed", "reason": "host only"},
                ],
                "peers": [
                    {
                        "name": "editor",
                        "display_name": "Editor",
                        "profile": "writer",
                        "verdict": "not_allowed",
                    }
                ],
                "next": "editor",
            },
            "meta/identity/plan": plan,
        }
    )
    assert await identities.withdraw(operation_id=OPERATION_ID) == IdentityWithdrawn(
        operation_id=OPERATION_ID, decision_id=DECISION_ID
    )
    assert await identities.changes_hold(True) == IdentityChangesHold(held=True)
    assert await identities.directory() == AgentDirectoryPage(
        profiles=(
            DirectoryProfile(profile="researcher", verdict="allowed"),
            DirectoryProfile(
                profile="operator",
                verdict="needs_decision",
                lines=("An operator can run shell commands.",),
            ),
            DirectoryProfile(profile="admin", verdict="not_allowed", reason="host only"),
        ),
        peers=(
            DirectoryPeer(
                name="editor",
                profile="writer",
                verdict="not_allowed",
                display_name="Editor",
            ),
        ),
        next="editor",
    )
    assert await identities.plan(
        "adopt", AgentRef(name="scout", expected_spec_digest=DIGEST, request_id="r")
    ) == IdentityChangePlan(
        outcome="require_approval",
        change="adopt",
        timeout_ms=600_000,
        request_digest=DIGEST,
        subject_digest=NEXT_DIGEST,
        rendering="adopt scout",
        approver_id="team:owners",
    )


# ---------------------------------------------------------------------------
# Refusals: one error type, a closed vocabulary
# ---------------------------------------------------------------------------


def refusal_error(kind, **members):
    from meerkat_meta.errors import IDENTITY_REFUSED_CODE

    data = {"kind": kind}
    data.update(members)
    return {"code": IDENTITY_REFUSED_CODE, "message": f"refused: {kind}", "data": data}


def test_every_refusal_kind_is_the_one_typed_error():
    from meerkat_meta import IDENTITY_REFUSAL_KINDS, IdentityRefusedError

    assert len(IDENTITY_REFUSAL_KINDS) == 57
    for kind in sorted(IDENTITY_REFUSAL_KINDS):
        error = _rpc_error_from_payload(
            refusal_error(
                kind,
                field="agents[1].name",
                retryable=False,
                detail="What was wrong, and what to send instead.",
            ),
            request_id="1",
            method="meta/identity/create",
        )
        assert type(error) is IdentityRefusedError, kind
        assert isinstance(error, RpcError)
        assert error.kind == kind
        assert error.known_kind is True
        assert error.field == "agents[1].name"
        assert error.detail == "What was wrong, and what to send instead."
        assert error.retryable is False
        assert error.details == {}
        assert error.refusal.kind == kind
        assert error.method == "meta/identity/create"


def test_a_refusal_carries_its_kinds_details_and_whether_it_is_retryable():
    from meerkat_meta import IdentityRefusedError

    stale = _rpc_error_from_payload(
        refusal_error(
            "stale_spec",
            field="expected_spec_digest",
            retryable=False,
            detail="The agent changed since you read it.",
            current_digest=NEXT_DIGEST,
            current_revision=4,
        ),
        request_id="1",
        method="meta/identity/update",
    )
    assert isinstance(stale, IdentityRefusedError)
    assert stale.details == {"current_digest": NEXT_DIGEST, "current_revision": 4}
    assert stale.retryable is False

    held = _rpc_error_from_payload(
        refusal_error("changes_held", retryable=True, detail="Retry later."),
        request_id="2",
        method="meta/identity/adopt",
    )
    assert isinstance(held, IdentityRefusedError)
    assert held.retryable is True
    assert held.field is None

    # A gateway that sends neither: the detail is the error's message, and
    # whether a retry may succeed is not known.
    bare = _rpc_error_from_payload(
        refusal_error("owner_read_only"), request_id="3", method="meta/identity/update"
    )
    assert isinstance(bare, IdentityRefusedError)
    assert bare.detail == "refused: owner_read_only"
    assert bare.retryable is None


def test_only_the_family_code_with_a_kind_is_an_identity_refusal():
    from meerkat_meta import IdentityRefusedError
    from meerkat_meta.errors import IDENTITY_REFUSED_CODE

    assert IDENTITY_REFUSED_CODE == -32058
    other_code = _rpc_error_from_payload(
        {"code": -32602, "message": "Invalid params", "data": {"kind": "name_taken"}},
        request_id="1",
        method="meta/identity/create",
    )
    assert not isinstance(other_code, IdentityRefusedError)
    for data in (None, "name_taken", {}, {"kind": 7}):
        no_kind = _rpc_error_from_payload(
            {"code": IDENTITY_REFUSED_CODE, "message": "refused", "data": data},
            request_id="2",
            method="meta/identity/create",
        )
        assert not isinstance(no_kind, IdentityRefusedError)
        assert isinstance(no_kind, RpcError)


def test_params_the_gateway_cannot_read_are_the_same_typed_error():
    """Params that do not parse (an unknown member, a wrong type) are refused
    ``invalid_request`` with problem ``malformed`` and no field on the
    family's code, and a change without its ``request_id`` with problem
    ``empty`` at ``request_id``: never as JSON-RPC invalid params."""
    from meerkat_meta import IdentityRefusedError

    for field, problem in ((None, "malformed"), ("request_id", "empty")):
        members = {"field": field} if field is not None else {}
        error = _rpc_error_from_payload(
            refusal_error(
                "invalid_request",
                retryable=False,
                detail="What was wrong, and what to send instead.",
                problem=problem,
                **members,
            ),
            request_id="1",
            method="meta/identity/adopt",
        )
        assert isinstance(error, IdentityRefusedError)
        assert error.kind == "invalid_request"
        assert error.field == field
        assert error.details == {"problem": problem}


def test_a_candidate_launch_refuses_a_change_with_the_gateway_wide_error():
    """A candidate launch refuses every writer before the family is reached,
    with the gateway-wide ``-32047`` and ``kind`` ``candidate_launch``: it
    keeps the class the SDK gives that code, and is not the family's error."""
    from meerkat_meta import AnnouncementRefusedError, IdentityRefusedError

    error = _rpc_error_from_payload(
        {
            "code": -32047,
            "message": "meta/identity/adopt is refused: this gateway is a candidate launch",
            "data": {"kind": "candidate_launch"},
        },
        request_id="1",
        method="meta/identity/adopt",
    )
    assert isinstance(error, AnnouncementRefusedError)
    assert not isinstance(error, IdentityRefusedError)
    assert error.kind == "candidate_launch"


def test_a_kind_newer_than_this_sdk_is_still_the_typed_error():
    from meerkat_meta import IDENTITY_REFUSAL_KINDS, IdentityRefusedError

    error = _rpc_error_from_payload(
        refusal_error("quota_exceeded", field="agents", retryable=True, quota=3),
        request_id="1",
        method="meta/identity/create",
    )
    assert isinstance(error, IdentityRefusedError)
    assert error.kind == "quota_exceeded"
    assert error.kind not in IDENTITY_REFUSAL_KINDS
    assert error.known_kind is False
    assert error.field == "agents"
    assert error.retryable is True
    assert error.details == {"quota": 3}


@pytest.mark.asyncio
async def test_a_refused_change_raises_the_typed_error_however_it_is_carried():
    from meerkat_meta import IdentityRefusedError

    identities, _ = make_identities(
        errors={
            "meta/identity/adopt": refusal_error(
                "not_adoptable",
                field="name",
                retryable=False,
                detail="An adoption does not take this agent, for the reason given.",
                reason="nothing_to_adopt",
            )
        }
    )
    with pytest.raises(IdentityRefusedError) as raised:
        await identities.adopt("scout", expected_spec_digest=DIGEST, request_id="r")
    assert raised.value.kind == "not_adoptable"
    assert raised.value.field == "name"
    assert raised.value.details == {"reason": "nothing_to_adopt"}

    # The same refusal carried as the change's outcome is the same error.
    identities, _ = make_identities(
        results={
            "meta/identity/adopt": {
                "outcome": "refused",
                "kind": "not_adoptable",
                "field": "name",
                "retryable": False,
                "detail": "An adoption does not take this agent, for the reason given.",
                "reason": "nothing_to_adopt",
            }
        }
    )
    with pytest.raises(IdentityRefusedError) as raised:
        await identities.adopt("scout", expected_spec_digest=DIGEST, request_id="r")
    assert raised.value.kind == "not_adoptable"
    assert raised.value.field == "name"
    assert raised.value.retryable is False
    assert raised.value.details == {"reason": "nothing_to_adopt"}
    assert raised.value.method == "meta/identity/adopt"
    assert (
        raised.value.detail
        == "An adoption does not take this agent, for the reason given."
    )

    # A read's refusal is the same error.
    identities, _ = make_identities(
        errors={
            "meta/identity/get": refusal_error(
                "identity_unknown", field="name", retryable=False, detail="No agent."
            )
        }
    )
    with pytest.raises(IdentityRefusedError) as raised:
        await identities.get("nobody")
    assert raised.value.kind == "identity_unknown"


# ---------------------------------------------------------------------------
# The closed vocabularies are the gateway's
# ---------------------------------------------------------------------------


def _rust(name):
    return (_IDENTITY_CREATION / name).read_text(encoding="utf-8")


def _closed_tokens(source, type_name):
    """The wire tokens of a closed vocabulary, as the family lists them once:
    ``closed_tokens!(<type_name> { Variant => "token", ... });``."""
    listed = source.split(f"closed_tokens!({type_name} {{")
    assert len(listed) == 2, f"closed_tokens!({type_name}) is not listed exactly once"
    body = listed[1].split("});", 1)[0]
    tokens = re.findall(r'^\s*[A-Z][A-Za-z]+ => "([a-z_]+)",$', body, flags=re.MULTILINE)
    assert tokens, f"closed_tokens!({type_name}) lists no token"
    assert len(tokens) == len(set(tokens)), f"{type_name}: a token is listed twice"
    return tokens


def test_the_refusal_kinds_are_the_gateways_closed_vocabulary():
    from meerkat_meta import IDENTITY_REFUSAL_KINDS

    tokens = _closed_tokens(_rust("refusal.rs"), "IdentityRefusalKind")
    assert len(tokens) == len(set(tokens)) == 57
    assert "owner_read_only" in tokens
    assert IDENTITY_REFUSAL_KINDS == frozenset(tokens), (
        "meerkat_meta.errors.IDENTITY_REFUSAL_KINDS and IdentityRefusalKind::token "
        "differ: a kind is added or removed in both, and the change is Breaking"
    )


def test_the_outcome_instruction_and_seat_vocabularies_are_the_gateways():
    from meerkat_meta import (
        AGENT_SEATS,
        IDENTITY_OUTCOMES,
        INSTRUCTION_NEXT_SESSION_REASONS,
        INSTRUCTION_OUTCOME_KINDS,
    )

    outcome = _rust("outcome.rs")
    assert IDENTITY_OUTCOMES == frozenset(_closed_tokens(outcome, "IdentityOutcome"))
    assert IDENTITY_OUTCOMES == {
        "done",
        "pending_decision",
        "pending_preparation",
        "refused",
    }
    assert INSTRUCTION_OUTCOME_KINDS == frozenset(
        _closed_tokens(outcome, "InstructionOutcome")
    )
    assert INSTRUCTION_OUTCOME_KINDS == {
        "applies_at_next_run",
        "applies_after_compaction",
        "applies_at_next_session",
        "unchanged",
    }
    assert INSTRUCTION_NEXT_SESSION_REASONS == frozenset(
        _closed_tokens(outcome, "InstructionNextSessionReason")
    )
    assert len(INSTRUCTION_NEXT_SESSION_REASONS) == 6
    assert AGENT_SEATS == frozenset(_closed_tokens(outcome, "AgentSeat"))
    assert AGENT_SEATS == {"seated", "owed"}


# ---------------------------------------------------------------------------
# Advisory values are tolerant
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_an_unknown_advisory_value_is_kept_raw():
    from meerkat_meta import identity_outcome_from_dict

    identities, _ = make_identities(
        results={
            "meta/identity/list": {
                "agents": [
                    summary(
                        readiness="hibernating",
                        provenance={
                            "entered": "forked",
                            "requester_kind": "service",
                            "requester": "importer",
                            "at_ms": 5,
                            "consent": {"form": "reviewed", "reviewer": "editor"},
                        },
                        badge="new",
                    )
                ],
                "total": 1,
            },
            "meta/identity/directory": {
                "profiles": [
                    {
                        "profile": "researcher",
                        "verdict": "needs_review",
                        "reviewer": "editor",
                    }
                ],
                "peers": [],
            },
        }
    )
    page = await identities.list()
    agent = page.agents[0]
    assert agent.readiness == "hibernating"
    assert agent.provenance.entered == "forked"
    assert agent.provenance.requester_kind == "service"
    assert agent.provenance.consent == {"form": "reviewed", "reviewer": "editor"}
    assert agent.provenance.consent_form == "reviewed"

    directory = await identities.directory()
    assert directory.profiles[0].verdict == "needs_review"
    assert directory.profiles[0].lines == ()
    assert directory.profiles[0].reason is None

    outcome = identity_outcome_from_dict(
        {
            "outcome": "done",
            "operation_id": OPERATION_ID,
            "agents": [{"name": "scout", "state": "hibernated", "note": "x"}],
            "notice": "kept",
        }
    )
    assert outcome.agents[0].state == "hibernated"


def test_a_closed_value_newer_than_this_sdk_is_kept_as_its_token():
    """A committed change stays readable: a seat or an instruction outcome
    this SDK does not know never turns a ``done`` answer into an exception.
    The vocabulary test is what fails."""
    from meerkat_meta import AGENT_SEATS, INSTRUCTION_OUTCOME_KINDS, identity_outcome_from_dict

    outcome = identity_outcome_from_dict(
        {
            "outcome": "done",
            "operation_id": OPERATION_ID,
            "agents": [
                {
                    "name": "scout",
                    "state": "committed",
                    "seat": "reserved",
                    "instructions": {"kind": "applies_at_next_wake", "reason": "asleep"},
                }
            ],
        }
    )
    agent = outcome.agents[0]
    assert agent.seat == "reserved" and agent.seat not in AGENT_SEATS
    assert agent.instructions.kind == "applies_at_next_wake"
    assert agent.instructions.kind not in INSTRUCTION_OUTCOME_KINDS
    assert agent.instructions.reason == "asleep"


# ---------------------------------------------------------------------------
# The launch policy
# ---------------------------------------------------------------------------

HOST_MAY_CHANGE_AGENTS = {
    "enabled": True,
    "approval": {
        "rules": [
            {
                "change": [
                    "create",
                    "descriptive",
                    "capability",
                    "peers",
                    "profile",
                    "archive",
                    "remove",
                    "adopt",
                ],
                "requester": {"kind": "host", "surface": "rpc"},
                "outcome": {"kind": "allow"},
            }
        ]
    },
}


def test_the_builder_sends_the_launch_policy_as_given():
    """Without a policy every change of the family is refused
    ``change_denied``; the builder is how an SDK host launches with one."""
    import copy

    from meerkat_meta import Meta

    def runtime_options(builder):
        return MetaRuntime(builder._config)._build_init_params()["runtime_options"]

    assert "identity_creation" not in runtime_options(Meta.builder())

    policy = copy.deepcopy(HOST_MAY_CHANGE_AGENTS)
    builder = Meta.builder()
    assert builder.identity_creation(policy) is builder
    assert runtime_options(builder)["identity_creation"] == HOST_MAY_CHANGE_AGENTS
    # The builder keeps its own copy: a later edit of the caller's object
    # does not change what the launch sends.
    policy["approval"]["rules"].clear()
    policy["enabled"] = False
    assert runtime_options(builder)["identity_creation"] == HOST_MAY_CHANGE_AGENTS

    with pytest.raises(TypeError):
        Meta.builder().identity_creation(True)


# ---------------------------------------------------------------------------
# One agent model
# ---------------------------------------------------------------------------


def test_no_type_of_the_family_names_where_an_agents_record_lives():
    import meerkat_meta.identities as family

    banned = ("custody", "origin", "store_owned", "intent_managed", "declared", "created_by")
    for name in family.__all__:
        value = getattr(family, name)
        if dataclasses.is_dataclass(value):
            for field in dataclasses.fields(value):
                assert not any(word in field.name for word in banned), (name, field.name)


def test_the_family_is_exported_from_the_package():
    import meerkat_meta
    import meerkat_meta.identities as family

    for name in family.__all__:
        assert getattr(meerkat_meta, name) is getattr(family, name), name
        assert name in meerkat_meta.__all__, name
    for name in ("IdentitiesHandle", "IdentityRefusedError", "IDENTITY_REFUSAL_KINDS"):
        assert name in meerkat_meta.__all__, name
    runtime = meerkat_meta.MetaRuntime(meerkat_meta.Meta.builder()._config)
    assert isinstance(runtime.identities, meerkat_meta.IdentitiesHandle)
