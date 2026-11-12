"""Long-term agents from host code (``meta/identity/*``).

A long-term agent is a durable, named member of the mob: it keeps its own
conversation and memory across restarts and is addressed by name. An agent
made at run time is the same kind of agent as one the host's configuration
declares, and every agent is read and changed through the same calls:
:attr:`MetaRuntime.identities <meerkat_meta.runtime.MetaRuntime.identities>`
(an :class:`~meerkat_meta.runtime.IdentitiesHandle`) has one call per host
method.

Writes: ``create`` (1 to 256 agents, all or nothing), ``update``,
``archive``, ``unarchive``, ``remove``, ``adopt``, ``settle_preparation``,
``withdraw`` and ``changes_hold``. Reads: ``list``, ``get``, ``operation``,
``directory`` and ``plan``. A read never wakes an agent.

Requests are spelled once, here, as every surface writes them: a create
names ``agents: [{name, profile, ...}]`` (:class:`AgentSpec`), a change of
one agent names it ``name``, and a refusal's ``field`` is a path into that
request (``agents[1].name``, ``set.labels``, ``peers_add[0]``).

A change answers one typed outcome:

- :class:`IdentityDone`: the change committed;
- :class:`IdentityPendingDecision`: a person must decide. This is a normal
  answer, not an error: nothing but the decision is written, and the same
  ``request_id`` and request return the same state. Read the operation
  later with ``identities.operation(operation_id=outcome.operation_id)``;
- :class:`IdentityPendingPreparation`: committed, not usable until the
  host has prepared the agent and settled it (``settle_preparation``);
- a refusal raises :class:`~meerkat_meta.errors.IdentityRefusedError`, the
  one error of the family, with the typed ``kind``, ``field`` and
  ``detail``. Only ``operation`` answers a refusal as a value
  (:class:`IdentityRefused`): the operation it read ended with one.

Vocabulary: refusal kinds
(:data:`~meerkat_meta.errors.IDENTITY_REFUSAL_KINDS`), the outcome tokens
(:data:`IDENTITY_OUTCOMES`), an instruction outcome's kinds and reasons
(:data:`INSTRUCTION_OUTCOME_KINDS`, :data:`INSTRUCTION_NEXT_SESSION_REASONS`)
and a seat (:data:`AGENT_SEATS`) are closed: a new value is a breaking
change, and each set is checked against the gateway's own. A gateway newer
than this SDK may still send one this SDK does not list: it reaches you as
its token, never as an exception. Advisory values (an agent's ``readiness``,
a change's ``state``, a provenance's ``entered`` and consent, a directory
verdict, a plan's ``outcome`` and ``change``, an agent's ``addressability``
and ``runtime_mode_override`` as read back, the ``effect`` on an agent's
schedules) are open: a value this SDK does not know is kept as the gateway
sent it.
"""

from __future__ import annotations

import dataclasses
from dataclasses import dataclass
from typing import Any, ClassVar, Mapping, Sequence, Union

from .errors import ContractMismatchError, IdentityRefusal

__all__ = [
    "AGENT_SEATS",
    "IDENTITY_CHANGES",
    "IDENTITY_OUTCOMES",
    "INSTRUCTION_NEXT_SESSION_REASONS",
    "INSTRUCTION_OUTCOME_KINDS",
    "AgentDirectoryPage",
    "AgentOutcome",
    "AgentPage",
    "AgentProvenance",
    "AgentRecord",
    "AgentRef",
    "AgentSet",
    "AgentSpec",
    "AgentSummary",
    "CreateAgents",
    "DirectoryPeer",
    "DirectoryProfile",
    "IdentityChangePlan",
    "IdentityChangeRequest",
    "IdentityChangeResult",
    "IdentityChangesHold",
    "IdentityDone",
    "IdentityOperation",
    "IdentityOperationUnknown",
    "IdentityOutcome",
    "IdentityPendingDecision",
    "IdentityPendingPreparation",
    "IdentityRefusal",
    "IdentityRefused",
    "IdentityWithdrawn",
    "InstructionOutcome",
    "ScheduleRef",
    "SchedulesFollowed",
    "SourceEvidence",
    "UpdateAgent",
    "identity_outcome_from_dict",
]

