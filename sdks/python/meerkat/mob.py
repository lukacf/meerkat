from __future__ import annotations

from typing import Any, Literal, NotRequired, TypedDict

from .streaming import EventSubscription

RenderClass = Literal[
    "user_prompt",
    "peer_message",
    "peer_request",
    "peer_response",
    "external_event",
    "flow_step",
    "continuation",
    "system_notice",
    "tool_scope_notice",
    "ops_progress",
]
RenderSalience = Literal["background", "normal", "important", "urgent"]
RenderMetadata = TypedDict(
    "RenderMetadata",
    {"class": RenderClass, "salience": NotRequired[RenderSalience]},
)

MemberDeliveryReceipt = TypedDict(
    "MemberDeliveryReceipt",
    {
        "agent_identity": str,
        "agent_runtime_id": str,
        "fence_token": int,
        "generation": NotRequired[int],
        "handling_mode": Literal["queue", "steer"],
    },
)

MemberRespawnReceipt = TypedDict(
    "MemberRespawnReceipt",
    {
        "agent_identity": str,
        "agent_runtime_id": str,
        "previous_fence_token": int,
        "fence_token": int,
        "generation": NotRequired[int],
    },
)

MobSpawnResult = TypedDict(
    "MobSpawnResult",
    {
        "mob_id": str,
        "agent_identity": str,
        "agent_runtime_id": str,
        "fence_token": int,
        "generation": NotRequired[int],
    },
)

MobRespawnResult = TypedDict(
    "MobRespawnResult",
    {
        "status": Literal["completed", "topology_restore_failed"],
        "receipt": MemberRespawnReceipt,
        "failed_peer_ids": NotRequired[list[str]],
    },
)

MobUnreachablePeer = TypedDict(
    "MobUnreachablePeer",
    {
        "peer": str,
        "reason": NotRequired[str],
    },
)

MobPeerConnectivitySnapshot = TypedDict(
    "MobPeerConnectivitySnapshot",
    {
        "reachable_peer_count": int,
        "unknown_peer_count": int,
        "unreachable_peers": list[MobUnreachablePeer],
    },
)

MobMemberSnapshot = TypedDict(
    "MobMemberSnapshot",
    {
        "status": str,
        "agent_runtime_id": str,
        "fence_token": int,
        "generation": NotRequired[int],
        "output_preview": NotRequired[str],
        "error": NotRequired[str],
        "tokens_used": int,
        "is_final": bool,
        "peer_connectivity": NotRequired[MobPeerConnectivitySnapshot],
        "kickoff": NotRequired[dict[str, Any]],
    },
)

MobKickoffMemberSnapshot = TypedDict(
    "MobKickoffMemberSnapshot",
    {
        "agent_identity": str,
        "agent_runtime_id": str,
        "fence_token": int,
        "generation": NotRequired[int],
        "status": str,
        "output_preview": NotRequired[str],
        "error": NotRequired[str],
        "tokens_used": int,
        "is_final": bool,
        "peer_connectivity": NotRequired[MobPeerConnectivitySnapshot],
        "kickoff": NotRequired[dict[str, Any]],
    },
)

MobHelperResult = TypedDict(
    "MobHelperResult",
    {
        "output": NotRequired[str],
        "tokens_used": int,
        "agent_identity": str,
        "agent_runtime_id": str,
        "fence_token": int,
        "generation": NotRequired[int],
    },
)

MobMember = TypedDict(
    "MobMember",
    {
        "agent_identity": str,
        "agent_runtime_id": str,
        "fence_token": int,
        "generation": NotRequired[int],
        "profile": str,
        "peer_id": NotRequired[str],
        "external_peer_specs": NotRequired[dict[str, dict[str, Any]]],
        "runtime_mode": NotRequired[str],
        "state": NotRequired[str],
        "wired_to": NotRequired[list[str]],
        "labels": NotRequired[dict[str, str]],
        "status": NotRequired[str],
        "error": NotRequired[str],
        "is_final": NotRequired[bool],
        "kickoff": NotRequired[dict[str, Any]],
    },
)

MobSpawnSpec = TypedDict(
    "MobSpawnSpec",
    {
        "profile": str,
        "agent_identity": str,
        "initial_message": NotRequired[str | list[dict[str, Any]] | None],
        "runtime_mode": NotRequired[str | None],
        "backend": NotRequired[str | None],
        "labels": NotRequired[dict[str, str] | None],
        "context": NotRequired[dict[str, Any] | None],
        "additional_instructions": NotRequired[list[str] | None],
    },
)


class Member:
    """Capability-bearing mob member handle."""

    def __init__(self, mob: "Mob", agent_identity: str):
        self._mob = mob
        self.agent_identity = agent_identity

    async def send(
        self,
        content: str | list[dict[str, Any]],
        *,
        handling_mode: Literal["queue", "steer"] = "queue",
        render_metadata: RenderMetadata | None = None,
    ) -> MemberDeliveryReceipt:
        return await self._mob._client.send_mob_member_content(
            self._mob.id,
            self.agent_identity,
            content,
            handling_mode=handling_mode,
            render_metadata=render_metadata,
        )

    async def events(self) -> EventSubscription:
        return await self._mob.subscribe_member_events(self.agent_identity)


