"""Inert official HTTP failures retain actionable, private handshake outcomes.

MCPConnection.start/_serve/close are unchanged test subjects. Only the existing
SDK session and transport edges are inert; no network, sign-in or native result
is established by these controls.
"""
import asyncio
from contextlib import asynccontextmanager
from types import SimpleNamespace

import httpx
import pytest

from test_compiler import load_template


PRIVATE = ("private-endpoint-canary", "private-request-header-canary",
           "private-response-header-canary", "private-error-text-canary",
           "private-response-body-canary")


def http_failure(status):
    request = httpx.Request("GET", "https://private-endpoint-canary.invalid/mcp",
                            headers={"Authorization": "Bearer " + PRIVATE[1]})
    response = httpx.Response(status, request=request, text=PRIVATE[4],
                             headers={"X-Private": PRIVATE[2]})
    return httpx.HTTPStatusError(PRIVATE[3], request=request, response=response)


def grouped(failure):
    return ExceptionGroup(PRIVATE[3], [RuntimeError(PRIVATE[4]),
        ExceptionGroup(PRIVATE[0], [failure])])


@pytest.fixture
def clients(monkeypatch):
    import mcp
    from mcp.client import stdio, streamable_http

    module = load_template("mcp_connections")
    observed = SimpleNamespace(module=module, failure=None, transport="http",
        contexts=[], initialized=asyncio.Event(), list_calls=0, calls=0,
        owner=None, wait=asyncio.Event())

    @asynccontextmanager
    async def http_transport(url, *, headers, timeout):
        assert timeout == 30
        assert headers == {"Authorization": "Bearer " + PRIVATE[1]}
        observed.contexts.append(("transport-enter", asyncio.current_task()))
        try:
            yield object(), object(), None
        finally:
            observed.contexts.append(("transport-exit", asyncio.current_task()))

    @asynccontextmanager
    async def stdio_transport(parameters):
        assert parameters.command == "never-executed"
        observed.contexts.append(("transport-enter", asyncio.current_task()))
        try:
            yield object(), object()
        finally:
            observed.contexts.append(("transport-exit", asyncio.current_task()))

    class Session:
        def __init__(self, *args, **kwargs):
            pass

        async def __aenter__(self):
            observed.owner = asyncio.current_task()
            observed.contexts.append(("session-enter", observed.owner))
            return self

        async def __aexit__(self, *args):
            observed.contexts.append(("session-exit", asyncio.current_task()))

        async def initialize(self):
            observed.initialized.set()
            if observed.failure is None:
                await observed.wait.wait()
            else:
                raise observed.failure

        async def list_tools(self, cursor=None):
            observed.list_calls += 1
            pytest.fail("A failed handshake cannot enumerate or expose tools")

        async def call_tool(self, *args, **kwargs):
            observed.calls += 1
            pytest.fail("A failed handshake cannot invoke a tool")

    monkeypatch.setenv("MCP_AUTH", PRIVATE[1])
    monkeypatch.setattr(mcp, "ClientSession", Session)
    monkeypatch.setattr(streamable_http, "streamablehttp_client", http_transport)
    monkeypatch.setattr(stdio, "stdio_client", stdio_transport)
    observed.config = {"name": "Observatory calendar", "transport": "http",
        "url": "https://private-endpoint-canary.invalid/mcp",
        "headers": {"Authorization": "Bearer ${MCP_AUTH}"}}
    return observed


async def failed_handshake(clients):
    connection = clients.module.MCPConnection(clients.config)
    try:
        with pytest.raises(RuntimeError) as caught:
            await asyncio.wait_for(connection.start(), timeout=2)
        assert connection.ready.done() and not connection.ready.cancelled()
        assert connection.ready.exception() is caught.value
        assert connection.task is None and clients.owner.done()
        assert connection.tools == {} and clients.list_calls == clients.calls == 0
        assert [name for name, _ in clients.contexts] == ["transport-enter", "session-enter",
            "session-exit", "transport-exit"]
        assert all(task is clients.owner for _, task in clients.contexts)
        message = str(caught.value)
        assert all(private not in message for private in PRIVATE)
        assert caught.value.__cause__ is None and caught.value.__context__ is None
        return message
    finally:
        await connection.close()


