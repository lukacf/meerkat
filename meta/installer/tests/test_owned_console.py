"""Remote-safe access uses the live worker's Console, never a caller's URL."""
import asyncio
import json
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock

import httpx
import pytest
from fastapi.testclient import TestClient

from launchkit import owned_console, server

GUIDE = "11111111-1111-4111-8111-111111111111"
JOB = "22222222-2222-4222-8222-222222222222"


@pytest.fixture
def console_api(tmp_path, monkeypatch):
    directory = tmp_path / GUIDE / JOB
    directory.mkdir(parents=True)
    receipt = {"guide_id": GUIDE, "build_id": JOB, "rehearsal_id": JOB,
               "status": "running", "native_console_url": "http://127.0.0.1:38659/console"}
    process = SimpleNamespace(returncode=None)
    task = SimpleNamespace(done=lambda: False)
    service = SimpleNamespace(_directory=lambda guide, job=None: directory,
        processes={JOB: process}, tasks={JOB: task}, status=AsyncMock(return_value=receipt))
    def save():
        (directory / "receipt.json").write_text(json.dumps(receipt))
    save()
    monkeypatch.setattr(server, "implementation", service)
    monkeypatch.setattr(server, "rehearsal", service)
    upstream_requests = []
    handler = SimpleNamespace(reply=lambda request: httpx.Response(200, content=b"native response"))
    async def upstream(request):
        upstream_requests.append(request)
        return handler.reply(request)
    client_class = httpx.AsyncClient
    def client(**kwargs):
        kwargs["transport"] = httpx.MockTransport(upstream)
        return client_class(**kwargs)
    monkeypatch.setattr(owned_console.httpx, "AsyncClient", client)
    return SimpleNamespace(client=TestClient(server.app), service=service, receipt=receipt,
        process=process, task=task, save=save, handler=handler, requests=upstream_requests,
        prefix=f"/api/owned-console/implementation/{GUIDE}/{JOB}")


@pytest.mark.parametrize("kind", ["implementation", "rehearsal"])
def test_status_and_console_shell_use_the_authoring_origin(console_api, kind):
    f = console_api
    status = f.client.get(f"/api/guide/{GUIDE}/{kind}")
    prefix = f"/api/owned-console/{kind}/{GUIDE}/{JOB}"
    assert status.json()["browser_console_url"] == prefix + "/console"
    response = f.client.get(prefix + "/console")
    assert response.status_code == 200
    assert f'<meta name="meta-base-url" content="{prefix}">' in response.text
    assert f'src="{prefix}/console/assets/console-app.js"' in response.text
    assert f'href="{prefix}/console/assets/console-app.css"' in response.text
    assert "38659" not in response.text
    assert response.headers["cache-control"] == "no-store"
    assert not f.requests


@pytest.mark.parametrize("path", ["console/assets/console-app.js", "console/assets/console-app.css",
                                    "console/experience", "console/timeline?after=cursor", "blobs/sha256:abc"])
def test_native_assets_and_api_keep_the_exact_worker_destination(console_api, path):
    f = console_api
    f.handler.reply = lambda request: httpx.Response(200, content=b"actual native bytes",
        headers={"set-cookie": "native-secret=fake", "location": "https://example.invalid/",
                 "content-type": "application/javascript"})
    response = f.client.get(f.prefix + "/" + path,
        headers={"Cookie": "forge-owner=fake", "Authorization": "Bearer fake", "X-Forwarded-Host": "bad.invalid"})
    assert response.status_code == 200 and response.content == b"actual native bytes"
    assert str(f.requests[0].url) == "http://127.0.0.1:38659/" + path
    assert not {"cookie", "authorization", "x-forwarded-host"} & set(f.requests[0].headers)
    assert "set-cookie" not in response.headers and "location" not in response.headers


def test_rpc_and_multipart_preserve_body_and_content_type(console_api):
    f = console_api
    for path, content_type, body in [("console/rpc", "application/json", b'{"method":"meta/console/send"}'),
                                     ("console/rpc/multipart", "multipart/form-data; boundary=xyz", b"--xyz\r\nraw\x00bytes")]:
        response = f.client.post(f.prefix + "/" + path, content=body,
            headers={"Origin": "http://testserver", "Content-Type": content_type})
        assert response.status_code == 200
        assert f.requests[-1].content == body and f.requests[-1].headers["content-type"] == content_type


