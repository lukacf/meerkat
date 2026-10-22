"""Hosted MCP OAuth (registered https callback) over the Python SDK wrappers."""

from unittest.mock import AsyncMock, MagicMock

import pytest

from meerkat import MeerkatClient


@pytest.mark.asyncio
async def test_completion_sends_the_host_handle_and_iss_only_when_given():
    client = MeerkatClient()
    client._process = MagicMock()
    client._dispatcher = MagicMock()
    client._request = AsyncMock(return_value={})

    await client.auth_mcp_login_complete(
        "hosted",
        "https://mcp.example/mcp",
        code="c",
        state="s",
        redirect_uri="https://host.example/mcp/oauth/callback/hosted-abc",
        host_attempt_handle="h",
        iss="https://idp.example",
    )
    await client.auth_mcp_login_complete(
        "hosted",
        "https://mcp.example/mcp",
        code="c",
        state="s",
        redirect_uri="http://127.0.0.1:1/mcp/oauth/callback",
    )
    (registered_call, loopback_call) = client._request.await_args_list
    method, registered = registered_call.args
    _, loopback = loopback_call.args
    assert method == "auth/login/complete"
    assert registered["host_attempt_handle"] == "h"
    assert registered["iss"] == "https://idp.example"
    assert registered["redirect_uri"] == "https://host.example/mcp/oauth/callback/hosted-abc"
    assert set(loopback) == {"mcp", "code", "state", "redirect_uri"}
