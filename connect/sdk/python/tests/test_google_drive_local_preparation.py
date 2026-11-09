import asyncio
import os
import threading
from pathlib import Path

import httpx
import pytest
from meerkat_toolkit.connectors.google_drive import GoogleDriveOperations
from meerkat_toolkit.httpx_client import prepare_httpx_client


@pytest.mark.asyncio
@pytest.mark.parametrize("mode", ["cancel", "deadline"])
@pytest.mark.parametrize("operation", ["upload", "download"])
async def test_cancelled_file_preparation_does_not_start_auth_or_request(tmp_path, monkeypatch, mode, operation):
    source = tmp_path / "source.bin"
    source.write_bytes(b"upload fixture")
    output = tmp_path / "destination.bin"
    output.write_bytes(b"prior destination")
    entered, release = threading.Event(), threading.Event()
    original_open = os.open
    observed = []
    def held_open(path, flags, *args, **kwargs):
        selected = Path(path) == source if operation == "upload" else Path(path).name.startswith(".toolkit-drive-")
        if selected:
            entered.set()
            assert release.wait(2), "test did not release file allocation worker"
        return original_open(path, flags, *args, **kwargs)
    monkeypatch.setattr(os, "open", held_open)
    class Auth(httpx.Auth):
        def auth_flow(self, request):
            observed.append("auth")
            yield request
    async def handler(request):
        observed.append("request")
        return httpx.Response(200, json={"id": "created"})
    async with prepare_httpx_client(httpx.AsyncClient(base_url="http://fixture.invalid/", transport=httpx.MockTransport(handler), auth=Auth(), trust_env=False)) as client:
        ops = GoogleDriveOperations(client, timeout_seconds=0.04 if mode == "deadline" else 2)
        call = ops.create_file({}, upload_path=source) if operation == "upload" else ops.download_file("file1", output)
        task = asyncio.create_task(call)
        error = None
        try:
            await asyncio.wait_for(asyncio.to_thread(entered.wait), 1)
            if mode == "cancel":
                task.cancel()
                await asyncio.sleep(0)
            else:
                await asyncio.sleep(0.07)
            pending_before_release = not task.done()
            release.set()
            try:
                await asyncio.wait_for(task, 2)
            except BaseException as exc:
                error = exc
            assert pending_before_release
            assert observed == [], "known cancellation during preparation still entered HTTPX auth/send"
            assert error is not None
            assert output.read_bytes() == b"prior destination"
            assert not list(tmp_path.glob(".toolkit-drive-*"))
            assert not client.is_closed
        finally:
            release.set()
            if not task.done():
                task.cancel()
            await asyncio.gather(task, return_exceptions=True)
