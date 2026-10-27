"""Scoped SDK media/state tests on prepared clients, no live Home Assistant."""

import asyncio
import httpx
import pytest
from meerkat_toolkit.httpx_client import prepare_httpx_client
from meerkat_toolkit.connectors.home_assistant import (
    HomeAssistantOperations,
    HomeAssistantError,
    FailureKind,
    WriteOutcome,
)

STATE = {
    "entity_id": "camera.front_clear",
    "state": "idle",
    "attributes": {"name": "Front"},
    "extension": {"null": None, "false": False},
}


@pytest.mark.asyncio
@pytest.mark.parametrize("payload", [[], [STATE]])
async def test_states_preserve_complete_collection(payload):
    async with httpx.AsyncClient(
        base_url="http://127.0.0.1/root/",
        transport=httpx.MockTransport(lambda r: httpx.Response(200, json=payload)),
    ) as c:
        prepare_httpx_client(c)
        assert await HomeAssistantOperations(c).list_states() == payload
        assert not c.is_closed


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "body",
    [
        b"{}",
        b"[null]",
        b'[{"entity_id":"camera.a","state":"idle","attributes":null}]',
        b'[{"entity_id":"camera.a","state":"idle","attributes":{},"state":"bad"}]',
        b'[{"entity_id":"camera.a","state":"idle","attributes":{"value":1e999}}]',
    ],
)
async def test_invalid_states_refuse_without_partial_success(body):
    async with httpx.AsyncClient(
        base_url="http://127.0.0.1/",
        transport=httpx.MockTransport(
            lambda r: httpx.Response(
                200, content=body, headers={"Content-Type": "application/json"}
            )
        ),
    ) as c:
        prepare_httpx_client(c)
        with pytest.raises(HomeAssistantError) as e:
            await HomeAssistantOperations(c).list_states()
        assert e.value.kind is FailureKind.INVALID_RESPONSE
        assert e.value.write_outcome is WriteOutcome.NOT_APPLICABLE


@pytest.mark.asyncio
@pytest.mark.parametrize("mime", ["image/jpeg", "image/png", "image/webp", "image/gif"])
async def test_camera_exact_binary_type_and_separate_limit(mime):
    body = b"\x00\xff\xfe" + b"x" * 1_048_577
    async with httpx.AsyncClient(
        base_url="http://127.0.0.1/",
        transport=httpx.MockTransport(
            lambda r: httpx.Response(200, content=body, headers={"Content-Type": mime})
        ),
    ) as c:
        prepare_httpx_client(c)
        result = await HomeAssistantOperations(c, max_response_bytes=2).get_camera_image(
            "camera.front"
        )
        assert result.data == body
        assert result.content_type == mime
        assert not c.is_closed


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "headers,body,kind",
    [
        ({"Content-Type": "text/html"}, b"<html>login</html>", "invalid_response"),
        ({}, b"bytes", "invalid_response"),
        ({"Content-Type": "image/png"}, b"", "invalid_response"),
        ({"Content-Type": "image/png", "Content-Length": "9"}, b"x", "invalid_response"),
        ({"Content-Type": "image/png", "Content-Length": "1, 1"}, b"x", "invalid_response"),
        ({"Content-Type": "image/png", "Content-Encoding": "gzip"}, b"", "unsupported_encoding"),
    ],
)
async def test_camera_refuses_malformed_receipts(headers, body, kind):
    async with httpx.AsyncClient(
        base_url="http://127.0.0.1/",
        transport=httpx.MockTransport(lambda r: httpx.Response(200, content=body, headers=headers)),
    ) as c:
        prepare_httpx_client(c)
        with pytest.raises(HomeAssistantError) as e:
            await HomeAssistantOperations(c).get_camera_image("camera.a")
        assert e.value.kind.value == kind