#: The outcome tokens of a change, a closed vocabulary.
IDENTITY_OUTCOMES: frozenset[str] = frozenset(
    {"done", "pending_decision", "pending_preparation", "refused"}
)
#: What became of an update's instructions change, a closed vocabulary
#: (:attr:`InstructionOutcome.kind`).
INSTRUCTION_OUTCOME_KINDS: frozenset[str] = frozenset(
    {
        "applies_at_next_run",
        "applies_after_compaction",
        "applies_at_next_session",
        "unchanged",
    }
)
#: Why an instructions change applies only when the agent next starts
#: afresh, a closed vocabulary (:attr:`InstructionOutcome.reason`).
INSTRUCTION_NEXT_SESSION_REASONS: frozenset[str] = frozenset(
    {
        "prepared_authorization_required",
        "slot_limit_exceeded",
        "active_set_budget_exceeded",
        "lowering_unavailable",
        "body_too_large",
        "record_unavailable",
    }
)
#: Whether the agent a create committed is seated, a closed vocabulary
#: (:attr:`AgentOutcome.seat`).
AGENT_SEATS: frozenset[str] = frozenset({"seated", "owed"})
#: The changes of the family, as ``plan`` names them.
IDENTITY_CHANGES: tuple[str, ...] = (
    "create",
    "update",
    "archive",
    "unarchive",
    "remove",
    "adopt",
)


# ---------------------------------------------------------------------------
# Requests
# ---------------------------------------------------------------------------


def _request_id(request_id: Any) -> str:
    if not isinstance(request_id, str) or not request_id.strip():
        raise ValueError(
            "request_id is required: host code names each change, so that the "
            "same id and request return the same state"
        )
    return request_id


@dataclass(frozen=True)
class SourceEvidence:
    """Host-attested evidence of where a request came from. Recorded with
    the change, never authority."""

    attestor: str
    subject: str
    source_ref: str
    observed_at_ms: int

    def to_dict(self) -> dict[str, Any]:
        return {
            "attestor": self.attestor,
            "subject": self.subject,
            "source_ref": self.source_ref,
            "observed_at_ms": self.observed_at_ms,
        }


def _envelope(
    params: dict[str, Any],
    proposer: str | None,
    source_evidence: SourceEvidence | None,
) -> dict[str, Any]:
    if proposer is not None:
        params["proposer"] = proposer
    if source_evidence is not None:
        params["source_evidence"] = source_evidence.to_dict()
    return params


@dataclass(frozen=True)
class AgentSpec:
    """One agent of a create.

    ``name`` is unique in the mob and never available again once the agent
    is removed. ``profile`` is one of the mob's profiles: it gives the
    agent's model, tools and skills. Optional: ``display_name``;
    ``instructions``, its standing brief; ``context``, descriptive and never
    authority; ``labels``; ``first_message``, its first turn, run once (text,
    or a list of content blocks); ``peers``, the agents it can message, by
    name (agents of the mob, or other agents of the same create);
    ``addressability``, who reaches it (``addressable``, which leaving it
    out means, or ``internal_only``); ``runtime_mode_override``, how it runs
    (``autonomous_host`` or ``turn_driven``; left out, as its profile says).
    The last two are the members and values of a roster row, with the same
    meaning.
    """

    name: str
    profile: str
    display_name: str | None = None
    instructions: Sequence[str] = ()
    context: Any | None = None
    labels: Mapping[str, str] = dataclasses.field(default_factory=dict)
    first_message: str | Sequence[Mapping[str, Any]] | None = None
    peers: Sequence[str] = ()
    addressability: str | None = None
    runtime_mode_override: str | None = None

    def to_dict(self) -> dict[str, Any]:
        agent: dict[str, Any] = {"name": self.name, "profile": self.profile}
        if self.display_name is not None:
            agent["display_name"] = self.display_name
        if self.instructions:
            agent["instructions"] = list(self.instructions)
        if self.context is not None:
            agent["context"] = self.context
        if self.labels:
            agent["labels"] = dict(self.labels)
        if self.first_message is not None:
            agent["first_message"] = (
                self.first_message
                if isinstance(self.first_message, str)
                else [dict(block) for block in self.first_message]
            )
        if self.peers:
            agent["peers"] = list(self.peers)
        if self.addressability is not None:
            agent["addressability"] = self.addressability
        if self.runtime_mode_override is not None:
            agent["runtime_mode_override"] = self.runtime_mode_override
        return agent


