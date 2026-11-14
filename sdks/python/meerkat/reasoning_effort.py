"""The provider-neutral ``reasoning_effort`` option.

Its level vocabulary, the typed refusal a request for a level the model or
route does not accept gets, and the report of what a run sent.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Literal, get_args

ReasoningEffortLevel = Literal["none", "minimal", "low", "medium", "high", "xhigh", "max"]
"""A provider-neutral reasoning effort level. ``xhigh`` has one spelling."""

REASONING_EFFORT_LEVELS: tuple[str, ...] = get_args(ReasoningEffortLevel)
"""The reasoning effort levels, lowest first."""

REASONING_EFFORT_CLEAR: dict[str, str] = {"action": "clear"}
"""The turn value that removes the session's level. A turn's
``reasoning_effort`` is a level (set it for this turn and the turns after
it), this value (remove it), or ``None`` (keep it)."""

REASONING_EFFORT_REFUSED_KIND = "reasoning_effort_refused"
"""``error.data.kind`` of the refusal a request for a level the model or
route does not accept gets (see :func:`reasoning_effort_refusal`)."""

ReasoningEffortRefusalReason = Literal[
    "unsupported_level",
    "supported_levels_unknown",
    "supported_levels_unavailable",
    "conflicting_provider_effort",
    "conflicting_thinking_budget",
    "conflicting_thinking_type",
    "conflicting_reasoning_body",
    "conflicting_reasoning_mode",
]
"""Why a level is refused. A closed vocabulary: a reason this SDK does not
know is not parsed (see :func:`reasoning_effort_refusal`)."""

REASONING_EFFORT_REFUSAL_REASONS: tuple[str, ...] = get_args(ReasoningEffortRefusalReason)
"""The refusal reasons this SDK knows, in the server's order."""

_TRANSIENT_REASON = "supported_levels_unavailable"


@dataclass(frozen=True, slots=True)
class ReasoningRoute:
    """The route that serves a request.

    ``backend`` (``openai_api``, ``copilot``, ``bedrock``, ``self_hosted``,
    ...) and ``api`` (``responses``, ``chat_completions``, ``messages``,
    ``generate_content``, ``realtime``) are names, not addresses. Both
    vocabularies may grow: a value this SDK does not know is kept as sent.
    """

    backend: str
    api: str


@dataclass(frozen=True, slots=True)
class ReasoningEffortRefusal:
    """A refused ``reasoning_effort`` level.

    The request was refused before any provider request: Meerkat lowered
    nothing, changed no model and moved to no other route.

    ``supported`` lists the levels the route accepts for the model, lowest
    first: ``None`` when no source states them or the source cannot be read
    now, and empty when the route or the model takes no level. ``facts`` is
    the source of ``supported`` (``catalog``, ``account``, ``operator``; the
    vocabulary may grow) and is ``None`` exactly when ``supported`` is.
    ``route`` is ``None`` when the refusal names no route.
    ``conflicting_param`` and ``conflicting_level`` are set for the
    ``conflicting_*`` reasons.

    ``transient`` says whether the same request can succeed later with no
    configuration change. It is true only for
    ``supported_levels_unavailable`` (the account's model list is not loaded
    yet) and is read from ``reason``; every other reason is permanent until
    the request or the configuration changes. ``retryable`` is the server's
    own mark of the same class (``error.data.retryable``), when it sent one.
    """

    requested: str
    provider: str
    model: str
    route: ReasoningRoute | None
    reason: str
    supported: tuple[str, ...] | None
    facts: str | None
    conflicting_param: str | None = None
    conflicting_level: str | None = None
    retryable: bool | None = None

    @property
    def transient(self) -> bool:
        return self.reason == _TRANSIENT_REASON


_UNREADABLE = object()


def _parse_route(value: Any) -> Any:
    """A route, ``None`` for a null or absent route, or the unreadable mark."""
    if value is None:
        return None
    if (
        not isinstance(value, dict)
        or not isinstance(value.get("backend"), str)
        or not isinstance(value.get("api"), str)
    ):
        return _UNREADABLE
    return ReasoningRoute(backend=value["backend"], api=value["api"])


def _is_level(value: Any) -> bool:
    return isinstance(value, str) and value in REASONING_EFFORT_LEVELS


def parse_reasoning_effort_refusal(
    value: Any, retryable: Any = None
) -> ReasoningEffortRefusal | None:
    """Parse a refusal object (the wire ``reasoning_effort_refusal``).

    ``None`` when it does not have the documented shape, a reason or level
    this SDK does not know included.
    """
    if not isinstance(value, dict):
        return None
    requested = value.get("requested")
    provider = value.get("provider")
    model = value.get("model")
    reason = value.get("reason")
    route = _parse_route(value.get("route"))
    if (
        not _is_level(requested)
        or not isinstance(provider, str)
        or not isinstance(model, str)
        or route is _UNREADABLE
        or not isinstance(reason, str)
        or reason not in REASONING_EFFORT_REFUSAL_REASONS
    ):
        return None
    supported = value.get("supported")
    levels: tuple[str, ...] | None = None
    if supported is not None:
        if not isinstance(supported, list) or not all(_is_level(level) for level in supported):
            return None
        levels = tuple(supported)
    facts = value.get("facts")
    if facts is not None and not isinstance(facts, str):
        return None
    conflicting_param = value.get("conflicting_param")
    if conflicting_param is not None and not isinstance(conflicting_param, str):
        return None
    conflicting_level = value.get("conflicting_level")
    if conflicting_level is not None and not _is_level(conflicting_level):
        return None
    return ReasoningEffortRefusal(
        requested=requested,
        provider=provider,
        model=model,
        route=route,
        reason=reason,
        supported=levels,
        facts=facts,
        conflicting_param=conflicting_param,
        conflicting_level=conflicting_level,
        retryable=retryable if isinstance(retryable, bool) else None,
    )