@pytest.mark.asyncio
async def test_explicit_media_limit_and_local_validation():
    calls = []

    def provider(r):
        calls.append(r)
        return httpx.Response(200, content=b"12", headers={"Content-Type": "image/png"})

    async with httpx.AsyncClient(
        base_url="http://127.0.0.1/", transport=httpx.MockTransport(provider)
    ) as c:
        prepare_httpx_client(c)
        for value in (0, -1, True, 1.5):
            with pytest.raises(ValueError):
                HomeAssistantOperations(c, max_media_bytes=value)
        op = HomeAssistantOperations(c, max_media_bytes=1)
        for target in ("light.a", "camera.A", "camera.a/b", "camera.a?width=1"):
            with pytest.raises(ValueError):
                await op.get_camera_image(target)
        assert calls == []
        with pytest.raises(HomeAssistantError) as e:
            await op.get_camera_image("camera.a")
        assert e.value.kind is FailureKind.RESPONSE_TOO_LARGE
        assert len(calls) == 1


@pytest.mark.asyncio
async def test_explicit_target_auth_and_request_scope_captured():
    calls = []

    class Auth(httpx.Auth):
        def auth_flow(self, r):
            r.headers["Authorization"] = "Bearer selected"
            yield r

    def provider(r):
        calls.append(r)
        return httpx.Response(200, content=b"png", headers={"Content-Type": "image/png"})

    async with httpx.AsyncClient(
        base_url="http://127.0.0.1/wrong/",
        params={"token": "ambient"},
        headers={
            "Range": "bytes=0-1",
            "If-None-Match": "ambient",
            "X-HTTP-Method-Override": "POST",
            "Authorization": "Bearer ambient",
        },
        transport=httpx.MockTransport(provider),
    ) as c:
        prepare_httpx_client(c)
        op = HomeAssistantOperations(c, base_url="http://127.0.0.1/selected", auth=Auth())
        c.base_url = "http://127.0.0.1/later/"
        await op.get_camera_image("camera.a")
    r = calls[0]
    assert str(r.url) == "http://127.0.0.1/selected/api/camera_proxy/camera.a"
    assert r.method == "GET" and r.headers["Authorization"] == "Bearer selected"
    assert all(k not in r.headers for k in ("range", "if-none-match", "x-http-method-override"))


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "status,kind",
    [
        (302, "redirect"),
        (401, "authentication"),
        (403, "permission"),
        (404, "not_found"),
        (503, "http_status"),
    ],
)
async def test_camera_status_no_follow_no_private_error(status, kind):
    calls = []

    def provider(r):
        calls.append(r)
        return httpx.Response(
            status, text="private secret", headers={"Location": "http://other.invalid/"}
        )

    async with httpx.AsyncClient(
        base_url="http://127.0.0.1/", follow_redirects=True, transport=httpx.MockTransport(provider)
    ) as c:
        prepare_httpx_client(c)
        with pytest.raises(HomeAssistantError) as e:
            await HomeAssistantOperations(c).get_camera_image("camera.a")
    assert len(calls) == 1 and e.value.kind.value == kind
    assert e.value.write_outcome is WriteOutcome.NOT_APPLICABLE
    assert "private secret" not in str(e.value)


@pytest.mark.asyncio
@pytest.mark.parametrize("cancel", [True, False])
async def test_camera_eof_close_is_joined_under_cancel_or_deadline(cancel):
    entered, release, closed = asyncio.Event(), asyncio.Event(), asyncio.Event()

    class Stream(httpx.AsyncByteStream):
        async def __aiter__(self):
            yield b"png"

        async def aclose(self):
            entered.set()
            await release.wait()
            closed.set()

    async with httpx.AsyncClient(
        base_url="http://127.0.0.1/",
        transport=httpx.MockTransport(
            lambda r: httpx.Response(200, stream=Stream(), headers={"Content-Type": "image/png"})
        ),
    ) as c:
        prepare_httpx_client(c)
        task = asyncio.create_task(
            HomeAssistantOperations(c, timeout_seconds=1 if cancel else 0.03).get_camera_image(
                "camera.a"
            )
        )
        await asyncio.wait_for(entered.wait(), 1)
        if cancel:
            task.cancel("first")
            task.cancel("second")
        await asyncio.wait({task}, timeout=0.06)
        assert not task.done() and not closed.is_set()
        release.set()
        with pytest.raises(asyncio.CancelledError if cancel else HomeAssistantError) as e:
            await task
        if not cancel:
            assert e.value.kind is FailureKind.TIMEOUT
        assert closed.is_set()