@dataclass(frozen=True)
class CreateAgents:
    """A create of 1 to 256 agents, all or nothing. They may name each other
    as peers."""

    agents: Sequence[AgentSpec]
    request_id: str
    proposer: str | None = None
    source_evidence: SourceEvidence | None = None

    change: ClassVar[str] = "create"

    def to_dict(self) -> dict[str, Any]:
        return _envelope(
            {
                "request_id": _request_id(self.request_id),
                "agents": [agent.to_dict() for agent in self.agents],
            },
            self.proposer,
            self.source_evidence,
        )


@dataclass(frozen=True)
class AgentSet:
    """What an update sets. A member left ``None`` is unchanged.

    ``instructions`` and ``context`` are replaced whole. ``labels`` sets each
    key it names and keeps the agent's other labels; ``labels_remove`` names
    keys to remove. ``profile`` moves the agent to that profile's model and
    tools, on the same conversation. ``addressability`` and
    ``runtime_mode_override`` take the values they take on a create
    (:class:`AgentSpec`); an update sets a runtime mode and cannot return
    the agent to its profile's.
    """

    display_name: str | None = None
    instructions: Sequence[str] | None = None
    context: Any | None = None
    labels: Mapping[str, str] | None = None
    labels_remove: Sequence[str] = ()
    profile: str | None = None
    addressability: str | None = None
    runtime_mode_override: str | None = None

    def to_dict(self) -> dict[str, Any]:
        changes: dict[str, Any] = {}
        if self.display_name is not None:
            changes["display_name"] = self.display_name
        if self.instructions is not None:
            changes["instructions"] = list(self.instructions)
        if self.context is not None:
            changes["context"] = self.context
        if self.labels is not None:
            changes["labels"] = dict(self.labels)
        if self.labels_remove:
            changes["labels_remove"] = list(self.labels_remove)
        if self.profile is not None:
            changes["profile"] = self.profile
        if self.addressability is not None:
            changes["addressability"] = self.addressability
        if self.runtime_mode_override is not None:
            changes["runtime_mode_override"] = self.runtime_mode_override
        return changes


@dataclass(frozen=True)
class UpdateAgent:
    """An update of one agent.

    ``expected_spec_digest`` is the agent's ``spec_digest`` as you read it
    (``get`` or ``list``): an agent that changed meanwhile refuses
    ``stale_spec`` with the current one. There is one spec digest on every
    surface, the digest of the agent's record: the same value in ``list``,
    ``get``, a change's outcome and ``stale_spec``'s ``current_digest``.
    """

    name: str
    expected_spec_digest: str
    request_id: str
    set: AgentSet = dataclasses.field(default_factory=AgentSet)
    peers_add: Sequence[str] = ()
    peers_remove: Sequence[str] = ()
    proposer: str | None = None
    source_evidence: SourceEvidence | None = None

    change: ClassVar[str] = "update"

    def to_dict(self) -> dict[str, Any]:
        params: dict[str, Any] = {
            "request_id": _request_id(self.request_id),
            "name": self.name,
            "expected_spec_digest": self.expected_spec_digest,
            "set": self.set.to_dict(),
        }
        if self.peers_add:
            params["peers_add"] = list(self.peers_add)
        if self.peers_remove:
            params["peers_remove"] = list(self.peers_remove)
        return _envelope(params, self.proposer, self.source_evidence)


@dataclass(frozen=True)
class AgentRef:
    """A change of one agent as a whole: its archive, its unarchive, its
    removal or its adoption.

    ``expected_spec_digest`` is the agent's ``spec_digest`` as you read it
    (``get`` or ``list``). There is one spec digest on every surface, the
    digest of the agent's record: the same value in ``list``, ``get``, a
    change's outcome and ``stale_spec``'s ``current_digest``. An adoption
    does not change it.
    """

    name: str
    expected_spec_digest: str
    request_id: str
    proposer: str | None = None
    source_evidence: SourceEvidence | None = None

    def to_dict(self) -> dict[str, Any]:
        return _envelope(
            {
                "request_id": _request_id(self.request_id),
                "name": self.name,
                "expected_spec_digest": self.expected_spec_digest,
            },
            self.proposer,
            self.source_evidence,
        )


#: A change as ``plan`` takes it.
IdentityChangeRequest = Union[CreateAgents, UpdateAgent, AgentRef]


def _operation_ref(request_id: str | None, operation_id: str | None) -> dict[str, Any]:
    """An operation, named by your own request's ``request_id`` or by the
    ``operation_id`` an outcome named: exactly one."""
    if (request_id is None) == (operation_id is None):
        raise ValueError("name exactly one of request_id and operation_id")
    if request_id is not None:
        return {"request_id": request_id}
    return {"operation_id": operation_id}


