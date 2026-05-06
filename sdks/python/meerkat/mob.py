from __future__ import annotations

from typing import Any, Literal, NotRequired, TypedDict

from .generated.types import (
    MobSpawnManyResult,
    WireBudgetSplitPolicy,
    WireAuthBindingRef,
    WireContentInput,
    WireMemberLaunchMode,
    WireMobBackendKind,
    WireMobProfile,
    WireMobRuntimeMode,
    WireRuntimeBinding,
    WireToolAccessPolicy,
    WireToolFilter,
)
from .streaming import EventSubscription
from .types import ResolvedModelCapabilities

MobLifecycleAction = Literal["stop", "resume", "complete", "reset", "destroy"]

WorkOrigin = Literal["external", "internal"]

# Opaque server-resolved handle for a mob member. App code treats this as
# a transparent token — obtained from `ensure_member`/`spawn_helper`/
# `fork_helper`/member-list responses, passed back on work-lane and
# member-targeted calls.
MobMemberRef = str

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
        "member_ref": MobMemberRef,
        "handling_mode": Literal["queue", "steer"],
    },
)

MemberRespawnReceipt = TypedDict(
    "MemberRespawnReceipt",
    {
        "agent_identity": str,
        "member_ref": MobMemberRef,
    },
)

MobSpawnResult = TypedDict(
    "MobSpawnResult",
    {
        "mob_id": str,
        "agent_identity": str,
        "member_ref": MobMemberRef,
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
        "output_preview": NotRequired[str],
        "error": NotRequired[str],
        "tokens_used": int,
        "is_final": bool,
        "realtime_attachment_status": NotRequired[
            Literal[
                "unattached",
                "intent_present_unbound",
                "binding_not_ready",
                "binding_ready",
                "replacement_pending",
                "reattach_required",
            ]
        ],
        "resolved_capabilities": NotRequired[ResolvedModelCapabilities],
        "peer_connectivity": NotRequired[MobPeerConnectivitySnapshot],
        "kickoff": NotRequired[dict[str, Any]],
        # Diagnostic bridge-session id for status/continuity only. Realtime
        # callers open `RealtimeChannel.mob_member(...)`; the server resolves
        # the current binding behind the stable mob-member target.
        "current_session_id": NotRequired[str],
    },
)

MobKickoffMemberSnapshot = TypedDict(
    "MobKickoffMemberSnapshot",
    {
        "agent_identity": str,
        "status": str,
        "output_preview": NotRequired[str],
        "error": NotRequired[str],
        "tokens_used": int,
        "is_final": bool,
        "peer_connectivity": NotRequired[MobPeerConnectivitySnapshot],
        "kickoff": NotRequired[dict[str, Any]],
    },
)

MobReadyMemberSnapshot = MobKickoffMemberSnapshot

MobHelperResult = TypedDict(
    "MobHelperResult",
    {
        "output": NotRequired[str],
        "tokens_used": int,
        "agent_identity": str,
        "member_ref": MobMemberRef,
    },
)

MobMember = TypedDict(
    "MobMember",
    {
        "agent_identity": str,
        "member_ref": MobMemberRef,
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
        "initial_message": NotRequired[WireContentInput | None],
        "runtime_mode": NotRequired[WireMobRuntimeMode | None],
        "backend": NotRequired[WireMobBackendKind | None],
        "labels": NotRequired[dict[str, str] | None],
        "context": NotRequired[dict[str, Any] | None],
        "additional_instructions": NotRequired[list[str] | None],
        "auth_binding": NotRequired[WireAuthBindingRef | dict[str, str] | None],
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
        initial_message: WireContentInput | None = None,
        runtime_mode: WireMobRuntimeMode | None = None,
        backend: WireMobBackendKind | None = None,
        labels: dict[str, str] | None = None,
        context: dict[str, Any] | None = None,
        additional_instructions: list[str] | None = None,
        binding: WireRuntimeBinding | dict[str, Any] | None = None,
        shell_env: dict[str, str] | None = None,
        auto_wire_parent: bool | None = None,
        launch_mode: WireMemberLaunchMode | dict[str, Any] | None = None,
        tool_access_policy: WireToolAccessPolicy | dict[str, Any] | None = None,
        budget_split_policy: WireBudgetSplitPolicy | dict[str, Any] | None = None,
        inherited_tool_filter: WireToolFilter | dict[str, list[str]] | None = None,
        override_profile: WireMobProfile | dict[str, Any] | None = None,
        auth_binding: WireAuthBindingRef | dict[str, str] | None = None,
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
            binding=binding,
            shell_env=shell_env,
            auto_wire_parent=auto_wire_parent,
            launch_mode=launch_mode,
            tool_access_policy=tool_access_policy,
            budget_split_policy=budget_split_policy,
            inherited_tool_filter=inherited_tool_filter,
            override_profile=override_profile,
            auth_binding=auth_binding,
        )

    async def spawn_many(
        self,
        specs: list[MobSpawnSpec],
    ) -> MobSpawnManyResult:
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

    async def wait_for_ready(
        self,
        *,
        member_ids: list[str] | None = None,
        timeout_ms: int | None = None,
    ) -> list[MobReadyMemberSnapshot]:
        return await self._client.wait_mob_ready(
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
    ) -> MobHelperResult:
        return await self._client.spawn_mob_helper(
            self.id,
            prompt,
            agent_identity=agent_identity,
            role_name=role_name,
        )

    async def fork_helper(
        self,
        source_member_id: str,
        prompt: str,
        *,
        agent_identity: str | None = None,
        role_name: str | None = None,
        fork_context: dict[str, Any] | None = None,
    ) -> MobHelperResult:
        return await self._client.fork_mob_helper(
            self.id,
            source_member_id,
            prompt,
            agent_identity=agent_identity,
            role_name=role_name,
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
