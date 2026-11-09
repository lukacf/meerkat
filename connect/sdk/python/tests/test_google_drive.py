"""Public Drive operations, synthetic transport and actual local file custody."""

from __future__ import annotations
import asyncio
import json
from pathlib import Path

import httpx
import pytest
from meerkat_toolkit.httpx_client import prepare_httpx_client

from meerkat_toolkit.connectors import google_drive as module
from meerkat_toolkit.connectors.google_drive import (
    DriveError,
    DriveWriteCancelled,
    FailureKind,
    GoogleDriveOperations,
    WriteOutcome,
)


class Stream(httpx.AsyncByteStream):
    def __init__(self, chunks, *, close_entered=None, close_release=None):
        self.chunks = chunks
        self.closed = False
        self.close_entered = close_entered
        self.close_release = close_release

    async def __aiter__(self):
        for chunk in self.chunks:
            if isinstance(chunk, BaseException):
                raise chunk
            yield chunk

    async def aclose(self):
        if self.close_entered:
            self.close_entered.set()
        if self.close_release:
            await self.close_release.wait()
        self.closed = True


def transport(handler, **kwargs):
    return prepare_httpx_client(
        httpx.AsyncClient(
            base_url="http://fixture.invalid/",
            transport=httpx.MockTransport(handler),
            trust_env=False,
            **kwargs,
        )
    )


@pytest.mark.asyncio
async def test_all_eight_operations_preserve_scope_fields_and_unknown_values(tmp_path):
    calls = []
    page = {
        "files": [{"name": "selected", "future": None}],
        "nextPageToken": " opaque+/= ",
        "future": {"false": False, "zero": 0},
    }

    async def provider(request):
        calls.append(request)
        if request.method == "DELETE":
            return httpx.Response(204)
        if request.url.path.endswith("/export") or request.url.params.get("alt") == "media":
            return httpx.Response(
                200, content=b' { "raw": false }\n', headers={"content-type": "application/json"}
            )
        if request.url.path.endswith("/permissions"):
            return httpx.Response(
                200, json={"id": "permission-different-from-file", "role": "reader"}
            )
        if request.url.path == "/drive/v3/files" and request.method == "GET":
            return httpx.Response(200, json=page)
        return httpx.Response(200, json={"name": "selected", "future": None, "flag": False})

    async with transport(
        provider,
        params={"key": "ambient", "fields": "wrong", "pageToken": "wrong"},
        headers={
            "If-Match": "wrong",
            "If-None-Match": "*",
            "Range": "bytes=1-3",
            "X-HTTP-Method-Override": "DELETE",
        },
    ) as client:
        ops = GoogleDriveOperations(client)
        assert (
            await ops.list_files_page(query=" q ", fields="files(name)", page_token=" exact+/= ")
            == page
        )
        assert (await ops.get_file("id/with?chars%", fields="name"))["name"] == "selected"
        assert (
            await ops.create_file({"name": "new", "future": None, "flag": False}, fields="name")
        )["flag"] is False
        assert (
            await ops.update_file(
                "file", {}, add_parents="p1,p2", remove_parents="p3", fields="name"
            )
        )["future"] is None
        assert (await ops.create_permission("file", {"role": "reader", "type": "user"}))[
            "id"
        ].startswith("permission-")
        await ops.delete_file("file")
        for operation in (
            ops.download_file("file", tmp_path / "download"),
            ops.export_file("file", "application/json", tmp_path / "export"),
        ):
            result = await operation
            assert Path(result["output_path"]).read_bytes() == b' { "raw": false }\n'
            assert result["bytes"] == 18
        assert not client.is_closed
        assert client.params["fields"] == "wrong"
    assert len(calls) == 8
    assert calls[0].url.params["pageToken"] == " exact+/= "
    assert calls[1].url.raw_path.split(b"?", 1)[0] == b"/drive/v3/files/id%2Fwith%3Fchars%25"
    for request in calls:
        assert "key" not in request.url.params
        assert not any(
            k in request.headers
            for k in ("If-Match", "If-None-Match", "Range", "X-HTTP-Method-Override")
        )


