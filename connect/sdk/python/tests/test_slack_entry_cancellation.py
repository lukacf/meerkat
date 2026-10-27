"""Pending cancellation must not start host authentication or a request."""

import asyncio
import importlib

import anyio
import httpx
import pytest


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["asyncio", "anyio"])
async def test_pending_cancellation_does_not_enter_auth_or_send(kind):
    from meerkat_toolkit.httpx_client import prepare_httpx_client

    module = importlib.import_module("meerkat_toolkit.frontend.slack")
    entries = []

    class Auth(httpx.Auth):
        async def async_auth_flow(self, request):
            entries.append("auth")
            yield request

    async def handler(request):
        entries.append("request")
        return httpx.Response(200, json={"ok": False, "error": "invalid_auth"})

    async with prepare_httpx_client(
        httpx.AsyncClient(
            base_url="https://slack.invalid/",
            auth=Auth(),
            trust_env=False,
            transport=httpx.MockTransport(handler),
        )
    ) as client:
        transport = module.SlackReplyTransport(client)
        caught = []

        async def invoke():
            if kind == "asyncio":
                asyncio.current_task().cancel()
                try:
                    await transport.send_text("D1", "hello", thread_ts="1.1")
                except asyncio.CancelledError as error:
                    caught.append(error)
            else:
                with anyio.CancelScope() as scope:
                    scope.cancel()
                    try:
                        await transport.send_text("D1", "hello", thread_ts="1.1")
                    except asyncio.CancelledError as error:
                        caught.append(error)

        await asyncio.wait_for(asyncio.create_task(invoke()), 1)
        assert len(caught) == 1
        assert entries == []
        assert not client.is_closed
