"""The original requester of the run a callback tool call serves.

The gateway names it on ``callback/call_tool`` and on ``callback/job/start``
as ``requester``, typed: the principal, and ``carried`` when the run's work
reached its agent through another actor (a peer hop or an approval). The
runtime sets it from the run's admitted inputs; tool arguments never do. A
console send from a person on a console that requires sign-in names that
person; a run without a requester names none.

A tool handler that declares a ``requester`` parameter receives it, so a tool
that acts in one person's name can refuse every other requester::

    OWNER = RequesterPrincipal(
        kind="human", id="alice-sub", trust_domain_id="oidc-issuer:https://id.example"
    )

    async def read_mail(args, requester=None):
        if requester is None or requester.direct_principal != OWNER:
            raise PermissionError("this mailbox serves its owner only")
        ...
"""
from __future__ import annotations

import inspect
from dataclasses import dataclass
from typing import Any, Callable

__all__ = [
    "RequesterPrincipal",
    "ToolCallRequester",
]


@dataclass(frozen=True)
class RequesterPrincipal:
    """The principal a requester names.

    ``kind`` is the principal's category (``"human"`` for a person) and ``id``
    its id within ``trust_domain_id``: the qualified trust domain (for a
    person signed in through an identity provider, ``"oidc-issuer:"`` and the
    issuer), or ``None`` for an unqualified principal.
    """

    kind: str
    id: str
    trust_domain_id: str | None = None


@dataclass(frozen=True)
class ToolCallRequester:
    """The original requester of the run a callback serves.

    A tool that acts in a person's name serves only a direct requester
    (:attr:`direct_principal`), never one carried across a peer hop.
    """

    principal: RequesterPrincipal
    carried: bool = False

    @property
    def direct_principal(self) -> RequesterPrincipal | None:
        """The principal, only when the requester is direct."""
        return None if self.carried else self.principal

    @classmethod
    def from_wire(cls, value: Any) -> ToolCallRequester | None:
        """Parse a wire ``requester`` object.

        An absent or malformed value is ``None``: no requester, which a
        person-scoped tool refuses.
        """
        if not isinstance(value, dict):
            return None
        principal = value.get("principal")
        if not isinstance(principal, dict):
            return None
        kind = principal.get("kind")
        principal_id = principal.get("id")
        if (
            not isinstance(kind, str)
            or not kind.strip()
            or not isinstance(principal_id, str)
            or not principal_id.strip()
        ):
            return None
        trust_domain_id = None
        qualification = principal.get("qualification")
        if qualification is not None:
            if not isinstance(qualification, dict):
                return None
            qualification_kind = qualification.get("kind")
            if qualification_kind == "qualified":
                trust_domain_id = qualification.get("trust_domain_id")
                if not isinstance(trust_domain_id, str) or not trust_domain_id.strip():
                    return None
            elif qualification_kind != "unqualified":
                return None
        carried = value.get("carried", False)
        if not isinstance(carried, bool):
            return None
        return cls(
            principal=RequesterPrincipal(
                kind=kind, id=principal_id, trust_domain_id=trust_domain_id
            ),
            carried=carried,
        )


def _accepts_requester(handler: Callable[..., Any]) -> bool:
    """Whether ``handler`` declares a ``requester`` parameter it can take by
    keyword."""
    try:
        parameter = inspect.signature(handler).parameters.get("requester")
    except (TypeError, ValueError):
        return False
    return parameter is not None and parameter.kind in (
        inspect.Parameter.POSITIONAL_OR_KEYWORD,
        inspect.Parameter.KEYWORD_ONLY,
    )