@pytest.mark.asyncio
@pytest.mark.parametrize("method", ["create", "update"])
async def test_large_multipart_is_streamed_without_new_size_cap(tmp_path, monkeypatch, method):
    path = tmp_path / "large.bin"
    blob = b"0123456789" * 630000
    path.write_bytes(blob)
    reads = []
    original = module._io

    async def observed(function, *args):
        if getattr(function, "__name__", "") == "read":
            reads.append(args[0])
        return await original(function, *args)

    monkeypatch.setattr(module, "_io", observed)
    calls = []

    async def provider(request):
        calls.append(request)
        body = await request.aread()
        assert int(request.headers["content-length"]) == len(body)
        boundary = request.headers["content-type"].split("boundary=", 1)[1].encode()
        assert body.endswith(b"\r\n--" + boundary + b"--\r\n")
        assert blob in body
        assert b'"future":null' in body and b'"flag":false' in body
        return httpx.Response(200, json={"name": "projected"})

    async with transport(provider) as client:
        ops = GoogleDriveOperations(client, max_request_bytes=100)
        body = {"name": "media", "future": None, "flag": False}
        if method == "create":
            await ops.create_file(
                body, upload_path=path, upload_content_type="application/x-fixture"
            )
        else:
            await ops.update_file(
                "file", body, upload_path=path, upload_content_type="application/x-fixture"
            )
    assert len(calls) == 1 and calls[0].url.params["uploadType"] == "multipart"
    assert calls[0].url.path.startswith("/upload/drive/v3/files")
    assert len(reads) > 90 and max(reads) <= 65536


@pytest.mark.asyncio
@pytest.mark.parametrize("raw", [b'{"id":"a","id":"b"}', b'{"x":NaN}', b'{"x":1e999}', b"[]", b"{"])
async def test_strict_json_after_write_is_unknown(raw):
    calls = []

    async def provider(request):
        calls.append(request)
        return httpx.Response(200, content=raw, headers={"content-type": "application/json"})

    async with transport(provider) as client:
        with pytest.raises(DriveError) as caught:
            await GoogleDriveOperations(client).create_file({})
    assert caught.value.kind is FailureKind.INVALID_RESPONSE
    assert caught.value.write_outcome is WriteOutcome.UNKNOWN
    assert len(calls) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("status", [301, 307, 401, 403, 404, 409, 412, 429, 500])
async def test_provider_failure_never_retries_or_exposes_body(status):
    calls = []

    async def provider(request):
        calls.append(request)
        return httpx.Response(
            status,
            content=b"private auth token canary",
            headers={"location": "http://other.invalid/", "retry-after": "7"},
        )

    async with transport(provider, follow_redirects=True) as client:
        with pytest.raises(DriveError) as caught:
            await GoogleDriveOperations(client).create_file({})
    assert len(calls) == 1 and caught.value.write_outcome is WriteOutcome.UNKNOWN
    assert caught.value.status_code == status and caught.value.retry_after_seconds == 7
    assert "canary" not in str(caught.value)


