from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager
import json

import anyio
import httpx
import pytest

from meerkat_toolkit.frontend.paired_host_live import (
    PairedHostLiveImageClient,
    ImageFailure,
    ImageSubmissionError,
)


ROUTE = "/live/image"
SCHEME = "HostLease"
TOKEN = SCHEME + " " + "t" * 43


def live_client(base, authorization_provider, **options):
    return PairedHostLiveImageClient(
        base,
        authorization_provider,
        route=ROUTE,
        authorization_scheme=SCHEME,
        **options,
    )


@asynccontextmanager
async def server(
    *, status=200, body=b'{"accepted":true}', extra=b"", drop=False, gate=None
):
    calls = []
    entered = asyncio.Event()
    handlers = set()

    async def handle(reader, writer):
        task = asyncio.current_task()
        handlers.add(task)
        try:
            head = await reader.readuntil(b"\r\n\r\n")
            lines = head.split(b"\r\n")
            headers = dict(line.split(b": ", 1) for line in lines[1:] if line)
            data = await reader.readexactly(int(headers[b"Content-Length"]))
            calls.append((lines[0], headers, data))
            entered.set()
            if gate is not None:
                await gate.wait()
            if not drop:
                writer.write(
                    f"HTTP/1.1 {status} Test\r\n".encode()
                    + b"Content-Type: application/json\r\nConnection: close\r\n"
                    + extra
                    + f"Content-Length: {len(body)}\r\n\r\n".encode()
                    + body
                )
                await writer.drain()
        finally:
            writer.close()
            await writer.wait_closed()
            handlers.discard(task)

    listener = await asyncio.start_server(handle, "127.0.0.1", 0)
    port = listener.sockets[0].getsockname()[1]
    try:
        yield f"http://127.0.0.1:{port}", calls, entered
    finally:
        if gate is not None:
            gate.set()
        listener.close()
        await listener.wait_closed()
        if handlers:
            await asyncio.gather(*tuple(handlers))


async def submit(client):
    return await client.submit_image(
        idempotency_key="job-1", mime_type="image/jpeg", data_b64="eA=="
    )


@pytest.mark.asyncio
async def test_actual_http_exact_route_current_lease_and_no_ambient_proxy(monkeypatch):
    async with server() as (sink, sink_calls, _), server() as (base, calls, _):
        monkeypatch.setenv("HTTP_PROXY", sink)
        monkeypatch.setenv("ALL_PROXY", sink)
        monkeypatch.setenv("NO_PROXY", "")
        current = TOKEN
        client = live_client(base, lambda: current)
        assert await submit(client) == {"accepted": True}
        current = SCHEME + " " + "u" * 43
        await submit(client)
        assert not sink_calls
        assert len(calls) == 2
        assert calls[0][0] == b"POST /live/image HTTP/1.1"
        assert calls[0][1][b"Authorization"] == TOKEN.encode()
        assert calls[1][1][b"Authorization"] == current.encode()
        assert json.loads(calls[0][2]) == {
            "data": "eA==",
            "idempotency_key": "job-1",
            "mime": "image/jpeg",
        }


@pytest.mark.parametrize(
    "url",
    [
        "https://127.0.0.1:8080",
        "http://localhost:8080",
        "http://127.0.0.2:8080",
        "http://127.0.0.1",
        "http://127.0.0.1:8080/path",
        "http://127.0.0.1:8080?",
        "http://user@127.0.0.1:8080",
        "http://127.0.0.1:8080/#",
        "http://127.0.0.1:8080\n",
    ],
)
def test_base_is_one_explicit_loopback_origin(url):
    with pytest.raises(ValueError):
        live_client(url, lambda: TOKEN)


@pytest.mark.parametrize(
    "route",
    [
        "",
        "live/image",
        "/",
        "//live",
        "/live/",
        "/./live",
        "/live/../image",
        "/live?image",
        "/live#image",
        "/live%2fimage",
        "/live image",
        "/" + "a" * 65,
        "/a" * 9,
        None,
    ],
)
def test_route_is_one_host_named_absolute_path(route):
    with pytest.raises(ValueError):
        PairedHostLiveImageClient(
            "http://127.0.0.1:8080",
            lambda: TOKEN,
            route=route,
            authorization_scheme=SCHEME,
        )


@pytest.mark.parametrize("scheme", ["", "Host Lease", "Lease\n", "1Lease", "a" * 65, None])
def test_authorization_scheme_is_one_token(scheme):
    with pytest.raises(ValueError):
        PairedHostLiveImageClient(
            "http://127.0.0.1:8080",
            lambda: TOKEN,
            route=ROUTE,
            authorization_scheme=scheme,
        )


@pytest.mark.asyncio
async def test_lease_with_another_scheme_has_no_http_effect():
    async with server() as (base, calls, _):
        client = live_client(base, lambda: "Bearer " + "t" * 43)
        with pytest.raises(ImageSubmissionError) as failure:
            await submit(client)
        assert failure.value.kind is ImageFailure.LEASE_UNAVAILABLE
        assert failure.value.request_entered is False
        assert not calls


@pytest.mark.asyncio
async def test_validation_and_missing_lease_have_no_http_effect():
    async with server() as (base, calls, _):
        client = live_client(base, lambda: None)
        with pytest.raises(ImageSubmissionError) as failure:
            await submit(client)
        assert failure.value.kind is ImageFailure.LEASE_UNAVAILABLE
        assert failure.value.request_entered is False
        with pytest.raises(ValueError):
            await client.submit_image(
                idempotency_key="", mime_type="image/jpeg", data_b64="eA=="
            )
        with pytest.raises(ValueError):
            await submit(
                live_client(base, lambda: TOKEN, max_request_bytes=1)
            )
        assert not calls


