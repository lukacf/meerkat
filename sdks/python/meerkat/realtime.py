from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field, is_dataclass
from typing import TYPE_CHECKING, Any, Literal, TypedDict

from .errors import MeerkatError
from .generated.types import (
    RealtimeCapabilitiesResult,
    RealtimeOpenInfo,
    RealtimeOpenRequest,
    RealtimeReconnectPolicy,
    RealtimeStatusResult,
)

if TYPE_CHECKING:
    from .client import MeerkatClient


class RealtimeSessionTarget(TypedDict):
    type: Literal["session_target"]
    session_id: str


# Phase 5G/T5i: `mob_member_target` was removed from the realtime wire
# contract. Callers navigate `mob/member_status → current_session_id →
# realtime/open_info` with a `session_target` instead. The Python SDK's
# `RealtimeChannel.mob_member(...)` constructor is kept for ergonomics
# and performs that resolution internally the first time a wire call is
# made. `RealtimeChannelTarget` is the single wire-level shape that
# crosses the RPC boundary.
RealtimeChannelTarget = RealtimeSessionTarget


def _reconnect_policy_to_wire(
    policy: RealtimeReconnectPolicy | dict[str, Any] | None,
) -> dict[str, Any] | None:
    if policy is None:
        return None
    if is_dataclass(policy):
        return asdict(policy)
    return dict(policy)


def _to_wire(value: Any) -> Any:
    if is_dataclass(value):
        return asdict(value)
    if isinstance(value, dict):
        return dict(value)
    return value


class RealtimeConnection:
    """A connected realtime websocket channel."""

    def __init__(self, websocket: Any):
        self._websocket = websocket

    @classmethod
    async def open(
        cls,
        open_info: RealtimeOpenInfo,
        *,
        role: Literal["primary", "observer"] = "primary",
        turning_mode: Literal["provider_managed", "explicit_commit"] = "provider_managed",
    ) -> "RealtimeConnection":
        try:
            import websockets
        except ModuleNotFoundError as exc:  # pragma: no cover - packaging failure
            raise MeerkatError(
                "REALTIME_TRANSPORT_UNAVAILABLE",
                "Install the Python SDK websocket dependency to use RealtimeChannel.connect()",
            ) from exc

        websocket = await websockets.connect(open_info.ws_url)
        connection = cls(websocket)
        await connection.send_frame(
            {
                "type": "channel.open",
                "protocol_version": open_info.default_protocol_version,
                "open_token": open_info.open_token,
                "role": role,
                "turning_mode": turning_mode,
            }
        )

        opened = await connection.recv()
        if opened is None:
            raise MeerkatError(
                "REALTIME_OPEN_FAILED",
                "realtime websocket closed before channel.open completed",
            )
        if opened.get("type") == "channel.error":
            raise MeerkatError(
                "REALTIME_OPEN_FAILED",
                str(opened.get("message", "realtime websocket rejected channel.open")),
            )
        if opened.get("type") != "channel.opened":
            raise MeerkatError(
                "INVALID_RESPONSE",
                f"Expected channel.opened after channel.open, got {opened.get('type')!r}",
            )
        return connection

    async def send_frame(self, frame: dict[str, Any]) -> None:
        await self._websocket.send(json.dumps(frame))

    async def send_input(self, chunk: dict[str, Any] | Any) -> None:
        await self.send_frame({"type": "channel.input", "chunk": _to_wire(chunk)})

    async def commit_turn(self) -> None:
        await self.send_frame({"type": "channel.commit_turn"})

    async def interrupt(self) -> None:
        await self.send_frame({"type": "channel.interrupt"})

    async def close(self) -> None:
        await self.send_frame({"type": "channel.close"})

    async def recv(self) -> dict[str, Any] | None:
        try:
            message = await self._websocket.recv()
        except Exception as exc:  # pragma: no cover - runtime transport closes
            try:
                import websockets
            except ModuleNotFoundError:  # pragma: no cover - surfaced elsewhere
                raise
            if isinstance(exc, websockets.ConnectionClosed):
                return None
            raise

        if message is None:
            return None
        if isinstance(message, bytes):
            message = message.decode("utf-8")
        return json.loads(message)


@dataclass(frozen=True, slots=True)
class _MobMemberBinding:
    """Deferred mob-member → session resolution for RealtimeChannel.

    Phase 5G/T5i removed `mob_member_target` from the realtime wire
    contract. The SDK stores this binding when callers construct a
    channel via :meth:`RealtimeChannel.mob_member` and resolves it to
    a concrete session id lazily on the first wire call by reading
    `mob/member_status.current_session_id`.
    """

    mob_id: str
    agent_identity: str


