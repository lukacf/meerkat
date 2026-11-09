"""Operation input capture remains exact across cooperative suspension."""

import asyncio
import threading

import httpx
import pytest
from meerkat_toolkit.httpx_client import prepare_httpx_client

from meerkat_toolkit.connectors import google_drive as module
from meerkat_toolkit.connectors.google_drive import DriveWriteCancelled, GoogleDriveOperations


@pytest.mark.asyncio
@pytest.mark.parametrize("mode", ["upload", "download"])
async def test_relative_file_target_is_captured_before_first_io(tmp_path, monkeypatch, mode):
    selected, later = tmp_path / "selected", tmp_path / "later"
    selected.mkdir()
    later.mkdir()
    (selected / "file.bin").write_bytes(b"selected bytes")
    (later / "file.bin").write_bytes(b"other bytes")
    monkeypatch.chdir(selected)
    entered, release = asyncio.Event(), asyncio.Event()
    original_io = module._io
    first = True

    async def gate(function, *args):
        nonlocal first
        if first:
            first = False
            entered.set()
            await release.wait()
        return await original_io(function, *args)

    monkeypatch.setattr(module, "_io", gate)
    uploads = []

    async def provider(request):
        if request.method == "POST":
            uploads.append(await request.aread())
            return httpx.Response(200, json={"name": "file.bin"})
        return httpx.Response(200, content=b"downloaded bytes")

    async with httpx.AsyncClient(
        base_url="http://fixture.invalid/", transport=httpx.MockTransport(provider), trust_env=False
    ) as client:
        prepare_httpx_client(client)
        ops = GoogleDriveOperations(client)
        call = (
            ops.create_file({"name": "file.bin"}, upload_path="file.bin")
            if mode == "upload"
            else ops.download_file("id", "file.bin")
        )
        task = asyncio.create_task(call)
        try:
            await asyncio.wait_for(entered.wait(), 1)
            monkeypatch.chdir(later)
            release.set()
            result = await task
        finally:
            release.set()
            if not task.done():
                task.cancel()
            await asyncio.gather(task, return_exceptions=True)
    if mode == "upload":
        assert len(uploads) == 1
        assert b"selected bytes" in uploads[0]
        assert b"other bytes" not in uploads[0]
    else:
        assert result == {"output_path": "file.bin", "bytes": 16}
        assert (selected / "file.bin").read_bytes() == b"downloaded bytes"
        assert (later / "file.bin").read_bytes() == b"other bytes"
    assert not list(selected.glob(".toolkit-drive-*"))
    assert not list(later.glob(".toolkit-drive-*"))


@pytest.mark.asyncio
async def test_query_whitespace_is_preserved_as_url_data():
    query = "\tmodifiedTime < '2026-01-01T00:00:00Z'\n and name = 'a  b' "
    requests = []

    async def provider(request):
        requests.append(request)
        return httpx.Response(200, json={"files": []})

    async with httpx.AsyncClient(
        base_url="http://fixture.invalid/", transport=httpx.MockTransport(provider), trust_env=False
    ) as client:
        prepare_httpx_client(client)
        assert await GoogleDriveOperations(client).list_files_page(query=query) == {"files": []}
    assert len(requests) == 1
    assert requests[0].url.params["q"] == query


@pytest.mark.asyncio
async def test_repeated_cancel_joins_entered_upload_file_worker(tmp_path, monkeypatch):
    path = tmp_path / "selected.bin"
    path.write_bytes(b"upload bytes")
    entered, release = threading.Event(), threading.Event()
    original_io = module._io
    handles = []

    async def observed(function, *args):
        if getattr(function, "__name__", "") == "read":
            handles.append(function.__self__)

            def blocked_read():
                entered.set()
                if not release.wait(2):
                    raise AssertionError("test did not release local file worker")
                return function(*args)

            return await original_io(blocked_read)
        return await original_io(function, *args)

    monkeypatch.setattr(module, "_io", observed)
    requests = []

    async def provider(request):
        requests.append(request)
        return httpx.Response(200, json={})

    async with httpx.AsyncClient(
        base_url="http://fixture.invalid/", transport=httpx.MockTransport(provider), trust_env=False
    ) as client:
        prepare_httpx_client(client)
        task = asyncio.create_task(GoogleDriveOperations(client).create_file({}, upload_path=path))
        try:
            assert await asyncio.to_thread(entered.wait, 1)
            task.cancel("first")
            await asyncio.sleep(0)
            task.cancel("second")
            done, _ = await asyncio.wait({task}, timeout=0.02)
            assert not done
            assert handles and not handles[0].closed
            release.set()
            with pytest.raises(DriveWriteCancelled) as caught:
                await task
            assert caught.value.write_outcome.value == "unknown"
            assert all(handle.closed for handle in handles)
            assert not client.is_closed
            assert not requests
        finally:
            release.set()
            if not task.done():
                task.cancel()
            await asyncio.gather(task, return_exceptions=True)


@pytest.mark.asyncio
@pytest.mark.parametrize("mode", ["cancel", "deadline"])
async def test_cancelled_allocated_upload_is_not_an_attempt(tmp_path, monkeypatch, mode):
    path = tmp_path / "selected.bin"
    path.write_bytes(b"upload bytes")
    entered, release = threading.Event(), threading.Event()
    original_open = module.os.open
    descriptors, effects = [], []

    def allocated_open(selected, flags, *args, **kwargs):
        descriptor = original_open(selected, flags, *args, **kwargs)
        if selected == path:
            descriptors.append(descriptor)
            entered.set()
            assert release.wait(2), "test did not release allocated descriptor"
        return descriptor

    monkeypatch.setattr(module.os, "open", allocated_open)

    class Auth(httpx.Auth):
        def auth_flow(self, request):
            effects.append("auth")
            yield request

    async def provider(request):
        effects.append("provider")
        return httpx.Response(200, json={"id": "new"})

    async with prepare_httpx_client(
        httpx.AsyncClient(
            base_url="http://fixture.invalid/",
            auth=Auth(),
            transport=httpx.MockTransport(provider),
        )
    ) as client:
        ops = GoogleDriveOperations(client, timeout_seconds=0.04 if mode == "deadline" else 2)
        task = asyncio.create_task(ops.create_file({}, upload_path=path))
        try:
            assert await asyncio.to_thread(entered.wait, 1)
            if mode == "cancel":
                task.cancel()
                await asyncio.sleep(0)
            else:
                await asyncio.sleep(0.07)
            assert not task.done()
            assert len(descriptors) == 1
            assert module.os.fstat(descriptors[0]).st_size == len(b"upload bytes")
            release.set()
            if mode == "cancel":
                with pytest.raises(asyncio.CancelledError) as caught:
                    await task
                assert type(caught.value) is asyncio.CancelledError
            else:
                with pytest.raises(module.DriveError) as caught:
                    await task
                assert caught.value.kind is module.FailureKind.TIMEOUT
                assert caught.value.write_outcome is module.WriteOutcome.NOT_ATTEMPTED
            assert effects == []
            with pytest.raises(OSError):
                module.os.fstat(descriptors[0])
            assert not client.is_closed
        finally:
            release.set()
            if not task.done():
                task.cancel()
            await asyncio.gather(task, return_exceptions=True)
