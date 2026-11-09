"""Drive preserves actual send-entry evidence across preparation failures."""

from __future__ import annotations

import httpx
import pytest

from meerkat_toolkit.connectors import _httpx_stream
from meerkat_toolkit.connectors.google_drive import (
    DriveError,
    FailureKind,
    GoogleDriveOperations,
    WriteOutcome,
)
from meerkat_toolkit.httpx_client import HttpxClientNotPrepared, prepare_httpx_client


@pytest.mark.asyncio
@pytest.mark.parametrize("stage", ["pre_send", "hook", "body", "close"])
async def test_preparation_error_type_does_not_erase_actual_send_entry(stage, monkeypatch):
    calls, auth_calls = [], []

    class Auth(httpx.Auth):
        def auth_flow(self, request):
            auth_calls.append(request)
            yield request

    class Body(httpx.AsyncByteStream):
        closed = False

        async def __aiter__(self):
            if stage == "body":
                raise HttpxClientNotPrepared()
            yield b'{"id":"new"}'

        async def aclose(self):
            self.closed = True
            if stage == "close":
                raise HttpxClientNotPrepared()

    stream = Body()

    async def hook(response):
        if stage == "hook":
            raise HttpxClientNotPrepared()

    async def provider(request):
        calls.append(request)
        return httpx.Response(200, stream=stream)

    async with prepare_httpx_client(
        httpx.AsyncClient(
            base_url="http://fixture.invalid/",
            transport=httpx.MockTransport(provider),
            auth=Auth(),
            event_hooks={"response": [hook]},
        )
    ) as client:
        if stage == "pre_send":
            # The actual helper guard, before it can create the send task.
            # Drive's separate operation-entry guard has already been imported.
            def refusal(client):
                raise HttpxClientNotPrepared()

            monkeypatch.setattr(_httpx_stream, "require_prepared_httpx_client", refusal)
        with pytest.raises(DriveError) as caught:
            await GoogleDriveOperations(client).create_file({"name": "new"})
        assert caught.value.kind is FailureKind.CLIENT
        assert caught.value.write_outcome is (
            WriteOutcome.NOT_ATTEMPTED if stage == "pre_send" else WriteOutcome.UNKNOWN
        )
        assert len(calls) == len(auth_calls) == (0 if stage == "pre_send" else 1)
        assert stream.closed is (stage != "pre_send")
        assert not client.is_closed
