"""Missing capability controls frozen before source implementation."""

import asyncio
import importlib.util
import json
from pathlib import Path
import traceback
from urllib.parse import parse_qs, urlsplit

import anyio
import httpx
import pytest

from meerkat_toolkit.connectors.slack import FailureKind, SlackError, SlackOperations
from meerkat_toolkit.httpx_client import HttpxClientNotPrepared, prepare_httpx_client
from test_home_assistant import Server, client, respond


@pytest.mark.parametrize(
    "method,kwargs,response,path,http_method,query",
    [
        (
            "auth_test",
            {},
            {
                "ok": True,
                "team_id": "T123",
                "user_id": "U123",
                "unknown": {"null": None, "false": False},
            },
            "auth.test",
            "POST",
            {},
        ),
        (
            "list_conversations_page",
            {
                "types": ["im", "private_channel"],
                "cursor": "exact+/= cursor",
                "team_id": "T123",
                "exclude_archived": True,
                "limit": 7,
            },
            {"ok": True, "channels": [], "response_metadata": {"next_cursor": "next exact+/="}},
            "conversations.list",
            "GET",
            {
                "types": ["im,private_channel"],
                "cursor": ["exact+/= cursor"],
                "team_id": ["T123"],
                "exclude_archived": ["true"],
                "limit": ["7"],
            },
        ),
        (
            "get_conversation",
            {"channel": "C123", "include_locale": True},
            {"ok": True, "channel": {"id": "C123", "is_private": False, "unknown": None}},
            "conversations.info",
            "GET",
            {"channel": ["C123"], "include_locale": ["true"], "include_num_members": ["false"]},
        ),
    ],
)
async def test_missing_discovery_public_wire(method, kwargs, response, path, http_method, query):
    async def handler(_request, writer):
        await respond(writer, response)

    async with Server(handler) as server:
        async with client(server) as transport:
            value = await getattr(SlackOperations(transport), method)(**kwargs)
            assert value == response
            assert not transport.is_closed
        assert len(server.requests) == 1
        request = server.requests[0]
        assert request.method == http_method
        assert urlsplit(request.target).path == "/api/" + path
        assert parse_qs(urlsplit(request.target).query) == query
        assert request.body == b""
    assert server.joined


@pytest.mark.parametrize(
    "method,kwargs",
    [
        ("list_conversations_page", {"types": []}),
        ("list_conversations_page", {"types": "im"}),
        ("list_conversations_page", {"types": ["im", "im"]}),
        ("list_conversations_page", {"types": ["public_channel", None]}),
        ("list_conversations_page", {"types": ["unknown"]}),
        ("list_conversations_page", {"exclude_archived": 1}),
        ("list_conversations_page", {"limit": True}),
        ("list_conversations_page", {"limit": 1000}),
        ("list_conversations_page", {"limit": 0}),
        ("list_conversations_page", {"cursor": ""}),
        ("list_conversations_page", {"cursor": "\ud800"}),
        ("list_conversations_page", {"team_id": "T123\n"}),
        ("get_conversation", {"channel": "#general"}),
        ("get_conversation", {"channel": "C123", "include_locale": 1}),
        ("get_conversation", {"channel": "C123", "include_num_members": None}),
    ],
)
async def test_invalid_selection_before_auth(method, kwargs):
    class Auth(httpx.Auth):
        async def async_auth_flow(self, request):
            raise AssertionError("invalid local input entered auth")
            yield request

    async with prepare_httpx_client(
        httpx.AsyncClient(base_url="http://127.0.0.1/", auth=Auth(), trust_env=False)
    ) as transport:
        with pytest.raises(ValueError):
            await getattr(SlackOperations(transport), method)(**kwargs)


@pytest.mark.parametrize(
    "method,kwargs,payload",
    [
        ("auth_test", {}, {"ok": True, "team_id": "T123"}),
        ("auth_test", {}, {"ok": True, "team_id": None, "user_id": "U123"}),
        ("auth_test", {}, {"ok": True, "team_id": "T123", "user_id": True}),
        ("list_conversations_page", {}, {"ok": True, "channels": None}),
        ("list_conversations_page", {}, {"ok": True, "channels": [{"id": "C1"}, {"id": 2}]}),
        ("list_conversations_page", {}, {"ok": True, "channels": [], "response_metadata": []}),
        (
            "list_conversations_page",
            {},
            {"ok": True, "channels": [], "response_metadata": {"next_cursor": False}},
        ),
        ("get_conversation", {"channel": "C123"}, {"ok": True, "channel": {"id": "COTHER"}}),
        ("get_conversation", {"channel": "C123"}, {"ok": True, "channel": {}}),
        ("get_conversation", {"channel": "C123"}, {"ok": 1, "channel": {"id": "C123"}}),
    ],
)
async def test_whole_invalid_response_refuses(method, kwargs, payload):
    async def handler(_request, writer):
        await respond(writer, payload)

    async with Server(handler) as server:
        async with client(server) as transport:
            with pytest.raises(SlackError) as caught:
                await getattr(SlackOperations(transport), method)(**kwargs)
            assert caught.value.kind is FailureKind.INVALID_RESPONSE
        assert len(server.requests) == 1
    assert server.joined


