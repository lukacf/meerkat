from __future__ import annotations

import json

import pytest

from meerkat import MeerkatClient, RealtimeChannel
from meerkat.generated.types import (
    RealtimeOpenInfo,
    RealtimeProtocolVersion,
    RealtimeStatusResult,
)

REALTIME_PROTOCOL_VERSION: RealtimeProtocolVersion = "2"


def test_python_sdk_connect_args_enable_realtime_ws_host() -> None:
    args = MeerkatClient._build_args(  # noqa: SLF001
        False,
        isolated=False,
        realm_id=None,
        instance_id=None,
        realm_backend=None,
        state_root=None,
        context_root=None,
        user_config_root=None,
    )
    assert args[:2] == ["--realtime-ws", "127.0.0.1:0"]


@pytest.mark.asyncio
async def test_realtime_channel_connects_and_exchanges_frames() -> None:
    import websockets

    seen: list[dict[str, object]] = []

    async def handler(websocket) -> None:
        open_frame = json.loads(await websocket.recv())
        seen.append(open_frame)
        await websocket.send(
            json.dumps(
                {
                    "type": "channel.opened",
                    "protocol_version": REALTIME_PROTOCOL_VERSION,
                    "status": {"state": "ready", "attempt_count": 0},
                    "capabilities": {
                        "input_kinds": ["text"],
                        "output_kinds": ["text"],
                        "turning_modes": ["provider_managed"],
                        "interrupt_supported": True,
                        "transcript_supported": True,
                        "tool_lifecycle_events_supported": False,
                        "video_supported": False,
                    },
                    "role": "primary",
                }
            )
        )

        seen.append(json.loads(await websocket.recv()))
        await websocket.send(
            json.dumps(
                {
                    "type": "channel.event",
                    "event": {"type": "output_text_delta", "delta": "world"},
                }
            )
        )
        seen.append(json.loads(await websocket.recv()))
        seen.append(json.loads(await websocket.recv()))
        seen.append(json.loads(await websocket.recv()))
        await websocket.send(json.dumps({"type": "channel.closed", "reason": "done"}))

    async with websockets.serve(handler, "127.0.0.1", 0) as server:
        port = server.sockets[0].getsockname()[1]
        client = MeerkatClient()

        async def fake_open_info(_request):
            return RealtimeOpenInfo(
                ws_url=f"ws://127.0.0.1:{port}/realtime/ws",
                open_token="token-1",
                expires_at="2026-04-15T12:00:00Z",
                target={"type": "session_target", "session_id": "session-1"},
                supported_protocol_versions=[REALTIME_PROTOCOL_VERSION],
                default_protocol_version=REALTIME_PROTOCOL_VERSION,
                capabilities={
                    "input_kinds": ["text"],
                    "output_kinds": ["text"],
                    "turning_modes": ["provider_managed"],
                    "interrupt_supported": True,
                    "transcript_supported": True,
                    "tool_lifecycle_events_supported": False,
                    "video_supported": False,
                },
            )

        client.realtime_open_info = fake_open_info  # type: ignore[method-assign]

        channel = RealtimeChannel.session(client, "session-1")
        connection = await channel.connect(wait_for_attachment=False)
        await connection.send_input({"kind": "text_chunk", "text": "hello"})
        frame = await connection.recv()
        assert frame["type"] == "channel.event"
        assert frame["event"]["type"] == "output_text_delta"
        assert frame["event"]["delta"] == "world"

        await connection.commit_turn()
        await connection.interrupt()
        await connection.close()
        closed = await connection.recv()
        assert closed is not None
        assert closed["type"] == "channel.closed"

        assert seen == [
            {
                "type": "channel.open",
                "protocol_version": REALTIME_PROTOCOL_VERSION,
                "open_token": "token-1",
                "role": "primary",
                "turning_mode": "provider_managed",
            },
            {
                "type": "channel.input",
                "chunk": {"kind": "text_chunk", "text": "hello"},
            },
            {"type": "channel.commit_turn"},
            {"type": "channel.interrupt"},
            {"type": "channel.close"},
        ]