class Mob:
    """First-class mob handle backed by explicit RPC methods."""

    def __init__(self, client: Any, mob_id: str):
        self._client = client
        self.id = mob_id

    async def status(self) -> dict[str, Any]:
        return await self._client.mob_status(self.id)

    async def members(self) -> list[MobMember]:
        return await self._client.list_mob_members(self.id)

    async def spawn(
        self,
        *,
        profile: str,
        agent_identity: str,
        initial_message: str | list[dict[str, Any]] | None = None,
        runtime_mode: str | None = None,
        backend: str | None = None,
        labels: dict[str, str] | None = None,
        context: dict[str, Any] | None = None,
        additional_instructions: list[str] | None = None,
    ) -> MobSpawnResult:
        return await self._client.spawn_mob_member(
            self.id,
            profile=profile,
            agent_identity=agent_identity,
            initial_message=initial_message,
            runtime_mode=runtime_mode,
            backend=backend,
            labels=labels,
            context=context,
            additional_instructions=additional_instructions,
        )

    async def spawn_many(
        self,
        specs: list[MobSpawnSpec],
    ) -> list[MobSpawnResult]:
        return await self._client.spawn_mob_members(self.id, specs)

    async def read_events(
        self,
        *,
        after_cursor: int = 0,
        limit: int = 100,
    ) -> dict[str, list[dict[str, Any]]]:
        return await self._client.read_mob_events(
            self.id,
            after_cursor=after_cursor,
            limit=limit,
        )

    async def retire(self, agent_identity: str) -> None:
        await self._client.retire_mob_member(self.id, agent_identity)

    async def respawn(
        self,
        agent_identity: str,
        initial_message: str | list[dict[str, Any]] | None = None,
    ) -> MobRespawnResult:
        return await self._client.respawn_mob_member(self.id, agent_identity, initial_message)

    async def force_cancel(self, agent_identity: str) -> None:
        await self._client.force_cancel_mob_member(self.id, agent_identity)

    async def member_status(self, agent_identity: str) -> MobMemberSnapshot:
        return await self._client.mob_member_status(self.id, agent_identity)

    async def wait_for_kickoff_complete(
        self,
        *,
        member_ids: list[str] | None = None,
        timeout_ms: int | None = None,
    ) -> list[MobKickoffMemberSnapshot]:
        return await self._client.wait_mob_kickoff(
            self.id,
            member_ids=member_ids,
            timeout_ms=timeout_ms,
        )

    async def spawn_helper(
        self,
        prompt: str,
        *,
        agent_identity: str | None = None,
        role_name: str | None = None,
        profile_name: str | None = None,
    ) -> MobHelperResult:
        return await self._client.spawn_mob_helper(
            self.id,
            prompt,
            agent_identity=agent_identity,
            role_name=role_name,
            profile_name=profile_name,
        )

    async def fork_helper(
        self,
        source_member_id: str,
        prompt: str,
        *,
        agent_identity: str | None = None,
        role_name: str | None = None,
        profile_name: str | None = None,
        fork_context: dict[str, Any] | None = None,
    ) -> MobHelperResult:
        return await self._client.fork_mob_helper(
            self.id,
            source_member_id,
            prompt,
            agent_identity=agent_identity,
            role_name=role_name,
            profile_name=profile_name,
            fork_context=fork_context,
        )

    async def wire(self, member: str, peer: str | dict[str, Any]) -> None:
        await self._client.wire_mob_members(self.id, member, peer)

    async def unwire(self, member: str, peer: str | dict[str, Any]) -> None:
        await self._client.unwire_mob_members(self.id, member, peer)

    async def lifecycle(self, action: str) -> None:
        await self._client.mob_lifecycle(self.id, action)

    def member(self, agent_identity: str) -> Member:
        return Member(self, agent_identity)

    async def append_system_context(
        self,
        agent_identity: str,
        text: str,
        *,
        source: str | None = None,
        idempotency_key: str | None = None,
    ) -> dict[str, Any]:
        return await self._client.append_mob_system_context(
            self.id,
            agent_identity,
            text,
            source=source,
            idempotency_key=idempotency_key,
        )

    async def flows(self) -> list[str]:
        return await self._client.list_mob_flows(self.id)

    async def run_flow(self, flow_id: str, params: dict[str, Any] | None = None) -> str:
        return await self._client.run_mob_flow(self.id, flow_id, params or {})

    async def flow_status(self, run_id: str) -> dict[str, Any] | None:
        return await self._client.get_mob_flow_status(self.id, run_id)

    async def cancel_flow(self, run_id: str) -> None:
        await self._client.cancel_mob_flow(self.id, run_id)

    async def subscribe_member_events(self, agent_identity: str) -> EventSubscription:
        """Subscribe to events for a single mob member."""
        return await self._client.subscribe_mob_member_events(self.id, agent_identity)

    async def subscribe_events(self) -> EventSubscription:
        """Subscribe to attributed mob-wide events."""
        return await self._client.subscribe_mob_events(self.id)

    async def subscribe_member(self, agent_identity: str) -> EventSubscription:
        return await self.subscribe_member_events(agent_identity)

    async def subscribe_all(self) -> EventSubscription:
        return await self.subscribe_events()