def _plan_params(change: str, request: IdentityChangeRequest) -> dict[str, Any]:
    if change not in IDENTITY_CHANGES:
        raise ValueError(f"change is one of {', '.join(IDENTITY_CHANGES)}")
    expected: type = (
        CreateAgents
        if change == "create"
        else UpdateAgent
        if change == "update"
        else AgentRef
    )
    if not isinstance(request, expected):
        raise ValueError(f"a planned {change} is a {expected.__name__}")
    params = request.to_dict()
    params["change"] = change
    return params


def _settle_preparation_params(name: str, request_id: str) -> dict[str, Any]:
    """The params of a settle: the agent and the caller's id of the
    operation, and nothing else (the gateway accepts no other member)."""
    return {"name": name, "request_id": _request_id(request_id)}


# ---------------------------------------------------------------------------
# Answers
# ---------------------------------------------------------------------------


def _object(raw: Any, what: str) -> dict[str, Any]:
    if not isinstance(raw, dict):
        raise ContractMismatchError(f"{what} must be an object, got {type(raw).__name__}")
    return raw


def _required(raw: Mapping[str, Any], key: str, kind: type | tuple[type, ...], what: str) -> Any:
    value = raw.get(key)
    if isinstance(value, bool) and kind is not bool:
        value = None
    if not isinstance(value, kind):
        raise ContractMismatchError(f"{what} has no {key}")
    return value


def _optional(raw: Mapping[str, Any], key: str, kind: type | tuple[type, ...]) -> Any:
    value = raw.get(key)
    if isinstance(value, bool) and kind is not bool:
        return None
    return value if isinstance(value, kind) else None


def _strings(raw: Any) -> tuple[str, ...]:
    if not isinstance(raw, list):
        return ()
    return tuple(item for item in raw if isinstance(item, str))


@dataclass(frozen=True)
class ScheduleRef:
    """One schedule, as an answer names it: its id, and its name when it
    has one."""

    schedule_id: str
    name: str | None = None

    @classmethod
    def from_dict(cls, raw: Any) -> ScheduleRef:
        what = "a schedule"
        data = _object(raw, what)
        return cls(
            schedule_id=_required(data, "schedule_id", str, what),
            name=_optional(data, "name", str),
        )


@dataclass(frozen=True)
class SchedulesFollowed:
    """An agent's schedules as a change of the agent leaves them or, in a
    plan, would leave them.

    The schedules that target an agent follow three changes, so that no
    schedule keeps firing at an agent that takes no delivery: an archive
    pauses its Active schedules, an unarchive resumes exactly the schedules
    that were paused that way (one somebody paused by name stays paused),
    and a removal ends every one. ``effect`` says which (advisory; today
    ``paused``, ``resumed``, ``ended``), ``count`` how many and
    ``schedules`` which, as they stand after the change: after an archive
    the schedules that are paused for the agent, after an unarchive the
    agent's schedules that are active (one somebody resumed by name while
    the agent was archived included), after a removal the ended ones. A
    schedule somebody paused by name is on neither of the first two lists.
    ``incomplete`` is ``True`` when a schedule could not be read or
    changed: the same request asked again follows the rest.
    """

    effect: str
    count: int
    schedules: tuple[ScheduleRef, ...] = ()
    incomplete: bool = False

    @classmethod
    def from_dict(cls, raw: Any) -> SchedulesFollowed:
        what = "an agent's schedules"
        data = _object(raw, what)
        schedules = data.get("schedules")
        return cls(
            effect=_required(data, "effect", str, what),
            count=_required(data, "count", int, what),
            schedules=tuple(
                ScheduleRef.from_dict(schedule)
                for schedule in (schedules if isinstance(schedules, list) else [])
            ),
            incomplete=data.get("incomplete") is True,
        )


def _schedules(data: Mapping[str, Any]) -> SchedulesFollowed | None:
    schedules = data.get("schedules")
    return SchedulesFollowed.from_dict(schedules) if schedules is not None else None


