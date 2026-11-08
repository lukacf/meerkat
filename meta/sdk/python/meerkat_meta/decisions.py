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
from typing import Any

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
    presentation: dict[str, Any] | None = None

    def to_dict(self) -> dict[str, Any]:
        attestation: dict[str, Any] = {
            "ingress": self.ingress,
            "subject": self.subject,
            "scope": self.scope,
            "event_ref": self.event_ref,
            "observed_at_ms": self.observed_at_ms,
        }
        if self.presentation is not None:
            attestation["presentation"] = self.presentation
        return attestation