@pytest.mark.parametrize("cursor", [None, "", "opaque+/= same"])
async def test_list_preserves_full_envelope_without_following(cursor):
    payload = {
        "ok": True,
        "channels": [
            {
                "id": "C1",
                "team_id": "TOTHER",
                "private": False,
                "unknown": {"date": None, "values": [1, 2, False]},
            }
        ],
        "response_metadata": {"next_cursor": cursor, "unknown": None},
        "unknown": False,
    }

    async def handler(_request, writer):
        await respond(writer, payload)

    async with Server(handler) as server:
        async with client(server) as transport:
            assert (
                await SlackOperations(transport).list_conversations_page(team_id="TREQUEST")
                == payload
            )
        assert len(server.requests) == 1
        assert parse_qs(urlsplit(server.requests[0].target).query) == {
            "types": ["public_channel"],
            "exclude_archived": ["false"],
            "limit": ["100"],
            "team_id": ["TREQUEST"],
        }
    assert server.joined


async def test_selected_client_capture_and_mutable_filter_snapshot():
    entered, release = asyncio.Event(), asyncio.Event()

    async def pause(request):
        entered.set()
        await release.wait()

    async def handler(request, writer):
        await respond(
            writer, {"ok": True, "channels": [], "credential": request.headers["authorization"]}
        )

    async with Server(handler) as server:
        async with prepare_httpx_client(
            httpx.AsyncClient(
                base_url=server.url + "proxy/",
                headers={"Authorization": "Bearer A"},
                params={"token": "wrong", "types": "wrong", "team_id": "wrong"},
                event_hooks={"request": [pause]},
                trust_env=False,
            )
        ) as a:
            async with prepare_httpx_client(
                httpx.AsyncClient(
                    base_url=server.url, headers={"Authorization": "Bearer B"}, trust_env=False
                )
            ) as b:
                ops = SlackOperations(a)
                a.base_url = "http://unused.invalid/"
                types = ["im", "mpim"]
                task = asyncio.create_task(ops.list_conversations_page(types=types))
                await entered.wait()
                types[:] = ["public_channel"]
                release.set()
                assert (await task)["credential"] == "Bearer A"
                assert (await SlackOperations(b).list_conversations_page())[
                    "credential"
                ] == "Bearer B"
                assert not a.is_closed and not b.is_closed
        assert len(server.requests) == 2
        assert urlsplit(server.requests[0].target).path == "/proxy/api/conversations.list"
        assert parse_qs(urlsplit(server.requests[0].target).query) == {
            "types": ["im,mpim"],
            "exclude_archived": ["false"],
            "limit": ["100"],
        }
    assert server.joined


@pytest.mark.parametrize(
    "method,kwargs",
    [("auth_test", {}), ("list_conversations_page", {}), ("get_conversation", {"channel": "C1"})],
)
@pytest.mark.parametrize("cancel_scope", [False, True])
async def test_pending_cancellation_never_enters_auth(method, kwargs, cancel_scope):
    entered = []

    class Auth(httpx.Auth):
        async def async_auth_flow(self, request):
            entered.append(request)
            yield request

    async with prepare_httpx_client(
        httpx.AsyncClient(base_url="http://127.0.0.1/", auth=Auth(), trust_env=False)
    ) as transport:

        async def invoke():
            if cancel_scope:
                with anyio.CancelScope() as scope:
                    scope.cancel()
                    await getattr(SlackOperations(transport), method)(**kwargs)
                return
            asyncio.current_task().cancel()
            await getattr(SlackOperations(transport), method)(**kwargs)

        task = asyncio.create_task(invoke())
        if cancel_scope:
            await task
        else:
            with pytest.raises(asyncio.CancelledError):
                await task
        assert entered == []