@pytest.mark.asyncio
async def test_realtime_channel_connects_from_supplied_open_info() -> None:
    import websockets

    seen: list[dict[str, object]] = []

    async def handler(websocket) -> None:
        open_frame = json.loads(await websocket.recv())
        seen.append(open_frame)
        await websocket.send(
            json.dumps(
                {
                    "type": "channel.opened",
                    "protocol_version": REALTIME_PROTOCOL_VERSION,
                    "status": {"state": "ready", "attempt_count": 0},
                    "capabilities": {
                        "input_kinds": ["text"],
                        "output_kinds": ["text"],
                        "turning_modes": ["provider_managed"],
                        "interrupt_supported": True,
                        "transcript_supported": True,
                        "tool_lifecycle_events_supported": False,
                        "video_supported": False,
                    },
                    "role": "primary",
                }
            )
        )
        await websocket.send(json.dumps({"type": "channel.closed", "reason": "done"}))

    async with websockets.serve(handler, "127.0.0.1", 0) as server:
        port = server.sockets[0].getsockname()[1]
        client = MeerkatClient()

        async def unexpected_open_info(_request):
            raise AssertionError("channel.connect_with_open_info should not fetch bootstrap")

        client.realtime_open_info = unexpected_open_info  # type: ignore[method-assign]

        channel = RealtimeChannel.session(client, "session-1")
        connection = await channel.connect_with_open_info(
            RealtimeOpenInfo(
                ws_url=f"ws://127.0.0.1:{port}/realtime/ws",
                open_token="token-2",
                expires_at="2026-04-15T12:00:00Z",
                target={"type": "session_target", "session_id": "session-1"},
                supported_protocol_versions=[REALTIME_PROTOCOL_VERSION],
                default_protocol_version=REALTIME_PROTOCOL_VERSION,
                capabilities={
                    "input_kinds": ["text"],
                    "output_kinds": ["text"],
                    "turning_modes": ["provider_managed"],
                    "interrupt_supported": True,
                    "transcript_supported": True,
                    "tool_lifecycle_events_supported": False,
                    "video_supported": False,
                },
            )
        )

        closed = await connection.recv()
        assert closed is not None
        assert closed["type"] == "channel.closed"
        assert seen == [
            {
                "type": "channel.open",
                "protocol_version": REALTIME_PROTOCOL_VERSION,
                "open_token": "token-2",
                "role": "primary",
                "turning_mode": "provider_managed",
            }
        ]


@pytest.mark.asyncio
async def test_realtime_channel_connect_wait_accepts_wire_dict_status() -> None:
    import websockets

    async def handler(websocket) -> None:
        await websocket.recv()
        await websocket.send(
            json.dumps(
                {
                    "type": "channel.opened",
                    "protocol_version": REALTIME_PROTOCOL_VERSION,
                    "status": {"state": "ready", "attempt_count": 0},
                    "capabilities": {
                        "input_kinds": ["text"],
                        "output_kinds": ["text"],
                        "turning_modes": ["provider_managed"],
                        "interrupt_supported": True,
                        "transcript_supported": True,
                        "tool_lifecycle_events_supported": False,
                        "video_supported": False,
                    },
                    "role": "primary",
                }
            )
        )
        await websocket.recv()

    async with websockets.serve(handler, "127.0.0.1", 0) as server:
        port = server.sockets[0].getsockname()[1]
        client = MeerkatClient()

        async def fake_open_info(_request):
            return RealtimeOpenInfo(
                ws_url=f"ws://127.0.0.1:{port}/realtime/ws",
                open_token="token-3",
                expires_at="2026-04-15T12:00:00Z",
                target={"type": "session_target", "session_id": "session-1"},
                supported_protocol_versions=[REALTIME_PROTOCOL_VERSION],
                default_protocol_version=REALTIME_PROTOCOL_VERSION,
                capabilities={
                    "input_kinds": ["text"],
                    "output_kinds": ["text"],
                    "turning_modes": ["provider_managed"],
                    "interrupt_supported": True,
                    "transcript_supported": True,
                    "tool_lifecycle_events_supported": False,
                    "video_supported": False,
                },
            )

        async def fake_status(_params):
            return RealtimeStatusResult(status={"state": "ready", "attempt_count": 0})

        client.realtime_open_info = fake_open_info  # type: ignore[method-assign]
        client.realtime_status = fake_status  # type: ignore[method-assign]

        channel = RealtimeChannel.session(client, "session-1")
        connection = await channel.connect(attachment_timeout_secs=1.0)
        await connection.close()
