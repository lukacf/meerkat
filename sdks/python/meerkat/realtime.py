from __future__ import annotations

import json
from dataclasses import asdict, dataclass, is_dataclass
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


class RealtimeMobMemberTarget(TypedDict):
    type: Literal["mob_member_target"]
    mob_id: str
    agent_identity: str


RealtimeChannelTarget = RealtimeSessionTarget | RealtimeMobMemberTarget


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
class RealtimeChannel:
    """High-level realtime channel scaffold for public SDK callers."""

    _client: "MeerkatClient"
    target: RealtimeChannelTarget
    role: Literal["primary", "observer"] = "primary"
    turning_mode: Literal["provider_managed", "explicit_commit"] = "provider_managed"
    reconnect_policy: RealtimeReconnectPolicy | dict[str, Any] | None = None

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
        return cls(
            _client=client,
            target={
                "type": "mob_member_target",
                "mob_id": mob_id,
                "agent_identity": agent_identity,
            },
            role=role,
            turning_mode=turning_mode,
            reconnect_policy=reconnect_policy,
        )

    def open_request(self) -> RealtimeOpenRequest:
        return RealtimeOpenRequest(
            target=dict(self.target),
            role=self.role,
            turning_mode=self.turning_mode,
            reconnect_policy=_reconnect_policy_to_wire(self.reconnect_policy),
        )

    async def open_info(self) -> RealtimeOpenInfo:
        return await self._client.realtime_open_info(self.open_request())

    async def status(self) -> RealtimeStatusResult:
        return await self._client.realtime_status({"target": dict(self.target)})

    async def capabilities(self) -> RealtimeCapabilitiesResult:
        return await self._client.realtime_capabilities({"target": dict(self.target)})

    async def connect_with_open_info(self, open_info: RealtimeOpenInfo) -> RealtimeConnection:
        if dict(self.target) != dict(open_info.target):
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