@pytest.mark.asyncio
@pytest.mark.parametrize("status", [302, 307, 308])
async def test_redirect_never_discloses_or_replays(status):
    async with server() as (sink, sink_calls, _):
        async with server(
            status=status, extra=f"Location: {sink}/stolen\r\n".encode()
        ) as (base, calls, _):
            with pytest.raises(ImageSubmissionError) as failure:
                await submit(live_client(base, lambda: TOKEN))
            assert failure.value.kind is ImageFailure.REDIRECT
            assert failure.value.request_entered is True
            assert len(calls) == 1
            assert not sink_calls


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "body",
    [
        b"bad",
        b"[]",
        b'{"x":1,"x":2}',
        b'{"nested":{"n":1e999}}',
        b'{"n":NaN}',
        b'{"error":"secret"}',
    ],
)
async def test_malformed_response_is_unconfirmed_and_redacted(body):
    async with server(body=body) as (base, calls, _):
        with pytest.raises(ImageSubmissionError) as failure:
            await submit(live_client(base, lambda: TOKEN))
        assert failure.value.kind is ImageFailure.INVALID_RESPONSE
        assert failure.value.request_entered is True
        assert failure.value.__context__ is None
        assert "secret" not in str(failure.value)
        assert len(calls) == 1


@pytest.mark.asyncio
async def test_lost_response_is_one_attempt():
    async with server(drop=True) as (base, calls, _):
        with pytest.raises(ImageSubmissionError) as failure:
            await submit(live_client(base, lambda: TOKEN))
        assert failure.value.kind is ImageFailure.TRANSPORT
        assert failure.value.request_entered is True
        assert len(calls) == 1


@pytest.mark.asyncio
async def test_real_deadline_has_no_automatic_retry():
    gate = asyncio.Event()
    async with server(gate=gate, drop=True) as (base, calls, entered):
        request = asyncio.create_task(
            submit(live_client(base, lambda: TOKEN, timeout_seconds=0.1))
        )
        await entered.wait()
        with pytest.raises(ImageSubmissionError) as failure:
            await request
        assert failure.value.kind is ImageFailure.TIMEOUT
        assert len(calls) == 1


@pytest.mark.asyncio
async def test_response_byte_bound():
    async with server(body=b'{"large":"' + b"x" * 100 + b'"}') as (base, calls, _):
        with pytest.raises(ImageSubmissionError) as failure:
            await submit(
                live_client(base, lambda: TOKEN, max_response_bytes=32)
            )
        assert failure.value.kind is ImageFailure.RESPONSE_TOO_LARGE
        assert len(calls) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("ordinary_error", [False, True])
async def test_repeated_cancel_joins_supported_cooperative_response_close(
    monkeypatch, ordinary_error
):
    close_entered, release_close, closed = (
        asyncio.Event(),
        asyncio.Event(),
        asyncio.Event(),
    )
    actual_client = httpx.AsyncClient
    clients = []

    class Stream(httpx.AsyncByteStream):
        async def __aiter__(self):
            yield b"{}"

        async def aclose(self):
            with anyio.CancelScope(shield=True):
                close_entered.set()
                await release_close.wait()
                closed.set()

    def make_client(**options):
        assert options["trust_env"] is False
        assert options["follow_redirects"] is False
        client = actual_client(
            transport=httpx.MockTransport(
                lambda request: httpx.Response(
                    500 if ordinary_error else 200,
                    headers={"content-type": "application/json"},
                    stream=Stream(),
                )
            ),
            **options,
        )
        clients.append(client)
        return client

    monkeypatch.setattr(httpx, "AsyncClient", make_client)
    request = asyncio.create_task(
        submit(live_client("http://127.0.0.1:8080", lambda: TOKEN))
    )
    await close_entered.wait()
    request.cancel("first caller cancellation")
    await asyncio.sleep(0)
    request.cancel("second caller cancellation")
    await asyncio.sleep(0)
    assert not request.done()
    assert not closed.is_set()
    release_close.set()
    with pytest.raises(asyncio.CancelledError) as cancellation:
        await request
    assert cancellation.value.args == ("first caller cancellation",)
    assert closed.is_set()
    assert all(client.is_closed for client in clients)


@pytest.mark.asyncio
async def test_primary_http_failure_survives_cleanup_failure(monkeypatch):
    actual_client = httpx.AsyncClient

    class Stream(httpx.AsyncByteStream):
        async def __aiter__(self):
            yield b"{}"

        async def aclose(self):
            raise RuntimeError("secret-close-diagnostic")

    monkeypatch.setattr(
        httpx,
        "AsyncClient",
        lambda **options: actual_client(
            transport=httpx.MockTransport(
                lambda request: httpx.Response(403, stream=Stream())
            ),
            **options,
        ),
    )
    with pytest.raises(ImageSubmissionError) as failure:
        await submit(live_client("http://127.0.0.1:8080", lambda: TOKEN))
    assert failure.value.kind is ImageFailure.HTTP_STATUS
    assert failure.value.status == 403
    assert failure.value.__context__ is None


@pytest.mark.asyncio
async def test_cancel_before_first_scheduling_has_no_client(monkeypatch):
    monkeypatch.setattr(
        httpx, "AsyncClient", lambda **options: pytest.fail("unexpected client")
    )
    task = asyncio.create_task(
        submit(live_client("http://127.0.0.1:8080", lambda: TOKEN))
    )
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