@dataclass(frozen=True)
class InstructionOutcome:
    """What became of an update's instructions change.

    The new text is in the agent's record at once, whatever the outcome. It
    reaches the agent at its next run; a run that starts within moments of
    the change may still use the earlier instructions; earlier instructions
    are never rewritten.

    ``kind`` is one of :data:`INSTRUCTION_OUTCOME_KINDS`:
    ``applies_at_next_run``; ``applies_after_compaction`` (the agent's model
    takes new instructions only at the head of what it reads);
    ``applies_at_next_session`` (the change does not reach the agent as it
    runs now and applies when it next starts afresh, for ``reason``, one of
    :data:`INSTRUCTION_NEXT_SESSION_REASONS`); ``unchanged``.
    """

    kind: str
    reason: str | None = None

    @classmethod
    def from_dict(cls, raw: Any) -> InstructionOutcome:
        data = _object(raw, "an instruction outcome")
        return cls(
            kind=_required(data, "kind", str, "an instruction outcome"),
            reason=_optional(data, "reason", str),
        )


@dataclass(frozen=True)
class AgentOutcome:
    """One agent of a committed change.

    ``state`` says what the change did to it (advisory; today ``committed``,
    ``pending_preparation``, ``archiving``, ``archived``, ``restored``,
    ``retiring``, ``removed``). ``spec_digest`` is its spec digest after the
    change, what its next change names as ``expected_spec_digest``; absent
    for a removed agent. ``seat`` is on a create's outcome only, one of
    :data:`AGENT_SEATS`: ``owed`` means the agent is committed and durable
    and its first message did not start; it is seated without a restart at
    its next delivery. ``instructions`` is on an update's outcome only.
    ``schedules`` says how the agent's schedules stand after the change
    (:class:`SchedulesFollowed`): it is on the first answer of an archive,
    an unarchive and a removal, on a launch whose host gives the family its
    schedules (the stdio gateway does).

    ``seat`` and ``session_id`` say how the agent stands when the outcome is
    answered: a replay of the request, and a later ``operation`` read,
    answer today's session, and no ``seat`` once the agent is neither seated
    nor owed its seat (it was archived or removed since). A replay and an
    ``operation`` read state no ``schedules`` either, so do not compare a
    retry's answer whole with the first one: the ``operation_id`` and each
    agent's ``state`` and ``spec_digest`` are what stays.
    """

    name: str
    state: str
    spec_digest: str | None = None
    spec_revision: int | None = None
    session_id: str | None = None
    seat: str | None = None
    instructions: InstructionOutcome | None = None
    schedules: SchedulesFollowed | None = None

    @classmethod
    def from_dict(cls, raw: Any) -> AgentOutcome:
        data = _object(raw, "an agent outcome")
        instructions = data.get("instructions")
        return cls(
            name=_required(data, "name", str, "an agent outcome"),
            state=_required(data, "state", str, "an agent outcome"),
            spec_digest=_optional(data, "spec_digest", str),
            spec_revision=_optional(data, "spec_revision", int),
            session_id=_optional(data, "session_id", str),
            seat=_optional(data, "seat", str),
            instructions=(
                InstructionOutcome.from_dict(instructions)
                if instructions is not None
                else None
            ),
            schedules=_schedules(data),
        )


def _agent_outcomes(data: Mapping[str, Any]) -> tuple[AgentOutcome, ...]:
    agents = data.get("agents")
    if not isinstance(agents, list):
        raise ContractMismatchError("a committed outcome has no agents")
    return tuple(AgentOutcome.from_dict(agent) for agent in agents)


@dataclass(frozen=True)
class IdentityDone:
    """The change committed. ``agents`` are the agents it changed, in the
    order the request names them."""

    operation_id: str
    agents: tuple[AgentOutcome, ...] = ()

    outcome: ClassVar[str] = "done"


@dataclass(frozen=True)
class IdentityPendingDecision:
    """A decision is held for the change: a person must decide it.

    Nothing else is written. The same ``request_id`` and request return this
    state again, so do not repeat the change under a new id. Read what
    became of it with ``identities.operation(operation_id=...)``;
    ``deadline_ms`` is when the decision expires (UNIX ms).
    """

    operation_id: str
    decision_id: str
    deadline_ms: int

    outcome: ClassVar[str] = "pending_decision"


@dataclass(frozen=True)
class IdentityPendingPreparation:
    """The change committed, and at least one of its agents is not usable
    yet: the host prepares what the agent needs first.

    A create answers it for an agent of a profile whose preparation the
    launch's policy names ``required``. Such an agent reads
    ``readiness`` ``preparation_pending``, takes no delivery (a retryable
    refusal), runs nothing and wakes nothing. Prepare what it needs, then
    call ``identities.settle_preparation(name, request_id=...)``: the agent
    is seated dormant and its first message, if it has one, runs once. A
    host that cannot prepare an agent leaves it pending, or removes it.
    """

    operation_id: str
    agents: tuple[AgentOutcome, ...] = ()

    outcome: ClassVar[str] = "pending_preparation"


