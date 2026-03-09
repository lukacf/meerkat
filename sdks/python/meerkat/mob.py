from __future__ import annotations

from typing import Any

from .streaming import EventSubscription


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
        initial_message: str | None = None,
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

    async def respawn(self, meerkat_id: str, initial_message: str | None = None) -> None:
        await self._client.respawn_mob_member(self.id, meerkat_id, initial_message)

    async def wire(self, a: str, b: str) -> None:
        await self._client.wire_mob_members(self.id, a, b)

    async def unwire(self, a: str, b: str) -> None:
        await self._client.unwire_mob_members(self.id, a, b)

    async def lifecycle(self, action: str) -> None:
        await self._client.mob_lifecycle(self.id, action)

    async def send_message(self, meerkat_id: str, message: str) -> None:
        await self._client.send_mob_message(self.id, meerkat_id, message)

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
