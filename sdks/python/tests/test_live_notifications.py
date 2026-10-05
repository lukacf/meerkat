"""Channel-scoped live/* notifications reach on_live_notification callbacks."""

import asyncio
import json

import pytest

from meerkat import LiveNotification, MeerkatClient, parse_live_notification
from meerkat.generated.types import (
    LiveAssistantOutputAvailableParams,
    LiveAssistantPlaybackHintParams,
    LiveMediaHealthRequestedParams,
)
from meerkat.streaming import _StdoutDispatcher


def _reader(lines: list[dict]) -> asyncio.StreamReader:
    reader = asyncio.StreamReader()
    for line in lines:
        reader.feed_data((json.dumps(line) + "\n").encode())
    reader.feed_eof()
    return reader


def _notification(method: str, params: dict) -> dict:
    return {"jsonrpc": "2.0", "method": method, "params": params}


def test_parse_live_notification_types_the_known_methods():
    assert parse_live_notification(
        "live/assistant_playback_hint", {"channel_id": "ch-1", "hint": "duck"}
    ) == LiveNotification(
        method="live/assistant_playback_hint",
        params=LiveAssistantPlaybackHintParams(channel_id="ch-1", hint="duck"),
    )
    assert parse_live_notification(
        "live/media_health_requested", {"channel_id": "ch-1", "output_id": "out-1"}
    ) == LiveNotification(
        method="live/media_health_requested",
        params=LiveMediaHealthRequestedParams(channel_id="ch-1", output_id="out-1"),
    )
    assert parse_live_notification(
        "live/assistant_output_available",
        {"channel_id": "ch-1", "output_id": "out-2", "content_index": 0},
    ) == LiveNotification(
        method="live/assistant_output_available",
        params=LiveAssistantOutputAvailableParams(
            channel_id="ch-1", output_id="out-2", content_index=0
        ),
    )
    # Unknown methods and malformed payloads are ignored, never misread.
    assert parse_live_notification("live/some_future_notification", {"channel_id": "c"}) is None
    assert (
        parse_live_notification("live/assistant_playback_hint", {"channel_id": "c", "hint": "up"})
        is None
    )
    assert parse_live_notification("live/assistant_playback_hint", {"hint": "duck"}) is None
    assert (
        parse_live_notification(
            "live/assistant_output_available",
            {"channel_id": "c", "output_id": "o", "content_index": True},
        )
        is None
    )


@pytest.mark.asyncio
async def test_dispatcher_routes_live_notifications_to_the_sink_not_a_session_queue():
    seen: list[tuple[str, dict]] = []
    reader = _reader(
        [
            _notification("live/assistant_playback_hint", {"channel_id": "ch-1", "hint": "duck"}),
            _notification(
                "live/assistant_playback_hint", {"channel_id": "ch-1", "hint": "restore"}
            ),
            # A sentinel response orders the assertions after both lines.
            {"jsonrpc": "2.0", "id": 7, "result": {}},
        ]
    )
    dispatcher = _StdoutDispatcher(reader)
    dispatcher.set_live_notification_sink(lambda method, params: seen.append((method, params)))
    dispatcher.start()
    await asyncio.wait_for(dispatcher.expect_response(7), timeout=1.0)
    assert seen == [
        ("live/assistant_playback_hint", {"channel_id": "ch-1", "hint": "duck"}),
        ("live/assistant_playback_hint", {"channel_id": "ch-1", "hint": "restore"}),
    ]
    assert dispatcher._event_queues == {}
    assert dispatcher._unmatched_buffer == {}
    await dispatcher.stop()


def test_client_callbacks_receive_typed_notifications_and_unsubscribe():
    client = MeerkatClient()
    seen: list[LiveNotification] = []

    def faulty(_notification: LiveNotification) -> None:
        raise RuntimeError("callback fault")

    client.on_live_notification(faulty)
    unsubscribe = client.on_live_notification(seen.append)
    client._dispatch_live_notification(
        "live/assistant_playback_hint", {"channel_id": "ch-1", "hint": "duck"}
    )
    client._dispatch_live_notification("live/some_future_notification", {"channel_id": "ch-1"})
    assert seen == [
        LiveNotification(
            method="live/assistant_playback_hint",
            params=LiveAssistantPlaybackHintParams(channel_id="ch-1", hint="duck"),
        )
    ]
    unsubscribe()
    client._dispatch_live_notification(
        "live/assistant_playback_hint", {"channel_id": "ch-1", "hint": "restore"}
    )
    assert len(seen) == 1
