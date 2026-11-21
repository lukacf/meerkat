"""Durable decisions on the stdio gateway (``meta/decisions/*``).

The host reads, presents and withdraws the durable decisions of the
runtime's persistent state directory through the ``MobHandle`` wrappers
(``decisions_list``, ``decisions_get``, ``decisions_rendering``,
``decisions_present``, ``decisions_withdraw``, ``decisions_decide``). A
``decisions_decide`` without an attestation is the stdio operator's; with a
:class:`DecisionAttestation`, a decision ingress the launch declares
(``runtime_options.decisions``) attests the person who decided.

``decisions_decide`` refuses while this SDK serves a host tool callback
(:class:`~meerkat_meta.errors.DecideInsideToolCallbackError`): an agent's
tool call never becomes a decision.
"""

from __future__ import annotations

from contextvars import ContextVar, Token
from dataclasses import dataclass
from typing import Any, Sequence, TypeAlias

from .errors import DecideInsideToolCallbackError

_SERVING_TOOL_CALLBACK: ContextVar[bool] = ContextVar(
    "meerkat_meta_serving_tool_callback", default=False
)


def _enter_tool_callback() -> Token[bool]:
    """Mark the current context (and the tasks it starts) as serving a host
    tool callback."""
    return _SERVING_TOOL_CALLBACK.set(True)


def _leave_tool_callback(token: Token[bool]) -> None:
    _SERVING_TOOL_CALLBACK.reset(token)


def _refuse_inside_tool_callback() -> None:
    if _SERVING_TOOL_CALLBACK.get():
        raise DecideInsideToolCallbackError()


def _presentation_string(name: str, value: object) -> None:
    if not isinstance(value, str):
        raise TypeError(f"{name} must be a string")


def _presentation_uint(name: str, value: object, bits: int) -> None:
    if type(value) is not int or not 0 <= value < 2**bits:
        raise TypeError(f"{name} must be an unsigned {bits}-bit integer")


@dataclass(frozen=True)
class DecisionApproverAudience:
    """Present to whoever acts for the decision's approver id."""

    approver_id: str

    def __post_init__(self) -> None:
        _presentation_string("approver_id", self.approver_id)

    def to_dict(self) -> dict[str, Any]:
        return {"kind": "approver", "approver_id": self.approver_id}


@dataclass(frozen=True)
class DecisionReviewerAudience:
    """Present to the decision's named reviewer (a principal key)."""

    reviewer: str

    def __post_init__(self) -> None:
        _presentation_string("reviewer", self.reviewer)

    def to_dict(self) -> dict[str, Any]:
        return {"kind": "reviewer", "reviewer": self.reviewer}


DecisionAudience: TypeAlias = DecisionApproverAudience | DecisionReviewerAudience


@dataclass(frozen=True)
class DecisionCanonicalPart:
    """A canonical rendering part returned by ``decisions_rendering``.

    ``index`` is zero-based; ``count`` is the split's part count. The gateway
    validates the digest and whether these parts cover its held rendering.
    """

    index: int
    count: int
    part_digest: str

    def __post_init__(self) -> None:
        _presentation_uint("index", self.index, 32)
        _presentation_uint("count", self.count, 32)
        _presentation_string("part_digest", self.part_digest)

    def to_dict(self) -> dict[str, Any]:
        return {"index": self.index, "count": self.count, "part_digest": self.part_digest}


@dataclass(frozen=True)
class DecisionPresentationLink:
    """A summary's link to the full rendering and the digest it served."""

    reference: str
    served_digest: str

    def __post_init__(self) -> None:
        _presentation_string("reference", self.reference)
        _presentation_string("served_digest", self.served_digest)

    def to_dict(self) -> dict[str, Any]:
        return {"reference": self.reference, "served_digest": self.served_digest}


@dataclass(frozen=True)
class DecisionPresentation:
    """Typed request describing what a host showed, not an authorization.

    Accepted by ``decisions_present`` and ``DecisionAttestation.presentation``.
    Constructors check the closed request's structure and integer widths;
    the gateway still validates the digest, completeness, audience and host
    attestation. Optional ``None`` fields are omitted on the wire.

    Existing raw dictionaries remain a separate compatibility path and are
    sent without these local checks. Results from the gateway stay raw.
    """

    audience: DecisionAudience
    subject_digest: str
    parts_shown: Sequence[DecisionCanonicalPart]
    channel_ref: str
    message_ref: str
    at_ms: int
    max_part_bytes: int | None = None
    link: DecisionPresentationLink | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.audience, (DecisionApproverAudience, DecisionReviewerAudience)):
            raise TypeError("audience must be a typed decision audience")
        for name in ("subject_digest", "channel_ref", "message_ref"):
            _presentation_string(name, getattr(self, name))
        _presentation_uint("at_ms", self.at_ms, 64)
        if self.max_part_bytes is not None:
            _presentation_uint("max_part_bytes", self.max_part_bytes, 32)
        if self.link is not None and not isinstance(self.link, DecisionPresentationLink):
            raise TypeError("link must be a DecisionPresentationLink or None")
        if (
            not isinstance(self.parts_shown, Sequence)
            or isinstance(self.parts_shown, (str, bytes, bytearray))
            or not all(isinstance(part, DecisionCanonicalPart) for part in self.parts_shown)
        ):
            raise TypeError("parts_shown must be a sequence of DecisionCanonicalPart values")
        # Retain an immutable snapshot instead of the caller's mutable list.
        object.__setattr__(self, "parts_shown", tuple(self.parts_shown))

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {
            "audience": self.audience.to_dict(),
            "subject_digest": self.subject_digest,
            "parts_shown": [part.to_dict() for part in self.parts_shown],
            "channel_ref": self.channel_ref,
            "message_ref": self.message_ref,
            "at_ms": self.at_ms,
        }
        if self.max_part_bytes is not None:
            result["max_part_bytes"] = self.max_part_bytes
        if self.link is not None:
            result["link"] = self.link.to_dict()
        return result


@dataclass(frozen=True)
class DecisionAttestation:
    """A host ingress's attestation of the person who decided.

    ``ingress`` names a decision ingress the launch declares; ``subject`` is
    the person it authenticated; ``scope`` the authentication scope (for
    example the chat session); ``event_ref`` the ingress's reference of the
    decide event, which no other decision may use; ``observed_at_ms`` the
    time the ingress observed it. ``presentation`` is what the ingress showed
    the person, in the ``meta/decisions/present`` shape, when it showed the
    decision itself.
    """

    ingress: str
    subject: str
    scope: str
    event_ref: str
    observed_at_ms: int
    presentation: DecisionPresentation | dict[str, Any] | None = None

    def to_dict(self) -> dict[str, Any]:
        attestation: dict[str, Any] = {
            "ingress": self.ingress,
            "subject": self.subject,
            "scope": self.scope,
            "event_ref": self.event_ref,
            "observed_at_ms": self.observed_at_ms,
        }
        if self.presentation is not None:
            attestation["presentation"] = (
                self.presentation.to_dict()
                if isinstance(self.presentation, DecisionPresentation)
                else self.presentation
            )
        return attestation
