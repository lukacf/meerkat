"""Exercise public operations at HTTPX's automatic close boundary.

This uses an in-memory HTTPX transport and an observed underlying close gate.
It neither contacts a provider nor executes native code.
"""

import asyncio
import json

import httpx
from meerkat_toolkit.httpx_client import prepare_httpx_client
import pytest

from meerkat_toolkit.connectors.gmail import GmailOperations
from meerkat_toolkit.connectors.google_calendar import GoogleCalendarOperations
from meerkat_toolkit.connectors.home_assistant import HomeAssistantOperations
from meerkat_toolkit.connectors.slack import SlackOperations


class GatedClose(httpx.AsyncByteStream):
    def __init__(self, payload):
        self.payload = json.dumps(payload).encode()
        self.entered = asyncio.Event()
        self.release = asyncio.Event()
        self.closed = False
        self.close_entries = 0

    async def __aiter__(self):
        yield self.payload

    async def aclose(self):
        self.close_entries += 1
        self.entered.set()
        await self.release.wait()
        self.closed = True


PAYLOADS = {
    "gmail": {"id": "message1", "threadId": "thread1"},
    "calendar": {"id": "event1", "etag": '"version1"', "updated": "2026-10-01T00:00:00Z"},
    "home_assistant": [],
    "slack": {"ok": True, "messages": [], "has_more": False},
}


def invocation(provider, client, timeout):
    if provider == "gmail":
        return GmailOperations(client, user_id="me", timeout_seconds=timeout).send_raw(
            b"To: fixture@example.test\r\nSubject: Fixture\r\n\r\nBody"
        )
    if provider == "calendar":
        return GoogleCalendarOperations(
            client, calendar_id="selected-calendar", timeout_seconds=timeout
        ).insert_event({"summary": "Fixture"})
    if provider == "home_assistant":
        return HomeAssistantOperations(client, timeout_seconds=timeout).call_service(
            "light", "turn_on", data={"entity_id": "light.fixture"}
        )
    return SlackOperations(client, timeout_seconds=timeout).history_page("C123")


@pytest.mark.asyncio
@pytest.mark.parametrize("provider", PAYLOADS)
@pytest.mark.parametrize("mode", ["success", "cancel", "deadline"])
async def test_operation_retains_underlying_close(provider, mode):
    stream = GatedClose(PAYLOADS[provider])
    requests = []

    async def handle(request):
        requests.append((request.method, str(request.url)))
        return httpx.Response(200, headers={"Content-Type": "application/json"}, stream=stream)

    error = None
    async with prepare_httpx_client(httpx.AsyncClient(
        base_url="http://fixture.invalid/", transport=httpx.MockTransport(handle), trust_env=False
    )) as client:
        timeout = 0.02 if mode == "deadline" else 2
        task = asyncio.create_task(invocation(provider, client, timeout))
        try:
            await asyncio.wait_for(stream.entered.wait(), 1)
            if mode == "cancel":
                task.cancel()
                await asyncio.sleep(0)
                task.cancel()
            done, _ = await asyncio.wait({task}, timeout=0.06 if mode == "deadline" else 0.02)
            completed_before_release = bool(done)
            stream.release.set()
            try:
                await asyncio.wait_for(task, 1)
            except BaseException as exc:
                error = exc
            closed_when_operation_returned = stream.closed
            client_still_borrowed = not client.is_closed
        finally:
            stream.release.set()
            if not task.done():
                task.cancel()
                try:
                    await task
                except BaseException:
                    pass
            # Fixture teardown happens after observing operation custody.
            if not stream.closed:
                await stream.aclose()
    assert len(requests) == 1
    assert client_still_borrowed
    if mode == "success":
        assert error is None
    elif mode == "cancel":
        assert isinstance(error, asyncio.CancelledError)
    else:
        assert error is not None and error.kind.value == "timeout"
    if mode != "success" and provider != "slack":
        assert error.write_outcome.value == "unknown"
    assert not completed_before_release, "operation returned before underlying close was released"
    assert closed_when_operation_returned, "operation returned before underlying close completed"
