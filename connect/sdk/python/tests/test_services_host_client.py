"""Host-client composition through real MCP and inert HTTPX provider fixtures.

This proves local composition, not OAuth acquisition or live account authority.
"""

import asyncio
import base64
from contextlib import asynccontextmanager
import json
import os
import unittest
from unittest.mock import patch

import httpx
from mcp import Client

from meerkat_toolkit.adapters import services_config as config
from meerkat_toolkit.adapters import services_mcp as adapter
from meerkat_toolkit.httpx_client import prepare_httpx_client


PRIVATE = "fixture-private-host-credential"
MIME = {"mime_base64": base64.b64encode(b"To: receiver@example.invalid\r\n\r\nSummary").decode()}


def configuration(provider="gmail", operations=None, user="first@example.invalid"):
    values = {"token_env": "EXISTING_INSTALLED_REFERENCE", "timeout_seconds": 3,
              "max_response_bytes": 65536}
    if provider == "gmail":
        values.update(user_id=user, max_message_bytes=4096)
    return config.validate_configuration({"profile": provider,
        "operations": operations or (["get_profile"] if provider == "gmail" else ["auth_test"]),
        "config": values})


class HostAuth(httpx.Auth):
    """A host gets its current credential once before each explicit request."""

    def __init__(self):
        self.token = "fixture-first"
        self.entries = 0
        self.failure = None

    async def async_auth_flow(self, request):
        self.entries += 1
        if self.failure:
            raise RuntimeError(PRIVATE)
        request.headers["Authorization"] = "Bearer " + self.token
        yield request


class Provider:
    def __init__(self):
        self.requests = []
        self.events = []
        self.status = 200
        self.failure = None
        self.release = None
        self.closing = asyncio.Event()

    async def handle(self, request):
        self.requests.append(request)
        if self.failure:
            raise self.failure
        if request.url.path == "/api/auth.test":
            value = {"ok": True, "team_id": "T1FIXTURE", "user_id": "U1FIXTURE"}
        elif request.url.path.endswith("/profile"):
            value = {"emailAddress": "Observed+Alias@example.invalid"}
        elif request.url.path.endswith("/messages/send"):
            value = {"id": "message-fixture", "threadId": "thread-fixture"}
        else:
            raise AssertionError("Unexpected fixture route")
        if self.status != 200:
            value = {"error": PRIVATE}
        raw = json.dumps(value).encode()
        owner = self

        class Stream(httpx.AsyncByteStream):
            async def __aiter__(self):
                yield raw

            async def aclose(self):
                owner.events.append("response_closing")
                owner.closing.set()
                if owner.release is not None:
                    await owner.release.wait()
                owner.events.append("response_closed")

        return httpx.Response(self.status, headers={"Content-Type": "application/json",
            "Location": "https://unselected.example.invalid/"}, stream=Stream(), request=request)


@asynccontextmanager
async def host_client(provider, service="gmail", *, auth=None, prepared=True, **kwargs):
    async with httpx.AsyncClient(base_url=config.ORIGINS[service], auth=auth,
        transport=httpx.MockTransport(provider.handle), trust_env=False, **kwargs) as client:
        if prepared:
            prepare_httpx_client(client)
        yield client


@asynccontextmanager
async def connected(profile):
    try:
        async with Client(adapter.create_server(profile)) as client:
            yield client
    finally:
        await profile.aclose()