@dataclass(frozen=True)
class IdentityRefused:
    """The change was refused: nothing of it was written. A change call
    raises this as :class:`~meerkat_meta.errors.IdentityRefusedError`; only
    ``operation`` answers it as a value."""

    refusal: IdentityRefusal

    outcome: ClassVar[str] = "refused"


@dataclass(frozen=True)
class IdentityOperationUnknown:
    """No change committed under the operation and no decision is held for
    it: it was never asked, or it was refused. Asking it commits it at most
    once. ``operation_id`` is the operation the read named."""

    operation_id: str

    outcome: ClassVar[str] = "unknown"


#: The typed outcome of a change.
IdentityOutcome = Union[
    IdentityDone, IdentityPendingDecision, IdentityPendingPreparation, IdentityRefused
]
#: What a change call returns: every outcome but a refusal, which it raises.
IdentityChangeResult = Union[
    IdentityDone, IdentityPendingDecision, IdentityPendingPreparation
]
#: What ``operation`` answers.
IdentityOperation = Union[IdentityOutcome, IdentityOperationUnknown]


def identity_outcome_from_dict(raw: Any) -> IdentityOutcome:
    """The typed outcome of a change, from the gateway's answer.

    Raises :class:`~meerkat_meta.errors.ContractMismatchError` for an answer
    that is no outcome this SDK reads: an outcome token outside
    :data:`IDENTITY_OUTCOMES`, or an outcome without its members.
    """
    data = _object(raw, "an identity outcome")
    token = data.get("outcome")
    if token == "done":
        return IdentityDone(
            operation_id=_required(data, "operation_id", str, "a done outcome"),
            agents=_agent_outcomes(data),
        )
    if token == "pending_decision":
        what = "a pending_decision outcome"
        return IdentityPendingDecision(
            operation_id=_required(data, "operation_id", str, what),
            decision_id=_required(data, "decision_id", str, what),
            deadline_ms=_required(data, "deadline_ms", int, what),
        )
    if token == "pending_preparation":
        return IdentityPendingPreparation(
            operation_id=_required(
                data, "operation_id", str, "a pending_preparation outcome"
            ),
            agents=_agent_outcomes(data),
        )
    if token == "refused":
        refusal = IdentityRefusal.from_dict(data)
        if refusal is None:
            raise ContractMismatchError("a refused outcome has no kind")
        return IdentityRefused(refusal=refusal)
    raise ContractMismatchError(
        f"identity outcome {token!r} is none this SDK reads "
        f"({', '.join(sorted(IDENTITY_OUTCOMES))})"
    )


@dataclass(frozen=True)
class AgentProvenance:
    """How, when and at whose request an agent came to exist. Recorded and
    shown; nothing reads it for a decision.

    ``entered`` is advisory (today ``created``, ``adopted``, ``imported``).
    ``requester_kind`` is the kind of requester that asked (``agent``,
    ``host``, ``human``) and ``requester`` who: an agent's name, a host
    surface, or a person's principal key. ``consent`` is the consent the
    change committed under, as the gateway sent it; :attr:`consent_form`
    is its form (advisory; today ``allowed``, ``notified``, ``decided``,
    ``imported``).
    """

    entered: str
    requester_kind: str
    requester: str
    at_ms: int
    consent: Mapping[str, Any] = dataclasses.field(default_factory=dict)

    @property
    def consent_form(self) -> str | None:
        form = self.consent.get("form")
        return form if isinstance(form, str) else None

    @classmethod
    def from_dict(cls, raw: Any) -> AgentProvenance:
        what = "an agent's provenance"
        data = _object(raw, what)
        consent = data.get("consent")
        return cls(
            entered=_required(data, "entered", str, what),
            requester_kind=_required(data, "requester_kind", str, what),
            requester=_required(data, "requester", str, what),
            at_ms=_required(data, "at_ms", int, what),
            consent=dict(consent) if isinstance(consent, dict) else {},
        )


def _summary_members(data: Mapping[str, Any], what: str) -> dict[str, Any]:
    provenance = data.get("provenance")
    return {
        "name": _required(data, "name", str, what),
        "profile": _required(data, "profile", str, what),
        "spec_digest": _required(data, "spec_digest", str, what),
        "archived": _required(data, "archived", bool, what),
        "readiness": _required(data, "readiness", str, what),
        "display_name": _optional(data, "display_name", str),
        "spec_revision": _optional(data, "spec_revision", int),
        "provenance": (
            AgentProvenance.from_dict(provenance) if provenance is not None else None
        ),
        "open_operations": _strings(data.get("open_operations")),
        "addressability": _optional(data, "addressability", str),
        "runtime_mode_override": _optional(data, "runtime_mode_override", str),
        "roster_listing_superseded": data.get("roster_listing_superseded") is True,
    }


