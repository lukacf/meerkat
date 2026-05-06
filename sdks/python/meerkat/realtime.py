from __future__ import annotations

import asyncio
import json
from dataclasses import asdict, dataclass, is_dataclass
from typing import TYPE_CHECKING, Any, Literal

from .errors import MeerkatError
from .generated.types import (
    RealtimeCapabilitiesResult,
    RealtimeChannelTarget,
    RealtimeChannelTargetMobMember,
    RealtimeChannelTargetSessionTarget,
    RealtimeInputChunk,
    RealtimeOpenInfo,
    RealtimeOpenRequest,
    RealtimeReconnectPolicy,
    RealtimeStatusResult,
)

if TYPE_CHECKING:
    from .client import MeerkatClient


# W3-H: `RealtimeChannelTarget` carries identity as a first-class wire
# fact for mob-member channels via the `mob_member` variant. The server
# resolves `(mob_id, agent_identity)` against the MobMachine's
# canonical `member_session_bindings` map on every tick, so respawn
# atomically rotates the bridge session without any SDK round-trip and
# without any client-side session-id pin. A terminal retire surfaces
# as `RealtimeErrorCode::BindingReleased`.
RealtimeSessionTarget = RealtimeChannelTargetSessionTarget
RealtimeMobMemberTarget = RealtimeChannelTargetMobMember


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


def _status_state(status: Any) -> str | None:
    if isinstance(status, dict):
        state = status.get("state")
    else:
        state = getattr(status, "state", None)
    return state if isinstance(state, str) else None


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

    async def send_input(self, chunk: RealtimeInputChunk | dict[str, Any] | Any) -> None:
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


@dataclass(slots=True)
class RealtimeChannel:
    """High-level realtime channel scaffold for public SDK callers."""

    _client: "MeerkatClient"
    # The wire target that crosses the RPC boundary. Carries identity
    # directly for `mob_member` channels (W3-H); the server resolves the
    # current bridge session on every tick from the MobMachine's
    # canonical member-session map, so the SDK never pins a session id.
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
        # W3-H: identity is a first-class wire fact. The channel target
        # is `mob_member { mob_id, agent_identity }`; the server
        # resolves the current bridge session from the MobMachine's
        # canonical member-session map on every tick, so respawn
        # rotates the bridge session without any SDK round-trip or
        # session-id pin.
        return cls(
            _client=client,
            target={
                "type": "mob_member",
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

    async def connect(
        self,
        *,
        wait_for_attachment: bool = True,
        attachment_timeout_secs: float = 30.0,
    ) -> RealtimeConnection:
        """Connect the realtime channel.

        When *wait_for_attachment* is True (the default), polls the channel
        status until the runtime reports the transport is bound — ``state``
        reaches ``"ready"``.  Without this, input sent immediately after
        connect may be lost when the runtime hasn't finished binding the
        transport (e.g. after a member respawn).
        """
        conn = await self.connect_with_open_info(await self.open_info())
        if wait_for_attachment:
            attached = False
            deadline = asyncio.get_running_loop().time() + attachment_timeout_secs
            while asyncio.get_running_loop().time() < deadline:
                try:
                    st = await self.status()
                except Exception:
                    await asyncio.sleep(0.25)
                    continue
                if _status_state(getattr(st, "status", None)) == "ready":
                    attached = True
                    break
                await asyncio.sleep(0.25)
            if not attached:
                await conn.close()
                raise MeerkatError(
                    "REALTIME_ATTACHMENT_TIMEOUT",
                    f"realtime transport did not reach ready state within {attachment_timeout_secs}s",
                )
        return conn
