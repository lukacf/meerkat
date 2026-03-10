import asyncio
from pathlib import Path

import pytest


FAKE_RPC = Path(__file__).resolve().parents[2] / "test-fixtures" / "fake_stream_rpc.mjs"


@pytest.mark.asyncio
async def test_python_sdk_buffered_stream_flushes_in_order_and_remote_end_is_visible():
    from meerkat import MeerkatClient

    async with MeerkatClient(str(FAKE_RPC)) as client:
        sub = await client.subscribe_session_events("buffered-session")
        deltas = []
        async for event in sub:
            deltas.append(getattr(event.payload, "delta", None))

        assert deltas == ["hi", "there"]
        assert sub.terminal_outcome is not None
        assert sub.terminal_outcome.get("outcome") == "remote_end"


@pytest.mark.asyncio
async def test_python_sdk_terminal_error_stream_is_visible_without_manual_close():
    from meerkat import MeerkatClient

    async with MeerkatClient(str(FAKE_RPC)) as client:
        sub = await client.subscribe_session_events("terminal-error-session")
        events = []
        async for event in sub:
            events.append(event)

        assert events == []
        assert sub.terminal_outcome is not None
        assert sub.terminal_outcome.get("outcome") == "terminal_error"
        assert sub.terminal_outcome.get("error", {}).get("code") == "stream_queue_overflow"
