"""Callback tool registry for providing tool implementations to the agent.

Example::

    from meerkat import MeerkatClient

    client = MeerkatClient()

    @client.tool("search", description="Search the web", input_schema={"type": "object"})
    async def handle_search(arguments: dict) -> str:
        return f"Results for {arguments.get('q', '')}"

    async with client:
        session = await client.create_session("Find info about Meerkat")

A handler that declares a ``dedupe_key`` parameter also receives the call's
:class:`ToolCallDedupeKey` (``None`` when the call has none)::

    @client.tool("charge", description="Charge a card")
    async def handle_charge(arguments: dict, dedupe_key=None) -> str:
        ...

A handler that declares a ``requester`` parameter receives the
:class:`ToolCallRequester` of the run the call serves (``None`` when it has
none), so a tool that acts in one person's name can refuse anyone else.
"""

from __future__ import annotations

import inspect
from dataclasses import dataclass
from typing import Any, Callable, Awaitable

from .types import ContentBlock

# A callback tool may return either a plain string (delivered as a single text
# block) or a typed list of :class:`ContentBlock` (delivered faithfully as the
# wire `WireToolResultContent` block array). We do NOT collapse block lists to a
# stringified blob — multimodal results must round-trip as structured content.
ToolResult = str | list[ContentBlock]


@dataclass(frozen=True)
class ToolCallDedupeKey:
    """The dedupe key of one callback tool call.

    The runtime sends it with a call that an agent-loop round dispatched. A
    resumed run that dispatches the same call again sends the same key, so a
    tool with effects that must not repeat deduplicates on all four fields.
    """

    run_id: str
    round: int
    call_id: str
    call_digest: str

    @classmethod
    def from_wire(cls, value: Any) -> ToolCallDedupeKey | None:
        """Parse a wire ``dedupe_key`` object.

        Unknown fields are ignored. An absent value, or one that is not a
        complete key, is ``None``: the key is advisory and never fails the
        call.
        """
        if not isinstance(value, dict):
            return None
        run_id = value.get("run_id")
        round_index = value.get("round")
        call_id = value.get("call_id")
        call_digest = value.get("call_digest")
        if (
            not isinstance(run_id, str)
            or isinstance(round_index, bool)
            or not isinstance(round_index, int)
            or round_index < 0
            or not isinstance(call_id, str)
            or not isinstance(call_digest, str)
        ):
            return None
        return cls(
            run_id=run_id,
            round=round_index,
            call_id=call_id,
            call_digest=call_digest,
        )


def _accepts_keyword(handler: Callable[..., Any], name: str) -> bool:
    """Whether ``handler`` declares a ``name`` parameter it can take by
    keyword."""
    try:
        parameter = inspect.signature(handler).parameters.get(name)
    except (TypeError, ValueError):
        return False
    return parameter is not None and parameter.kind in (
        inspect.Parameter.POSITIONAL_OR_KEYWORD,
        inspect.Parameter.KEYWORD_ONLY,
    )


def _accepts_dedupe_key(handler: Callable[..., Any]) -> bool:
    """Whether ``handler`` declares a ``dedupe_key`` parameter it can take by
    keyword."""
    return _accepts_keyword(handler, "dedupe_key")


@dataclass(frozen=True)
class RequesterPrincipal:
    """The principal a callback call's requester names, as the runtime sends
    it.

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
    """The original requester of the run a callback tool call serves.

    The runtime sets it from the run's admitted inputs; tool arguments never
    do. ``carried`` is true when the run's work reached its agent through
    another actor (a peer hop or an approval). A tool that acts in a person's
    name serves only a direct requester (:attr:`direct_principal`).
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
    return _accepts_keyword(handler, "requester")


@dataclass
class ToolRegistration:
    """A registered callback tool."""

    name: str
    description: str
    input_schema: dict[str, Any]
    handler: Callable[..., Awaitable[ToolResult]]
    # Whether the handler declares a ``dedupe_key`` parameter.
    accepts_dedupe_key: bool = False
    # Whether the handler declares a ``requester`` parameter.
    accepts_requester: bool = False


class ToolRegistry:
    """In-process registry of callback tool definitions and handlers."""

    def __init__(self) -> None:
        self._tools: dict[str, ToolRegistration] = {}

    def register(
        self,
        name: str,
        handler: Callable[..., Awaitable[ToolResult]],
        *,
        description: str = "",
        input_schema: dict[str, Any] | None = None,
    ) -> None:
        """Register a tool handler.

        The handler receives the call's arguments. A handler that declares a
        ``dedupe_key`` parameter also receives the call's
        :class:`ToolCallDedupeKey` by keyword, or ``None``; one that declares
        a ``requester`` parameter receives the :class:`ToolCallRequester` of
        the run the call serves, or ``None``.
        """
        self._tools[name] = ToolRegistration(
            name=name,
            description=description or f"Tool: {name}",
            input_schema=input_schema or {"type": "object"},
            handler=handler,
            accepts_dedupe_key=_accepts_dedupe_key(handler),
            accepts_requester=_accepts_requester(handler),
        )

    def definitions(self) -> list[dict[str, Any]]:
        """Return wire-format tool definitions for RPC registration."""
        return [
            {
                "name": t.name,
                "description": t.description,
                "input_schema": t.input_schema,
            }
            for t in self._tools.values()
        ]

    async def handle(
        self,
        name: str,
        arguments: dict[str, Any],
        *,
        dedupe_key: ToolCallDedupeKey | None = None,
        requester: ToolCallRequester | None = None,
    ) -> tuple[ToolResult, bool]:
        """Execute a tool handler. Returns (content, is_error).

        The content is the wire `WireToolResultContent` shape: either a plain
        string or a typed list of :class:`ContentBlock`. A block list is passed
        through faithfully (serialized as structured wire content) rather than
        coerced to a stringified blob; only genuine non-list scalars are
        stringified.

        ``dedupe_key`` and ``requester`` each reach a handler that declares
        that parameter; other handlers are called with the arguments alone.
        """
        tool = self._tools.get(name)
        if tool is None:
            return f"Unknown tool: {name}", True
        keywords: dict[str, Any] = {}
        if tool.accepts_dedupe_key:
            keywords["dedupe_key"] = dedupe_key
        if tool.accepts_requester:
            keywords["requester"] = requester
        try:
            result = await tool.handler(arguments, **keywords)
            if isinstance(result, list):
                # Typed multimodal block list — deliver as structured content.
                return result, False
            if isinstance(result, str):
                return result, False
            # Genuine scalar (e.g. number/bool) — stringify for the text path.
            return str(result), False
        except Exception as exc:
            return f"Tool error: {exc}", True

    def __len__(self) -> int:
        return len(self._tools)

    def __bool__(self) -> bool:
        return bool(self._tools)