@dataclass(slots=True)
class RealtimeChannel:
    """High-level realtime channel scaffold for public SDK callers."""

    _client: "MeerkatClient"
    # Either a resolved `session_target` dict (for direct session
    # channels and for mob-member channels after the first resolve) or
    # None when a pending mob binding must be resolved first.
    target: RealtimeSessionTarget | None = None
    role: Literal["primary", "observer"] = "primary"
    turning_mode: Literal["provider_managed", "explicit_commit"] = "provider_managed"
    reconnect_policy: RealtimeReconnectPolicy | dict[str, Any] | None = None
    # Internal: set when the channel was built via
    # :meth:`mob_member`; resolved into a `session_target` on first use.
    _mob_binding: _MobMemberBinding | None = field(default=None, repr=False)

    @classmethod
    def session(
        cls,
        client: "MeerkatClient",
        session_id: str,
        *,
        role: Literal["primary", "observer"] = "primary",
        turning_mode: Literal["provider_managed", "explicit_commit"] = "provider_managed",
        reconnect_policy: RealtimeReconnectPolicy | dict[str, Any] | None = None,
    ) -> "RealtimeChannel":
        return cls(
            _client=client,
            target={"type": "session_target", "session_id": session_id},
            role=role,
            turning_mode=turning_mode,
            reconnect_policy=reconnect_policy,
        )

    @classmethod
    def mob_member(
        cls,
        client: "MeerkatClient",
        mob_id: str,
        agent_identity: str,
        *,
        role: Literal["primary", "observer"] = "primary",
        turning_mode: Literal["provider_managed", "explicit_commit"] = "provider_managed",
        reconnect_policy: RealtimeReconnectPolicy | dict[str, Any] | None = None,
    ) -> "RealtimeChannel":
        # Defer the mob-member → session resolve until the first wire
        # call (open_info / status / capabilities). The channel target
        # that crosses the RPC boundary is always `session_target`
        # (Phase 5G/T5i).
        return cls(
            _client=client,
            target=None,
            role=role,
            turning_mode=turning_mode,
            reconnect_policy=reconnect_policy,
            _mob_binding=_MobMemberBinding(
                mob_id=mob_id, agent_identity=agent_identity
            ),
        )

    async def _resolved_target(self) -> RealtimeSessionTarget:
        if self.target is not None:
            return self.target
        binding = self._mob_binding
        if binding is None:
            raise MeerkatError(
                "INVALID_REQUEST",
                "RealtimeChannel has no target or mob binding to resolve",
            )
        status = await self._client.mob_member_status(
            mob_id=binding.mob_id, agent_identity=binding.agent_identity
        )
        session_id = status.get("current_session_id") if isinstance(status, dict) else None
        if not isinstance(session_id, str) or not session_id:
            raise MeerkatError(
                "INVALID_RESPONSE",
                "mob/member_status did not surface current_session_id for "
                f"mob {binding.mob_id!r} agent {binding.agent_identity!r}; "
                "the member may not yet be bound to a session",
            )
        resolved: RealtimeSessionTarget = {
            "type": "session_target",
            "session_id": session_id,
        }
        # Cache the resolved target so subsequent calls do not re-resolve.
        self.target = resolved
        return resolved

    def open_request(self) -> RealtimeOpenRequest:
        if self.target is None:
            raise MeerkatError(
                "INVALID_REQUEST",
                "RealtimeChannel.open_request() requires a resolved target; "
                "call await channel.open_info()/status()/capabilities()/connect() "
                "first so the mob-member binding resolves to a session_target.",
            )
        return RealtimeOpenRequest(
            target=dict(self.target),
            role=self.role,
            turning_mode=self.turning_mode,
            reconnect_policy=_reconnect_policy_to_wire(self.reconnect_policy),
        )

    async def open_info(self) -> RealtimeOpenInfo:
        target = await self._resolved_target()
        request = RealtimeOpenRequest(
            target=dict(target),
            role=self.role,
            turning_mode=self.turning_mode,
            reconnect_policy=_reconnect_policy_to_wire(self.reconnect_policy),
        )
        return await self._client.realtime_open_info(request)

    async def status(self) -> RealtimeStatusResult:
        target = await self._resolved_target()
        return await self._client.realtime_status({"target": dict(target)})

    async def capabilities(self) -> RealtimeCapabilitiesResult:
        target = await self._resolved_target()
        return await self._client.realtime_capabilities({"target": dict(target)})

    async def connect_with_open_info(self, open_info: RealtimeOpenInfo) -> RealtimeConnection:
        target = await self._resolved_target()
        if dict(target) != dict(open_info.target):
            raise MeerkatError(
                "INVALID_RESPONSE",
                "realtime/open_info returned a target that does not match the requested channel target",
            )
        return await RealtimeConnection.open(
            open_info,
            role=self.role,
            turning_mode=self.turning_mode,
        )

    async def connect(self) -> RealtimeConnection:
        return await self.connect_with_open_info(await self.open_info())
