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
        "member_id": str,
        "session_id": str,
        "handling_mode": Literal["queue", "steer"],
    },
)

MemberRespawnReceipt = TypedDict(
    "MemberRespawnReceipt",
    {
        "member_id": str,
        "old_session_id": NotRequired[str | None],
        "new_session_id": NotRequired[str | None],
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
        "current_session_id": NotRequired[str],
        "peer_connectivity": NotRequired[MobPeerConnectivitySnapshot],
    },
)

MobHelperResult = TypedDict(
    "MobHelperResult",
    {
        "output": NotRequired[str],
        "tokens_used": int,
        "session_id": NotRequired[str],
    },
)


class Member:
    """Capability-bearing mob member handle."""

    def __init__(self, mob: "Mob", meerkat_id: str):
        self._mob = mob
        self.meerkat_id = meerkat_id

    async def send(
        self,
        content: str | list[dict[str, Any]],
        *,
        handling_mode: Literal["queue", "steer"] = "queue",
        render_metadata: RenderMetadata | None = None,
    ) -> MemberDeliveryReceipt:
        return await self._mob._client.send_mob_member_content(
            self._mob.id,
            self.meerkat_id,
            content,
            handling_mode=handling_mode,
            render_metadata=render_metadata,
        )

    async def events(self) -> EventSubscription:
        return await self._mob.subscribe_member_events(self.meerkat_id)


class Mob:
    """First-class mob handle backed by explicit RPC methods."""


    def __init__(self, client: Any, mob_id: str):
        self._client = client
        self.id = mob_id

    async def status(self) -> dict[str, Any]:
        return await self._client.mob_status(self.id)

    async def members(self) -> list[dict[str, Any]]:
        return await self._client.list_mob_members(self.id)

    async def spawn(
        self,
        *,
        profile: str,
        meerkat_id: str,
        initial_message: str | list[dict] | None = None,
        runtime_mode: str | None = None,
        backend: str | None = None,
        resume_session_id: str | None = None,
        labels: dict[str, str] | None = None,
        context: dict[str, Any] | None = None,
        additional_instructions: list[str] | None = None,
    ) -> dict[str, Any]:
        return await self._client.spawn_mob_member(
            self.id,
            profile=profile,
            meerkat_id=meerkat_id,
            initial_message=initial_message,
            runtime_mode=runtime_mode,
            backend=backend,
            resume_session_id=resume_session_id,
            labels=labels,
            context=context,
            additional_instructions=additional_instructions,
        )

    async def retire(self, meerkat_id: str) -> None:
        await self._client.retire_mob_member(self.id, meerkat_id)

    async def respawn(
        self,
        meerkat_id: str,
        initial_message: str | list[dict[str, Any]] | None = None,
    ) -> MobRespawnResult:
        return await self._client.respawn_mob_member(self.id, meerkat_id, initial_message)

    async def force_cancel(self, meerkat_id: str) -> None:
        await self._client.force_cancel_mob_member(self.id, meerkat_id)

    async def member_status(self, meerkat_id: str) -> MobMemberSnapshot:
        return await self._client.mob_member_status(self.id, meerkat_id)

    async def spawn_helper(
        self,
        prompt: str,
        *,
        meerkat_id: str | None = None,
        profile_name: str | None = None,
    ) -> MobHelperResult:
        return await self._client.spawn_mob_helper(
            self.id,
            prompt,
            meerkat_id=meerkat_id,
            profile_name=profile_name,
        )

    async def fork_helper(
        self,
        source_member_id: str,
        prompt: str,
        *,
        meerkat_id: str | None = None,
        profile_name: str | None = None,
        fork_context: dict[str, Any] | None = None,
    ) -> MobHelperResult:
        return await self._client.fork_mob_helper(
            self.id,
            source_member_id,
            prompt,
            meerkat_id=meerkat_id,
            profile_name=profile_name,
            fork_context=fork_context,
        )

    async def wire(self, member: str, peer: str | dict[str, Any]) -> None:
        await self._client.wire_mob_members(self.id, member, peer)

    async def unwire(self, member: str, peer: str | dict[str, Any]) -> None:
        await self._client.unwire_mob_members(self.id, member, peer)

    async def lifecycle(self, action: str) -> None:
        await self._client.mob_lifecycle(self.id, action)

    def member(self, meerkat_id: str) -> Member:
        return Member(self, meerkat_id)

    async def append_system_context(
        self,
        meerkat_id: str,
        text: str,
        *,
        source: str | None = None,
        idempotency_key: str | None = None,
    ) -> dict[str, Any]:
        return await self._client.append_mob_system_context(
            self.id,
            meerkat_id,
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

    async def subscribe_member_events(self, meerkat_id: str) -> EventSubscription:
        """Subscribe to events for a single mob member."""
        return await self._client.subscribe_mob_member_events(self.id, meerkat_id)

    async def subscribe_events(self) -> EventSubscription:
        """Subscribe to attributed mob-wide events."""
        return await self._client.subscribe_mob_events(self.id)

    async def subscribe_member(self, meerkat_id: str) -> EventSubscription:
        return await self.subscribe_member_events(meerkat_id)

    async def subscribe_all(self) -> EventSubscription:
        return await self.subscribe_events()