@pytest.mark.asyncio
@pytest.mark.parametrize("method", ["get", "update"])
async def test_returned_file_identity_checked_only_when_present(method):
    response = {"id": "wrong"}

    async def provider(request):
        return httpx.Response(200, json=response)

    async with transport(provider) as client:
        ops = GoogleDriveOperations(client)

        async def invoke():
            return await (
                ops.get_file("expected", fields="name")
                if method == "get"
                else ops.update_file("expected", {}, fields="name")
            )

        with pytest.raises(DriveError) as caught:
            await invoke()
        assert caught.value.kind is FailureKind.INVALID_RESPONSE
        response = {"name": "projected", "unknown": None}
        assert await invoke() == response


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "status,raw,valid",
    [(204, b"", True), (200, b"{}", True), (204, b"x", False), (200, b'{"id":"x"}', False)],
)
async def test_delete_has_drive_empty_success_contract(status, raw, valid):
    async def provider(request):
        return httpx.Response(status, content=raw, headers={"content-type": "application/json"})

    async with transport(provider) as client:
        if valid:
            assert await GoogleDriveOperations(client).delete_file("file") is None
        else:
            with pytest.raises(DriveError):
                await GoogleDriveOperations(client).delete_file("file")


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "kind", ["truncated", "too_long", "bad_length", "encoding", "stream_error"]
)
async def test_download_failure_retains_prior_file_and_cleans_stage(tmp_path, kind):
    output = tmp_path / "output"
    output.write_bytes(b"prior")
    headers = {"content-type": "application/json"}
    chunks = [b'{ "x": false }\n']
    if kind == "truncated":
        headers["content-length"] = "100"
    elif kind == "too_long":
        headers["content-length"] = "1"
    elif kind == "bad_length":
        headers["content-length"] = "bad"
    elif kind == "encoding":
        headers["content-encoding"] = "gzip"
    else:
        chunks.append(httpx.ReadError("private canary"))
    stream = Stream(chunks)

    async def provider(request):
        return httpx.Response(200, headers=headers, stream=stream)

    async with transport(provider) as client:
        with pytest.raises(DriveError) as caught:
            await GoogleDriveOperations(client).download_file("file", output)
        assert caught.value.write_outcome is WriteOutcome.NOT_APPLICABLE
        assert "canary" not in str(caught.value)
    assert stream.closed and output.read_bytes() == b"prior"
    assert sorted(p.name for p in tmp_path.iterdir()) == ["output"]


@pytest.mark.asyncio
@pytest.mark.parametrize("cancel", [True, False])
async def test_cleanup_is_joined_before_download_publication(tmp_path, cancel):
    output = tmp_path / "output"
    output.write_bytes(b"prior")
    entered, release = asyncio.Event(), asyncio.Event()
    stream = Stream([b"accepted"], close_entered=entered, close_release=release)

    async def provider(request):
        return httpx.Response(200, stream=stream)

    async with transport(provider) as client:
        task = asyncio.create_task(GoogleDriveOperations(client).download_file("file", output))
        await entered.wait()
        assert output.read_bytes() == b"prior"
        if cancel:
            task.cancel("first")
            task.cancel("second")
        done, _ = await asyncio.wait({task}, timeout=0.02)
        assert not done
        release.set()
        if cancel:
            with pytest.raises(asyncio.CancelledError):
                await task
        else:
            assert (await task)["bytes"] == 8
        assert not client.is_closed
    assert stream.closed
    assert output.read_bytes() == (b"prior" if cancel else b"accepted")
    assert sorted(p.name for p in tmp_path.iterdir()) == ["output"]


@pytest.mark.asyncio
async def test_deadline_during_close_does_not_publish(tmp_path):
    output = tmp_path / "output"
    output.write_bytes(b"prior")
    entered, release = asyncio.Event(), asyncio.Event()
    stream = Stream([b"new"], close_entered=entered, close_release=release)

    async def provider(request):
        return httpx.Response(200, stream=stream)

    async with transport(provider) as client:
        task = asyncio.create_task(
            GoogleDriveOperations(client, timeout_seconds=0.02).download_file("f", output)
        )
        await entered.wait()
        done, _ = await asyncio.wait({task}, timeout=0.04)
        assert not done
        release.set()
        with pytest.raises(DriveError) as caught:
            await task
        assert caught.value.kind is FailureKind.TIMEOUT
    assert output.read_bytes() == b"prior" and stream.closed
    assert len(list(tmp_path.iterdir())) == 1


