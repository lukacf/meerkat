"""Host preparation is explicit and checked before credentials or requests."""

import httpx
import pytest

from meerkat_toolkit.httpx_client import (
    HttpxClientNotPrepared,
    prepare_httpx_client,
    require_prepared_httpx_client,
)
from test_httpx_response_close import GatedClose, PAYLOADS, invocation


@pytest.mark.asyncio
@pytest.mark.parametrize("provider", PAYLOADS)
@pytest.mark.parametrize("mode", ["missing", "reordered"])
async def test_unprepared_client_refuses_before_auth_or_provider(provider, mode):
    observed = []

    class Auth(httpx.Auth):
        async def async_auth_flow(self, request):
            observed.append("credential")
            yield request

    async def hook(response):
        observed.append("hook")

    async def handler(request):
        observed.append("request")
        return httpx.Response(200, json=PAYLOADS[provider])

    async with httpx.AsyncClient(
        base_url="http://fixture.invalid/",
        transport=httpx.MockTransport(handler),
        auth=Auth(),
        event_hooks={"response": [hook]},
        trust_env=False,
    ) as client:
        if mode == "reordered":
            prepare_httpx_client(client)
            client.event_hooks["response"].insert(0, hook)
        selected = tuple(client.event_hooks["response"])
        operation = invocation(provider, client, 1)
        with pytest.raises(HttpxClientNotPrepared) as refusal:
            await operation
        assert refusal.value.request_attempted is False
        assert observed == []
        assert tuple(client.event_hooks["response"]) == selected
        assert not client.is_closed


@pytest.mark.asyncio
async def test_preparation_is_idempotent_and_preserves_host_configuration():
    async def first(response):
        pass

    async def second(response):
        pass

    auth = httpx.BasicAuth("fixture", "fixture-password")
    async with httpx.AsyncClient(
        base_url="http://fixture.invalid/selected/",
        auth=auth,
        params={"host": "value"},
        headers={"X-Host": "value"},
        cookies={"host-cookie": "value"},
        event_hooks={"response": [first, second]},
        transport=httpx.MockTransport(lambda request: httpx.Response(200)),
        trust_env=False,
    ) as client:
        hooks = client.event_hooks["response"]
        state = (client.auth, client.base_url, client.params, client.headers, client.cookies)
        assert prepare_httpx_client(client) is client
        installed = tuple(hooks)
        assert installed[1:] == (first, second)
        assert prepare_httpx_client(client) is client
        assert tuple(hooks) == installed
        assert client.event_hooks["response"] is hooks
        assert state == (
            client.auth, client.base_url, client.params, client.headers, client.cookies
        )
        require_prepared_httpx_client(client)
        assert not client.is_closed


@pytest.mark.asyncio
@pytest.mark.parametrize("provider", PAYLOADS)
async def test_normal_response_hook_can_read_body_and_complete(provider):
    stream = GatedClose(PAYLOADS[provider])
    stream.release.set()
    sends = []

    async def handler(request):
        sends.append(request)
        return httpx.Response(200, headers={"Content-Type": "application/json"}, stream=stream)

    async def hook(response):
        await response.aread()

    async with prepare_httpx_client(httpx.AsyncClient(
        base_url="http://fixture.invalid/",
        transport=httpx.MockTransport(handler),
        event_hooks={"response": [hook]},
        trust_env=False,
    )) as client:
        await invocation(provider, client, 1)
        assert len(sends) == 1
        assert stream.closed and stream.close_entries == 1
        assert not client.is_closed