@dataclass(frozen=True)
class AgentSummary:
    """One agent as a list names it. Every agent of the mob is listed the
    same way, whoever asked for it and by whatever route it came to exist.

    ``spec_digest`` is what a change names as ``expected_spec_digest``.
    ``readiness`` says where its member stands (advisory; today ``dormant``,
    ``live``, ``broken``, ``preparation_pending``, ``retiring``,
    ``unseated``). ``open_operations`` are the ids of the operations open
    for it: its create while it waits for its host's preparation (until the
    settle), and each change of it that is held for a person's decision or
    approved and not yet carried out; empty when nothing is open. Read one
    with ``identities.operation(operation_id=...)``. ``addressability`` and
    ``runtime_mode_override`` read back what a create or an update set,
    each only when it is not the default: ``addressability`` is
    ``internal_only`` or ``None`` (the agent is addressable), and
    ``runtime_mode_override`` is ``None`` for an agent that runs as its
    profile says. ``roster_listing_superseded`` is ``True`` when the host's
    roster lists this name while the agent is served from its record (an
    adopted agent whose roster still lists it, for one): the roster's entry
    for the name is not applied, and a change of the agent is a change of
    its record. The gateway writes it only when it is so.
    """

    name: str
    profile: str
    spec_digest: str
    archived: bool
    readiness: str
    display_name: str | None = None
    spec_revision: int | None = None
    provenance: AgentProvenance | None = None
    open_operations: tuple[str, ...] = ()
    addressability: str | None = None
    runtime_mode_override: str | None = None
    roster_listing_superseded: bool = False

    @classmethod
    def from_dict(cls, raw: Any) -> AgentSummary:
        what = "an agent"
        return cls(**_summary_members(_object(raw, what), what))


@dataclass(frozen=True)
class AgentRecord(AgentSummary):
    """One agent in full: what a list says of it, and the rest of what a
    create or an update may set, under the names they set it by."""

    instructions: tuple[str, ...] = ()
    context: Any | None = None
    labels: Mapping[str, str] = dataclasses.field(default_factory=dict)
    peers: tuple[str, ...] = ()

    @classmethod
    def from_dict(cls, raw: Any) -> AgentRecord:
        what = "an agent record"
        data = _object(raw, what)
        labels = data.get("labels")
        return cls(
            **_summary_members(data, what),
            instructions=_strings(data.get("instructions")),
            context=data.get("context"),
            labels=dict(labels) if isinstance(labels, dict) else {},
            peers=_strings(data.get("peers")),
        )


@dataclass(frozen=True)
class AgentPage:
    """A page of the mob's agents, in one stable order. ``next`` is the
    name to read the next page after (``list(after=page.next)``), which
    continues that order; ``None`` on the last page. Do not rely on the
    order being alphabetical."""

    agents: tuple[AgentSummary, ...] = ()
    next: str | None = None

    @classmethod
    def from_dict(cls, raw: Any) -> AgentPage:
        data = _object(raw, "an agent page")
        agents = data.get("agents")
        if not isinstance(agents, list):
            raise ContractMismatchError("an agent page has no agents")
        return cls(
            agents=tuple(AgentSummary.from_dict(agent) for agent in agents),
            next=_optional(data, "next", str),
        )


@dataclass(frozen=True)
class DirectoryProfile:
    """One profile of the directory, with what the host's rules give a
    create of an agent of it.

    ``verdict`` is advisory (today ``allowed``; ``needs_decision``, with the
    ``lines`` the decision shows; ``not_allowed``, with the rule's
    ``reason`` when it gives one).
    """

    profile: str
    verdict: str
    lines: tuple[str, ...] = ()
    reason: str | None = None

    @classmethod
    def from_dict(cls, raw: Any) -> DirectoryProfile:
        what = "a directory profile"
        data = _object(raw, what)
        return cls(
            profile=_required(data, "profile", str, what),
            verdict=_required(data, "verdict", str, what),
            lines=_strings(data.get("lines")),
            reason=_optional(data, "reason", str),
        )