class HostClientTests(unittest.IsolatedAsyncioTestCase):
    async def test_borrow_uses_no_environment_or_client_factory_and_does_not_mutate_client(self):
        for service in ("slack", "gmail"):
            with self.subTest(service=service):
                provider, auth = Provider(), HostAuth()
                async with host_client(provider, service, auth=auth) as client:
                    hooks = tuple(client.event_hooks["response"])
                    state = (client.base_url, client.auth, client.headers.copy(), client.params)
                    with patch.object(adapter, "selected_token", side_effect=AssertionError("env route")), \
                         patch.object(type(os.environ), "__getitem__", side_effect=AssertionError("env read")), \
                         patch.object(httpx.AsyncClient, "__init__", side_effect=AssertionError("new client")):
                        profile = adapter.create_profile_from_client(configuration(service), client)
                    self.assertIs(profile.client, client)
                    self.assertEqual(hooks, tuple(client.event_hooks["response"]))
                    self.assertEqual(state, (client.base_url, client.auth, client.headers, client.params))
                    self.assertEqual(auth.entries, 0)
                    self.assertEqual(provider.requests, [])
                    async with connected(profile) as mcp:
                        self.assertEqual(len((await mcp.list_tools()).tools), 1)
                        self.assertEqual(auth.entries, 0)
                        tool = "slack_auth_test" if service == "slack" else "gmail_get_profile"
                        self.assertFalse((await mcp.call_tool(tool, {})).is_error)
                    self.assertFalse(client.is_closed)

    async def test_explicit_calls_use_rotated_host_auth_and_exact_fixed_routes(self):
        for service, tool, expected in (("slack", "slack_auth_test", "https://slack.com/api/auth.test?"),
                ("gmail", "gmail_get_profile", "https://gmail.googleapis.com/gmail/v1/users/first%40example.invalid/profile")):
            with self.subTest(service=service):
                provider, auth = Provider(), HostAuth()
                async with host_client(provider, service, auth=auth, params={"token": PRIVATE},
                                       follow_redirects=True) as client:
                    async with connected(adapter.create_profile_from_client(configuration(service), client)) as mcp:
                        first = await mcp.call_tool(tool, {})
                        auth.token = "fixture-rotated"
                        second = await mcp.call_tool(tool, {})
                        self.assertFalse(first.is_error or second.is_error)
                    self.assertEqual(auth.entries, 2)
                    self.assertEqual([str(r.url) for r in provider.requests], [expected, expected])
                    self.assertEqual([r.headers["Authorization"] for r in provider.requests],
                                     ["Bearer fixture-first", "Bearer fixture-rotated"])
                    self.assertEqual(provider.events.count("response_closed"), 2)

    async def test_invalid_clients_and_configuration_refuse_before_auth_or_mutation(self):
        provider, auth = Provider(), HostAuth()
        origins = ["http://gmail.googleapis.com/", "https://unselected.example.invalid/",
                   "https://gmail.googleapis.com/prefix/", "https://gmail.googleapis.com/?x=y",
                   "https://user:password@gmail.googleapis.com/", "https://gmail.googleapis.com/#fragment",
                   "https://gmail.googleapis.com:444/", "https://slack.com/"]
        for origin in origins:
            async with httpx.AsyncClient(base_url=origin, auth=auth,
                    transport=httpx.MockTransport(provider.handle), trust_env=False) as client:
                prepare_httpx_client(client)
                before = tuple(client.event_hooks["response"])
                with self.assertRaises(config.ConfigurationError) as error:
                    adapter.create_profile_from_client(configuration(), client)
                self.assertIsNone(error.exception.__context__)
                self.assertIsNone(error.exception.__cause__)
                self.assertEqual(str(error.exception), "Invalid service MCP configuration")
                self.assertEqual(tuple(client.event_hooks["response"]), before)
                self.assertFalse(client.is_closed)
        async with host_client(provider, auth=auth, prepared=False) as client:
            with self.assertRaises(config.ConfigurationError):
                adapter.create_profile_from_client(configuration(), client)
            self.assertEqual(client.event_hooks["response"], [])
            prepare_httpx_client(client)
            await client.aclose()
            with self.assertRaises(config.ConfigurationError):
                adapter.create_profile_from_client(configuration(), client)
        with self.assertRaises(config.ConfigurationError):
            adapter.create_profile_from_client(configuration(), object())
        async with host_client(provider, auth=auth) as client:
            bad = config.Configuration("gmail", ("unselected_operation",), configuration().config)
            with self.assertRaises(config.ConfigurationError):
                adapter.create_profile_from_client(bad, client)
            self.assertFalse(client.is_closed)
        class FailingClient(httpx.AsyncClient):
            reads = 0

            @property
            def base_url(self):
                self.reads += 1
                if self.reads == 2:
                    raise RuntimeError(PRIVATE)
                return super().base_url

        async with FailingClient(base_url=config.ORIGINS["gmail"], auth=auth,
                transport=httpx.MockTransport(provider.handle), trust_env=False) as client:
            prepare_httpx_client(client)
            with self.assertRaises(config.ConfigurationError) as error:
                adapter.create_profile_from_client(configuration(), client)
            self.assertIsNone(error.exception.__context__)
            self.assertIsNone(error.exception.__cause__)
            self.assertNotIn(PRIVATE, str(error.exception))
            self.assertFalse(client.is_closed)
        self.assertEqual(auth.entries, 0)
        self.assertEqual(provider.requests, [])

    async def test_selected_accounts_and_operations_remain_isolated(self):
        providers, auths = [Provider(), Provider()], [HostAuth(), HostAuth()]
        auths[1].token = "fixture-second-account"
        async with host_client(providers[0], auth=auths[0]) as first, \
                   host_client(providers[1], auth=auths[1]) as second:
            first_profile = adapter.create_profile_from_client(configuration(user="first@example.invalid"), first)
            second_profile = adapter.create_profile_from_client(
                configuration(user="second@example.invalid", operations=["send_raw"]), second)
            async with connected(first_profile) as read, connected(second_profile) as write:
                self.assertEqual([t.name for t in (await read.list_tools()).tools], ["gmail_get_profile"])
                self.assertEqual([t.name for t in (await write.list_tools()).tools], ["gmail_send_raw"])
                self.assertTrue((await read.call_tool("gmail_send_raw", MIME)).is_error)
                self.assertTrue((await write.call_tool("gmail_get_profile", {})).is_error)
                for forbidden in ("user_id", "base_url", "auth", "token", "token_env"):
                    refused = await read.call_tool("gmail_get_profile", {forbidden: PRIVATE})
                    self.assertTrue(refused.is_error)
                    self.assertNotIn(PRIVATE, refused.model_dump_json())
                self.assertEqual([len(p.requests) for p in providers], [0, 0])
                self.assertFalse((await read.call_tool("gmail_get_profile", {})).is_error)
                self.assertFalse((await write.call_tool("gmail_send_raw", MIME)).is_error)
            self.assertIn("/first%40example.invalid/profile", str(providers[0].requests[0].url))
            self.assertIn("/second%40example.invalid/messages/send", str(providers[1].requests[0].url))
            self.assertEqual([a.entries for a in auths], [1, 1])

    async def test_auth_failure_and_provider_rejection_never_replay_an_entered_write(self):
        provider, auth = Provider(), HostAuth()
        async with host_client(provider, auth=auth, follow_redirects=True) as client:
            async with connected(adapter.create_profile_from_client(
                    configuration(operations=["send_raw"]), client)) as mcp:
                auth.failure = True
                failed = await mcp.call_tool("gmail_send_raw", MIME)
                self.assertEqual(provider.requests, [])
                self.assertEqual(auth.entries, 1)
                self.assertTrue(failed.is_error)
                self.assertNotIn(PRIVATE, failed.model_dump_json())
                self.assertEqual(failed.structured_content["error"]["write_outcome"], "unknown")
                auth.failure = None
                for status, kind in ((401, "authentication"), (403, "permission"), (302, "redirect")):
                    provider.status = status
                    failed = await mcp.call_tool("gmail_send_raw", MIME)
                    self.assertEqual(failed.structured_content["error"]["kind"], kind)
                    self.assertEqual(failed.structured_content["error"]["write_outcome"], "unknown")
                    self.assertNotIn(PRIVATE, failed.model_dump_json())
                self.assertEqual(len(provider.requests), 3)
                self.assertEqual(auth.entries, 4)
                self.assertEqual(provider.events.count("response_closed"), 3)

    async def test_ambiguous_transport_failure_is_not_retried(self):
        provider, auth = Provider(), HostAuth()
        provider.failure = httpx.ReadError(PRIVATE)
        async with host_client(provider, auth=auth) as client:
            async with connected(adapter.create_profile_from_client(
                    configuration(operations=["send_raw"]), client)) as mcp:
                failed = await mcp.call_tool("gmail_send_raw", MIME)
                self.assertTrue(failed.is_error)
                self.assertEqual(failed.structured_content["error"]["write_outcome"], "unknown")
                self.assertNotIn(PRIVATE, failed.model_dump_json())
                self.assertEqual((auth.entries, len(provider.requests)), (1, 1))

    async def test_one_profile_closure_preserves_client_and_other_profile(self):
        provider, auth = Provider(), HostAuth()
        async with host_client(provider, auth=auth) as client:
            profile = adapter.create_profile_from_client(configuration(), client)
            other = adapter.create_profile_from_client(configuration(operations=["send_raw"]), client)
            async with connected(profile) as read, connected(other) as write:
                await profile.aclose()
                await profile.aclose()
                refused = await read.call_tool("gmail_get_profile", {})
                self.assertEqual(refused.structured_content["error"]["kind"], "closed")
                self.assertFalse(client.is_closed)
                self.assertFalse((await write.call_tool("gmail_send_raw", MIME)).is_error)
                self.assertEqual((auth.entries, len(provider.requests)), (1, 1))
            self.assertFalse(client.is_closed)

    async def test_cancelled_call_and_repeated_close_join_response_cleanup(self):
        provider, auth = Provider(), HostAuth()
        provider.release = asyncio.Event()
        async with host_client(provider, auth=auth) as client:
            profile = adapter.create_profile_from_client(configuration(operations=["send_raw"]), client)
            async with connected(profile) as mcp:
                active = asyncio.create_task(mcp.call_tool("gmail_send_raw", MIME))
                closing = None
                try:
                    await asyncio.wait_for(provider.closing.wait(), 3)
                    active.cancel()
                    closing = asyncio.create_task(profile.aclose())
                    await asyncio.sleep(.02)
                    closing.cancel()
                    await asyncio.sleep(.02)
                    closing.cancel()
                    await asyncio.sleep(.02)
                    self.assertFalse(closing.done())
                    self.assertFalse(client.is_closed)
                    self.assertNotIn("response_closed", provider.events)
                finally:
                    provider.release.set()
                    await asyncio.wait_for(asyncio.gather(active, *([closing] if closing else []),
                                                         return_exceptions=True), 5)
                self.assertTrue(active.cancelled())
                self.assertTrue(closing.cancelled())
                await profile.aclose()
                refused = await mcp.call_tool("gmail_send_raw", MIME)
                self.assertEqual(refused.structured_content["error"]["kind"], "closed")
            self.assertEqual((auth.entries, len(provider.requests)), (1, 1))
            self.assertEqual(provider.events, ["response_closing", "response_closed"])
            self.assertFalse(client.is_closed)

    async def test_close_waits_for_success_and_refuses_waiting_new_call(self):
        provider = Provider()
        provider.release = asyncio.Event()
        async with host_client(provider) as client:
            profile = adapter.create_profile_from_client(configuration(), client)
            async with connected(profile) as mcp:
                active = asyncio.create_task(mcp.call_tool("gmail_get_profile", {}))
                closing = queued = None
                try:
                    await asyncio.wait_for(provider.closing.wait(), 3)
                    closing = asyncio.create_task(profile.aclose())
                    await asyncio.sleep(.02)
                    queued = asyncio.create_task(mcp.call_tool("gmail_get_profile", {}))
                    await asyncio.sleep(.02)
                    self.assertFalse(closing.done())
                finally:
                    provider.release.set()
                    await asyncio.wait_for(asyncio.gather(active, *([closing] if closing else []),
                                                         *([queued] if queued else [])), 5)
                self.assertFalse(active.result().is_error)
                self.assertEqual(queued.result().structured_content["error"]["kind"], "closed")
                self.assertEqual(len(provider.requests), 1)
                self.assertFalse(client.is_closed)

    async def test_profile_repr_does_not_invoke_host_object_repr(self):
        class PrivateClient(httpx.AsyncClient):
            def __repr__(self):
                raise AssertionError(PRIVATE)

        async with PrivateClient(base_url=config.ORIGINS["gmail"], trust_env=False) as client:
            prepare_httpx_client(client)
            profile = adapter.create_profile_from_client(configuration(), client)
            class PrivateOperations:
                def __repr__(self):
                    raise AssertionError(PRIVATE)
            profile.operations = PrivateOperations()
            self.assertNotIn(PRIVATE, repr(profile))
            self.assertNotIn("client=", repr(profile))
            await profile.aclose()

    async def test_existing_environment_route_still_owns_and_closes_its_client(self):
        provider, settings = Provider(), []
        def factory(**kwargs):
            settings.append(kwargs)
            return httpx.AsyncClient(transport=httpx.MockTransport(provider.handle), **kwargs)
        with patch.dict(os.environ, {"EXISTING_INSTALLED_REFERENCE": "fixture-env-token"}, clear=True):
            profile = await adapter.create_profile(configuration(), client_factory=factory)
        async with connected(profile) as mcp:
            self.assertFalse((await mcp.call_tool("gmail_get_profile", {})).is_error)
        self.assertEqual(provider.requests[0].headers["Authorization"], "Bearer fixture-env-token")
        self.assertTrue(profile.client.is_closed)
        self.assertEqual(len(settings), 1)
        self.assertEqual(settings[0], {"base_url": config.ORIGINS["gmail"],
            "headers": {"Authorization": "Bearer fixture-env-token"},
            "trust_env": False, "follow_redirects": False, "timeout": None})


if __name__ == "__main__":
    unittest.main()