def test_timeline_reconnect_preserves_native_resume_cursor(guide_console):
    f = guide_console
    cursor = "timeline:00042/owner-7"
    def events(request):
        if request.headers.get("last-event-id") == cursor:
            body = b'id: timeline:00043/owner-7\ndata: {"event":"next"}\n\n'
        else:
            body = b'id: timeline:00042/owner-7\ndata: {"event":"first"}\n\n'
        return httpx.Response(200, content=body, headers={"content-type": "text/event-stream"})
    f.handler.reply = events
    first = f.client.get(f.prefix + "/console/timeline/stream")
    assert first.status_code == 200 and b'"first"' in first.content
    resumed = f.client.get(f.prefix + "/console/timeline/stream",
        headers={"Last-Event-ID": cursor, "Cookie": "private=fake", "Authorization": "Bearer fake"})
    assert resumed.status_code == 200 and b'"next"' in resumed.content
    assert b'"first"' not in resumed.content
    assert f.requests[-1].headers["last-event-id"] == cursor
    assert not {"cookie", "authorization"} & set(f.requests[-1].headers)


@pytest.mark.parametrize("cursor", ["x" * (owned_console.MAX_EVENT_CURSOR + 1),
    "line\r\nbreak", "bad\x00cursor", "bad\x7fcursor", b"bad\x80cursor", b"caf\xc3\xa9"])
def test_invalid_event_cursor_is_rejected_before_forwarding(console_api, cursor):
    f = console_api
    assert f.client.get(f.prefix + "/console/timeline/stream", headers={"Last-Event-ID": cursor}).status_code == 400
    assert not f.requests


@pytest.mark.parametrize("address", ["https://example.invalid/console", "http://localhost:38659/console",
    "http://127.0.0.1:38659/console?next=http://example.invalid", "http://user@127.0.0.1:38659/console",
    "http://127.0.0.1:22/console", "http://127.0.0.1:99999/console", None])
def test_untrusted_receipt_addresses_never_become_proxy_targets(console_api, address):
    f = console_api
    f.receipt["native_console_url"] = address
    f.save()
    assert f.client.get(f.prefix + "/console/experience").status_code == 410
    assert not f.requests


@pytest.mark.parametrize("mutation", ["missing_process", "exited_process", "finished_task", "other_selection",
                                       "wrong_guide", "wrong_job", "stopping", "cleared_address"])
def test_saved_receipt_without_current_custody_cannot_open_console(console_api, mutation):
    f = console_api
    if mutation == "missing_process": f.service.processes.clear()
    elif mutation == "exited_process": f.process.returncode = 0
    elif mutation == "finished_task": f.task.done = lambda: True
    elif mutation == "other_selection":
        original = f.service._directory
        f.service._directory = lambda guide, job=None: original(guide, job) if job else Path("another-job")
    elif mutation == "wrong_guide": f.receipt["guide_id"] = JOB
    elif mutation == "wrong_job": f.receipt["build_id"] = GUIDE
    elif mutation == "stopping": f.receipt["stop_requested"] = True
    else: f.receipt["native_console_url"] = None
    f.save()
    assert f.client.get(f.prefix + "/console").status_code == 410
    assert f.client.get(f"/api/guide/{GUIDE}/implementation").json()["browser_console_url"] is None
    assert not f.requests


@pytest.mark.parametrize("headers", [{"Origin": "https://other.invalid"}, {"Sec-Fetch-Site": "cross-site"}])
def test_cross_origin_access_is_rejected_before_forwarding(console_api, headers):
    f = console_api
    assert f.client.post(f.prefix + "/console/rpc", json={}, headers=headers).status_code == 403
    assert not f.requests


@pytest.mark.parametrize("path", ["https://example.invalid/", "host/toolkit/setup", "console/%252e%252e/private", "console/bad%5cpath"])
def test_console_mount_does_not_offer_other_host_routes_or_traversal(console_api, path):
    f = console_api
    assert f.client.get(f.prefix + "/" + path).status_code == 404
    assert not f.requests


def test_worker_retirement_during_response_discards_the_response(console_api):
    f = console_api
    def response(request):
        f.process.returncode = 0
        return httpx.Response(200, content=b"retired owner content")
    f.handler.reply = response
    result = f.client.get(f.prefix + "/console/experience")
    assert result.status_code == 410 and "retired owner content" not in result.text


def test_redirect_is_not_followed(console_api):
    f = console_api
    f.handler.reply = lambda request: httpx.Response(302, headers={"Location": "https://example.invalid/"})
    assert f.client.get(f.prefix + "/console/experience").status_code == 502
    assert len(f.requests) == 1


async def read_console_stream(f):
    class Read:
        method = "GET"
        headers = {}
        url = httpx.URL("http://testserver" + f.prefix + "/console/timeline/stream")
        scope = {"query_string": b""}
        async def stream(self):
            yield b""
    owner = owned_console.resolve_owner(f.service, GUIDE, JOB, "implementation")
    response = await owned_console.console_response(Read(), owner, "console/timeline/stream")
    async def consume():
        return b"".join([chunk async for chunk in response.body_iterator])
    # Revocation must also close an idle upstream, rather than waiting forever
    # for another event. The deadline makes a missing custody check fail promptly.
    return response, await asyncio.wait_for(consume(), timeout=2)