@dataclass(frozen=True)
class DirectoryPeer:
    """One agent of the directory, with what the host's rules give listing
    it as a peer: ``verdict``, ``lines`` and ``reason`` as on a
    :class:`DirectoryProfile`. ``roster_listing_superseded`` is as on an
    :class:`AgentSummary`."""

    name: str
    profile: str
    verdict: str
    display_name: str | None = None
    lines: tuple[str, ...] = ()
    reason: str | None = None
    roster_listing_superseded: bool = False

    @classmethod
    def from_dict(cls, raw: Any) -> DirectoryPeer:
        what = "a directory peer"
        data = _object(raw, what)
        return cls(
            name=_required(data, "name", str, what),
            profile=_required(data, "profile", str, what),
            verdict=_required(data, "verdict", str, what),
            display_name=_optional(data, "display_name", str),
            lines=_strings(data.get("lines")),
            reason=_optional(data, "reason", str),
            roster_listing_superseded=data.get("roster_listing_superseded") is True,
        )


@dataclass(frozen=True)
class AgentDirectoryPage:
    """A page of what the caller may name in a create or an update: the
    mob's profiles, and the agents it may list as peers. ``next`` is the
    name to read the next page after; ``None`` on the last page."""

    profiles: tuple[DirectoryProfile, ...] = ()
    peers: tuple[DirectoryPeer, ...] = ()
    next: str | None = None

    @classmethod
    def from_dict(cls, raw: Any) -> AgentDirectoryPage:
        data = _object(raw, "a directory page")
        profiles = data.get("profiles")
        peers = data.get("peers")
        return cls(
            profiles=tuple(
                DirectoryProfile.from_dict(entry)
                for entry in (profiles if isinstance(profiles, list) else [])
            ),
            peers=tuple(
                DirectoryPeer.from_dict(entry)
                for entry in (peers if isinstance(peers, list) else [])
            ),
            next=_optional(data, "next", str),
        )


@dataclass(frozen=True)
class IdentityChangePlan:
    """The plan of a change: the outcome and the rendering it would get.
    Nothing is held and nothing is written.

    ``outcome`` is what the host's rules give it (advisory; today ``allow``
    and ``require_approval``); a change they deny is refused, not planned.
    ``change`` is the change class whose rule decided (advisory).
    ``approver_id`` is who decides, when the outcome names one.
    ``subject_digest`` is the digest of ``rendering``, the text a decider
    would be shown, whole: it is what a decision would bind. ``schedules``
    is on the plan of an archive, an unarchive and a removal: the schedules
    the change would take with it (:class:`SchedulesFollowed`), as they are
    when the plan is made; it is no part of the rendering.
    ``roster_listing_superseded`` is ``True`` when the host's roster lists
    the agent the change names while the agent is served from its record,
    so the change is of the record alone; the gateway writes it only when
    it is so.
    """

    outcome: str
    change: str
    timeout_ms: int
    request_digest: str
    subject_digest: str
    rendering: str
    approver_id: str | None = None
    schedules: SchedulesFollowed | None = None
    roster_listing_superseded: bool = False

    @classmethod
    def from_dict(cls, raw: Any) -> IdentityChangePlan:
        what = "a change plan"
        data = _object(raw, what)
        return cls(
            outcome=_required(data, "outcome", str, what),
            change=_required(data, "change", str, what),
            timeout_ms=_required(data, "timeout_ms", int, what),
            request_digest=_required(data, "request_digest", str, what),
            subject_digest=_required(data, "subject_digest", str, what),
            rendering=_required(data, "rendering", str, what),
            approver_id=_optional(data, "approver_id", str),
            schedules=_schedules(data),
            roster_listing_superseded=data.get("roster_listing_superseded") is True,
        )


@dataclass(frozen=True)
class IdentityWithdrawn:
    """A withdrawn change: its decision ended, and nothing was written."""

    operation_id: str
    decision_id: str

    @classmethod
    def from_dict(cls, raw: Any) -> IdentityWithdrawn:
        what = "a withdrawal"
        data = _object(raw, what)
        return cls(
            operation_id=_required(data, "operation_id", str, what),
            decision_id=_required(data, "decision_id", str, what),
        )


@dataclass(frozen=True)
class IdentityChangesHold:
    """Whether the mob's changes of agents are held now."""

    held: bool

    @classmethod
    def from_dict(cls, raw: Any) -> IdentityChangesHold:
        what = "a changes hold"
        return cls(held=_required(_object(raw, what), "held", bool, what))