@pytest.mark.parametrize(
    "method,kwargs,payload",
    [
        (
            "auth_test",
            {},
            {"ok": True, "team_id": "T1", "user_id": "U1", "bot_id": "B1", "enterprise_id": None},
        ),
        ("list_conversations_page", {}, {"ok": True, "channels": []}),
        ("get_conversation", {"channel": "C1"}, {"ok": True, "channel": {"id": "C1"}}),
    ],
)
async def test_held_close_remains_joined_through_repeated_cancellation(method, kwargs, payload):
    entered, release, finished = asyncio.Event(), asyncio.Event(), asyncio.Event()

    class HeldTransport(httpx.AsyncHTTPTransport):
        async def handle_async_request(self, request):
            response = await super().handle_async_request(request)
            original = response.stream

            class Held(httpx.AsyncByteStream):
                async def __aiter__(self):
                    async for chunk in original:
                        yield chunk

                async def aclose(self):
                    entered.set()
                    await release.wait()
                    await original.aclose()
                    finished.set()

            response.stream = Held()
            return response

    async def handler(_request, writer):
        await respond(writer, payload)

    async with Server(handler) as server:
        async with prepare_httpx_client(
            httpx.AsyncClient(
                base_url=server.url, transport=HeldTransport(retries=0), trust_env=False
            )
        ) as transport:
            task = asyncio.create_task(getattr(SlackOperations(transport), method)(**kwargs))
            try:
                await asyncio.wait_for(entered.wait(), 2)
                task.cancel()
                task.cancel()
                done, _ = await asyncio.wait({task}, timeout=0.02)
                assert not done
                release.set()
                with pytest.raises(asyncio.CancelledError):
                    await task
                assert finished.is_set()
                assert not transport.is_closed
            finally:
                release.set()
                if not task.done():
                    task.cancel()
                await asyncio.gather(task, return_exceptions=True)
        assert len(server.requests) == 1
    assert server.joined


@pytest.mark.parametrize("mode", ["duplicate", "length", "rate", "unknown", "hook"])
async def test_discovery_errors_are_safe_and_never_retried(mode):
    canary = "synthetic-private-error"

    async def handler(_request, writer):
        if mode == "duplicate":
            body = b'{"ok":true,"channels":[],"channels":[]}'
        elif mode == "length":
            body = b'{"ok":true,"channels":[]}'
        elif mode == "rate":
            writer.write(
                b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nRetry-After: 9\r\nConnection: close\r\n\r\n"
            )
            await writer.drain()
            return
        else:
            body = json.dumps({"ok": False, "error": canary}).encode()
        size = len(body) - 1 if mode == "length" else len(body)
        writer.write(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: "
            + str(size).encode()
            + b"\r\nConnection: close\r\n\r\n"
            + body
        )
        await writer.drain()

    async def hook(_request):
        if mode == "hook":
            raise ValueError(canary)

    async with Server(handler) as server:
        async with prepare_httpx_client(
            httpx.AsyncClient(base_url=server.url, event_hooks={"request": [hook]}, trust_env=False)
        ) as transport:
            with pytest.raises(SlackError) as caught:
                await SlackOperations(transport).list_conversations_page()
            assert canary not in "".join(traceback.format_exception(caught.value))
            assert caught.value.__cause__ is None and caught.value.__context__ is None
            if mode == "rate":
                assert caught.value.kind is FailureKind.RATE_LIMITED
                assert caught.value.retry_after_seconds == 9
        assert len(server.requests) == (0 if mode == "hook" else 1)
    assert server.joined


@pytest.mark.parametrize(
    "method,kwargs",
    [("auth_test", {}), ("list_conversations_page", {}), ("get_conversation", {"channel": "C1"})],
)
async def test_unprepared_refused_without_mutation(method, kwargs):
    async with httpx.AsyncClient(base_url="http://127.0.0.1/", trust_env=False) as transport:
        hooks = list(transport.event_hooks["response"])
        with pytest.raises(HttpxClientNotPrepared):
            await getattr(SlackOperations(transport), method)(**kwargs)
        assert transport.event_hooks["response"] == hooks


@pytest.mark.parametrize("matches", [True, False])
async def test_public_example_identity_compare_and_caller_pages(matches):
    spec = importlib.util.spec_from_file_location(
        "slack_discovery_example", Path(__file__).parents[1] / "examples/slack_discovery.py"
    )
    example = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(example)

    async def handler(request, writer):
        if urlsplit(request.target).path.endswith("auth.test"):
            await respond(writer, {"ok": True, "team_id": "T1", "user_id": "U1"})
        else:
            await respond(
                writer, {"ok": True, "channels": [], "response_metadata": {"next_cursor": "next"}}
            )

    async with Server(handler) as server:
        async with client(server) as transport:
            ops = SlackOperations(transport)
            if not matches:
                with pytest.raises(ValueError, match="selected Slack identity does not match"):
                    await example.observe_selected_identity(ops, team_id="T2", user_id="U1")
            else:
                await example.observe_selected_identity(ops, team_id="T1", user_id="U1")
                first = await ops.list_conversations_page(types=("im",))
                assert len(server.requests) == 2
                assert first["channels"] == []
                await ops.list_conversations_page(
                    types=("im",), cursor=first["response_metadata"]["next_cursor"]
                )
        assert len(server.requests) == (3 if matches else 1)
    assert server.joined