@pytest.mark.parametrize("status,nested", [(401, False), (403, False), (401, True), (403, True)])
async def test_http_authorization_rejection_is_private_and_actionable_after_original_cleanup(clients, status, nested):
    failure = http_failure(status)
    clients.failure = grouped(failure) if nested else failure
    message = await failed_handshake(clients)
    assert f"HTTP {status}" in message and "Observatory calendar" in message
    assert "declared private credentials" in message
    assert "choose an authenticated connector" in message
    assert "Browser sign-in and token refresh are not available for this configured connection" in message
    assert ("authorization or permission" in message) is (status == 403)
    assert "retry" not in message.lower() and "authenticated successfully" not in message.lower()


@pytest.mark.parametrize("kind", ["http-400", "http-500", "transport", "response-like", "mixed-cancellation", "oversized"])
async def test_other_or_unbounded_http_handshake_failures_keep_original_generic_outcome(clients, kind):
    if kind.startswith("http-"):
        clients.failure = grouped(http_failure(int(kind.removeprefix("http-"))))
    elif kind == "transport":
        clients.failure = grouped(httpx.ConnectError(PRIVATE[3],
            request=httpx.Request("GET", "https://private-endpoint-canary.invalid/mcp")))
    elif kind == "response-like":
        clients.failure = RuntimeError(PRIVATE[3])
        clients.failure.response = SimpleNamespace(status_code=401)
    elif kind == "mixed-cancellation":
        clients.failure = BaseExceptionGroup(PRIVATE[3], [asyncio.CancelledError(), http_failure(401)])
    else:
        clients.failure = ExceptionGroup(PRIVATE[3], [http_failure(401),
            *[RuntimeError(PRIVATE[4]) for _ in range(64)]])
    message = await failed_handshake(clients)
    assert message == (f"MCP connection Observatory calendar failed ({type(clients.failure).__name__}). "
                       "Check the endpoint, command and local credentials.")
    assert "HTTP 401" not in message and "HTTP 403" not in message
    assert "Browser sign-in" not in message


async def test_stdio_failure_does_not_enter_http_authorization_classifier(clients, monkeypatch):
    clients.config = {"name": "Observatory calendar", "transport": "stdio", "command": "never-executed"}
    clients.failure = grouped(http_failure(401))
    monkeypatch.setattr(clients.module, "_http_authorization_status",
                        lambda error: pytest.fail("Stdio failure must retain its original path"))
    message = await failed_handshake(clients)
    assert message == ("MCP connection Observatory calendar failed (ExceptionGroup). "
                       "Check the endpoint, command and local credentials.")


async def test_cancelled_http_handshake_keeps_cancellation_and_original_owned_cleanup(clients):
    connection = clients.module.MCPConnection(clients.config)
    starter = asyncio.create_task(connection.start())
    try:
        await asyncio.wait_for(clients.initialized.wait(), timeout=30)
        owner = connection.task
        starter.cancel()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(starter, timeout=2)
        assert connection.task is None and owner.done()
        assert connection.ready.done() and not connection.ready.cancelled()
        message = str(connection.ready.exception())
        assert message == ("MCP connection Observatory calendar failed (CancelledError). "
                           "Check the endpoint, command and local credentials.")
        assert all(private not in message for private in PRIVATE)
        assert clients.list_calls == clients.calls == 0
        assert [name for name, _ in clients.contexts] == ["transport-enter", "session-enter",
            "session-exit", "transport-exit"]
        assert all(task is owner for _, task in clients.contexts)
    finally:
        if not starter.done():
            starter.cancel()
        await asyncio.gather(starter, return_exceptions=True)
        await connection.close()
