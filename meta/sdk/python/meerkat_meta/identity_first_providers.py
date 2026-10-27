"""Provider protocols for identity-first rosters (REQ-42)."""
from __future__ import annotations

from typing import Any, Protocol, runtime_checkable

from .identity_first_models import RosterContext


# ---------------------------------------------------------------------------
# Provider protocols (REQ-42)
# ---------------------------------------------------------------------------

@runtime_checkable
class RosterProviderProtocol(Protocol):
    async def roster(self, context: RosterContext) -> list[Any]:
        """Return the desired ``DurableAgentSpec`` list.

        ``context`` carries the booted mob definition and the identities the
        runtime has already registered; see ``RosterContext``.
        """
        ...


@runtime_checkable
class AgentCustomizerProtocol(Protocol):
    async def customize_build(
        self, context: Any, spec: Any, draft: Any,
    ) -> None: ...

    async def after_create(
        self, identity: str, session_id: str, context: Any,
    ) -> None: ...


@runtime_checkable
class TopologyProviderProtocol(Protocol):
    async def compute_edges(
        self, target_identities: list[str], context: Any,
    ) -> list[Any]: ...