def reasoning_effort_refusal(error: BaseException) -> ReasoningEffortRefusal | None:
    """The typed refusal of a request whose level the model or route does not accept.

    Such a request fails with the JSON-RPC code ``-32602`` (invalid params;
    ``MeerkatError.code`` is the string ``"-32602"``) and ``error.data``
    ``{"kind": "reasoning_effort_refused", "retryable", "reasoning_effort_refusal"}``,
    which the SDK keeps as ``MeerkatError.details``. Branch on it, never on
    the error text. ``None`` for any other error, or when the data does not
    parse (a reason this SDK does not know included): ``error.details`` then
    still holds the data as sent.
    """
    details = getattr(error, "details", None)
    if not isinstance(details, dict) or details.get("kind") != REASONING_EFFORT_REFUSED_KIND:
        return None
    return parse_reasoning_effort_refusal(
        details.get("reasoning_effort_refusal"), details.get("retryable")
    )


@dataclass(frozen=True, slots=True)
class ReasoningEffortSent:
    """One distinct resolution a run sent.

    The level, the model and provider it went to, the route, the provider
    fields written (``fields``), the source of the route's levels
    (``facts``), who decided the level (``decided_by``: ``request``, or
    ``preference`` on a delegated live turn) and how many provider requests
    carried it. ``fields``, ``facts``, ``decided_by`` and the route's names
    are advisory: a value this SDK does not know is kept as sent.
    """

    level: str
    provider: str
    model: str
    route: ReasoningRoute | None = None
    fields: tuple[str, ...] = ()
    facts: str | None = None
    decided_by: str | None = None
    requests: int = 0


@dataclass(frozen=True, slots=True)
class ReasoningEffortReport:
    """What a run did with the session's level.

    ``requested`` is the level requested; ``sent`` holds each distinct
    resolution the run sent, in first-use order (two rows after a model
    fallback switch).
    """

    requested: str
    sent: tuple[ReasoningEffortSent, ...] = ()


@dataclass(frozen=True, slots=True)
class ReasoningEffortResolution:
    """The resolution of one provider request, or of a run's start.

    The members of a :class:`ReasoningEffortSent` row, with the level that
    was requested and no count.
    """

    requested: str
    level: str
    provider: str
    model: str
    route: ReasoningRoute | None = None
    fields: tuple[str, ...] = ()
    facts: str | None = None
    decided_by: str | None = None


def _string_tuple(value: Any) -> tuple[str, ...]:
    if not isinstance(value, list):
        return ()
    return tuple(item for item in value if isinstance(item, str))


def _string_or_none(value: Any) -> str | None:
    return value if isinstance(value, str) else None


def _advisory_route(value: Any) -> ReasoningRoute | None:
    route = _parse_route(value)
    return None if route is _UNREADABLE else route


def _parse_sent(value: Any) -> ReasoningEffortSent | None:
    if not isinstance(value, dict):
        return None
    level = value.get("level")
    provider = value.get("provider")
    model = value.get("model")
    if not isinstance(level, str) or not isinstance(provider, str) or not isinstance(model, str):
        return None
    requests = value.get("requests")
    return ReasoningEffortSent(
        level=level,
        provider=provider,
        model=model,
        route=_advisory_route(value.get("route")),
        fields=_string_tuple(value.get("fields")),
        facts=_string_or_none(value.get("facts")),
        decided_by=_string_or_none(value.get("decided_by")),
        requests=requests if isinstance(requests, int) and not isinstance(requests, bool) else 0,
    )


def parse_reasoning_effort_report(value: Any) -> ReasoningEffortReport | None:
    """Parse the ``reasoning_effort`` block of a run result.

    ``None`` when the run carried no level (the block is absent) or the
    block has no readable ``requested`` level. Advisory values this SDK does
    not know are kept.
    """
    if not isinstance(value, dict) or not isinstance(value.get("requested"), str):
        return None
    rows = value.get("sent")
    sent = tuple(
        row
        for row in (_parse_sent(item) for item in (rows if isinstance(rows, list) else []))
        if row is not None
    )
    return ReasoningEffortReport(requested=value["requested"], sent=sent)


def parse_reasoning_effort_resolution(value: Any) -> ReasoningEffortResolution | None:
    """Parse a resolution.

    The ``reasoning_effort`` member of ``run_started`` and ``run_resumed``,
    and the ``resolution`` member of ``request_reasoning_effort_resolved``.
    ``None`` when absent or unreadable.
    """
    if not isinstance(value, dict):
        return None
    requested = value.get("requested")
    level = value.get("level")
    provider = value.get("provider")
    model = value.get("model")
    if (
        not isinstance(requested, str)
        or not isinstance(level, str)
        or not isinstance(provider, str)
        or not isinstance(model, str)
    ):
        return None
    return ReasoningEffortResolution(
        requested=requested,
        level=level,
        provider=provider,
        model=model,
        route=_advisory_route(value.get("route")),
        fields=_string_tuple(value.get("fields")),
        facts=_string_or_none(value.get("facts")),
        decided_by=_string_or_none(value.get("decided_by")),
    )