def test_idle_sse_closes_when_the_owned_worker_retires(console_api):
    f = console_api
    class Events(httpx.AsyncByteStream):
        closed = False
        async def __aiter__(self):
            yield b'data: {"event":"native-frame"}\n\n'
            f.process.returncode = 0
            await asyncio.Event().wait()
        async def aclose(self):
            self.closed = True
    events = Events()
    f.handler.reply = lambda request: httpx.Response(200, stream=events, headers={"content-type": "text/event-stream"})
    response, content = asyncio.run(read_console_stream(f))
    assert response.status_code == 200
    assert content == b'data: {"event":"native-frame"}\n\n'
    assert response.headers["x-accel-buffering"] == "no"
    assert events.closed


def test_chunk_arriving_after_worker_retirement_is_not_forwarded(console_api):
    f = console_api
    class Events(httpx.AsyncByteStream):
        closed = False
        async def __aiter__(self):
            yield b'data: {"event":"current-owner"}\n\n'
            f.process.returncode = 0
            yield b'data: {"event":"retired-owner"}\n\n'
        async def aclose(self):
            self.closed = True
    events = Events()
    f.handler.reply = lambda request: httpx.Response(200, stream=events, headers={"content-type": "text/event-stream"})
    response, content = asyncio.run(read_console_stream(f))
    assert response.status_code == 200
    assert content == b'data: {"event":"current-owner"}\n\n'
    assert events.closed


@pytest.fixture
def guide_console(console_api, monkeypatch):
    f = console_api
    f.runtime = SimpleNamespace(is_running=True, rust_http_base_url="http://127.0.0.1:38659")
    f.architect = SimpleNamespace(runtime=f.runtime, console_generation=JOB,
        provider_stop_failed=False, provider_paused=False)
    monkeypatch.setattr(server, "architect", f.architect)
    f.prefix = f"/api/owned-console/guide/{JOB}"
    return f


def test_guide_console_uses_live_generation_for_shell_assets_and_rpc(guide_console):
    f = guide_console
    assert owned_console.guide_browser_console_url(f.architect) == f.prefix + "/console"
    shell = f.client.get(f.prefix + "/console")
    assert shell.status_code == 200
    assert f'content="{f.prefix}"' in shell.text
    assert shell.headers["content-security-policy"] == "frame-ancestors 'self'"
    for path in ("console/assets/console-app.js", "console/experience", "blobs/sha256:abc"):
        response = f.client.get(f.prefix + "/" + path)
        assert response.status_code == 200
        assert str(f.requests[-1].url) == "http://127.0.0.1:38659/" + path
    assert f.client.post(f.prefix + "/console/rpc", json={"method": "meta/console/send"}).status_code == 200


@pytest.mark.parametrize("mutation", ["stopped", "paused", "cleanup_failed", "replaced", "missing", "bad_origin"])
def test_guide_console_refuses_retired_generation_without_forwarding(guide_console, mutation):
    f = guide_console
    if mutation == "stopped": f.runtime.is_running = False
    elif mutation == "paused": f.architect.provider_paused = True
    elif mutation == "cleanup_failed": f.architect.provider_stop_failed = True
    elif mutation == "replaced": f.architect.console_generation = GUIDE
    elif mutation == "missing": f.architect.runtime = None
    else: f.runtime.rust_http_base_url = "https://other.invalid"
    assert f.client.get(f.prefix + "/console/experience").status_code == 410
    assert not f.requests


def test_guide_runtime_replaced_during_request_does_not_return_old_data(guide_console):
    f = guide_console
    def response(request):
        f.architect.runtime = SimpleNamespace(is_running=True, rust_http_base_url="http://127.0.0.1:38659")
        return httpx.Response(200, content=b"old runtime result")
    f.handler.reply = response
    result = f.client.get(f.prefix + "/console/experience")
    assert result.status_code == 410 and "old runtime result" not in result.text


def test_retirement_while_reading_request_never_forwards(console_api):
    f = console_api
    owner = owned_console.resolve_owner(f.service, GUIDE, JOB, "implementation")
    class Upload:
        method = "POST"
        headers = {}
        url = httpx.URL("http://testserver" + f.prefix + "/console/rpc")
        scope = {"query_string": b""}
        async def stream(self):
            yield b'{"method":'
            f.process.returncode = 0
            yield b'"meta/console/send"}'
    async def run():
        with pytest.raises(owned_console.HTTPException) as error:
            await owned_console.console_response(Upload(), owner, "console/rpc")
        assert error.value.status_code == 410
        assert not f.requests
    asyncio.run(run())