@pytest.mark.asyncio
async def test_missing_upload_is_local_preflight_without_auth(tmp_path):
    class Auth(httpx.Auth):
        def auth_flow(self, request):
            raise AssertionError("must not authenticate")

    async def provider(request):
        raise AssertionError("must not enter")

    async with transport(provider) as client:
        with pytest.raises(DriveError) as caught:
            await GoogleDriveOperations(client, auth=Auth()).create_file(
                {}, upload_path=tmp_path / "missing"
            )
    assert caught.value.kind is FailureKind.LOCAL_IO
    assert caught.value.write_outcome is WriteOutcome.NOT_ATTEMPTED


@pytest.mark.asyncio
async def test_write_cancel_joins_response_and_preserves_unknown():
    entered, release = asyncio.Event(), asyncio.Event()
    stream = Stream([b'{"name":"selected"}'], close_entered=entered, close_release=release)

    async def provider(request):
        return httpx.Response(200, headers={"content-type": "application/json"}, stream=stream)

    async with transport(provider) as client:
        task = asyncio.create_task(GoogleDriveOperations(client).create_file({}))
        await entered.wait()
        task.cancel()
        task.cancel()
        done, _ = await asyncio.wait({task}, timeout=0.02)
        assert not done
        release.set()
        with pytest.raises(DriveWriteCancelled) as caught:
            await task
        assert caught.value.write_outcome is WriteOutcome.UNKNOWN
    assert stream.closed


@pytest.mark.asyncio
async def test_mutable_body_frozen_before_auth_suspension():
    entered, release = asyncio.Event(), asyncio.Event()

    class Auth(httpx.Auth):
        async def async_auth_flow(self, request):
            entered.set()
            await release.wait()
            yield request

    sent = []

    async def provider(request):
        sent.append(json.loads(await request.aread()))
        return httpx.Response(200, json={})

    original = {"name": "before", "nested": {"v": False}, "null": None}
    async with transport(provider) as client:
        task = asyncio.create_task(GoogleDriveOperations(client, auth=Auth()).create_file(original))
        await entered.wait()
        original["nested"]["v"] = True
        original["name"] = "after"
        release.set()
        await task
    assert sent == [{"name": "before", "nested": {"v": False}, "null": None}]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "filename,explicit,metadata,expected",
    [
        ("source.txt", "application/x-explicit", "application/x-meta", "application/x-explicit"),
        ("source.txt", None, "application/x-meta", "text/plain"),
        ("source", None, "application/x-meta", "application/x-meta"),
        ("source", None, None, "application/octet-stream"),
    ],
)
async def test_upload_mime_precedence(tmp_path, filename, explicit, metadata, expected):
    path = tmp_path / filename
    path.write_bytes(b"bytes")

    async def provider(request):
        assert f"Content-Type: {expected}\r\n".encode() in await request.aread()
        return httpx.Response(200, json={})

    async with transport(provider) as client:
        await GoogleDriveOperations(client).create_file(
            {} if metadata is None else {"mimeType": metadata},
            upload_path=path,
            upload_content_type=explicit,
        )


@pytest.mark.asyncio
async def test_staging_fdopen_failure_closes_fd_and_unlinks_name(tmp_path, monkeypatch):
    output = tmp_path / "target"
    output.write_bytes(b"prior")
    original = module.os.fdopen
    descriptors = []

    def fail(fd, mode):
        if mode == "wb":
            descriptors.append(fd)
            raise OSError("private path canary")
        return original(fd, mode)

    monkeypatch.setattr(module.os, "fdopen", fail)

    async def provider(request):
        raise AssertionError("must not send")

    async with transport(provider) as client:
        with pytest.raises(DriveError) as caught:
            await GoogleDriveOperations(client).download_file("f", output)
    assert caught.value.kind is FailureKind.LOCAL_IO and "canary" not in str(caught.value)
    assert output.read_bytes() == b"prior" and sorted(p.name for p in tmp_path.iterdir()) == [
        "target"
    ]
    for fd in descriptors:
        with pytest.raises(OSError):
            module.os.fstat(fd)
