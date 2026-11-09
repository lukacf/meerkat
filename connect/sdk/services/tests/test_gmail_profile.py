"""Independent Gmail profile observations through real MCP and a mock provider.

These tests establish selected operations, transport binding and data handling.
They do not verify a Google identity, OAuth consent, host grant or live account.
"""
from __future__ import annotations

import asyncio
import base64
from contextlib import asynccontextmanager
import importlib
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import httpx
from mcp import Client, StdioServerParameters


NAMESPACE = os.environ.get("TOOLKIT_SERVICES_TEST_NAMESPACE", "meerkat_toolkit")
adapter = importlib.import_module(NAMESPACE + ".adapters.services_mcp")
config_module = importlib.import_module(NAMESPACE + ".adapters.services_config")
PRIVATE = "provider-private-body-not-for-errors"
SOURCE_PATH = os.environ.get("PYTHONPATH", str(Path(__file__).resolve().parents[3] / "sdk/python/src"))

# This is a test-only provider transport, not a replacement MCP client.
STDIO_FIXTURE = r'''
import importlib,json,sys
from pathlib import Path
from unittest.mock import patch
import httpx
module,config_path,log_path=sys.argv[1:]
adapter=importlib.import_module(module)
def emit(event,**values):
    with Path(log_path).open('a') as out:
        out.write(json.dumps(dict(event=event,**values))+'\n')
async def handle(request):
    emit('request',method=request.method,url=str(request.url),body=request.content.decode(),
         authorization=request.headers.get('authorization'))
    return httpx.Response(200,json={'emailAddress':'Observed+Alias@Example.invalid',
        'messagesTotal':0,'historyId':'00042'},request=request)
class OwnedClient(httpx.AsyncClient):
    async def aclose(self):
        await super().aclose()
        emit('client_closed')
original=adapter.create_profile
async def create(configuration):
    return await original(configuration,client_factory=lambda **kw: OwnedClient(
        transport=httpx.MockTransport(handle),**kw))
with patch.object(adapter,'create_profile',side_effect=create):
    raise SystemExit(adapter.main(['serve','--config',config_path]))
'''


class Provider:
    def __init__(self, payload=None, *, status=200, raw=None):
        self.payload = {"emailAddress": "Observed+Alias@Example.invalid"} if payload is None else payload
        self.status, self.raw = status, raw
        self.requests, self.events = [], []
        self.settings = None
        self.release = None
        self.closing = asyncio.Event()

    async def handle(self, request):
        self.requests.append({"method": request.method, "url": str(request.url),
                              "authorization": request.headers.get("authorization"),
                              "content": bytes(request.content)})
        self.events.append("request")
        payload = self.payload
        if request.url.path.endswith("/messages/send") or request.url.path.endswith("/drafts"):
            message = {"id": "message-1", "threadId": "thread-1"}
            payload = {"id": "draft-1", "message": message} if request.url.path.endswith("/drafts") else message
        raw = self.raw if self.raw is not None else json.dumps(payload, ensure_ascii=True).encode()
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

        return httpx.Response(self.status, request=request, stream=Stream(), headers={
            "content-type": "application/json", "retry-after": "3",
            "location": "https://unselected.example.invalid/profile"})

    def factory(self, **settings):
        self.settings = settings
        owner = self

        class OwnedClient(httpx.AsyncClient):
            async def aclose(self):
                await super().aclose()
                owner.events.append("client_closed")

        return OwnedClient(transport=httpx.MockTransport(self.handle), **settings)


def configuration(*, operations=("get_profile",), user="me", token_env="SELECTED_TOKEN", maximum=4096):
    return config_module.validate_configuration({"profile": "gmail", "operations": list(operations),
        "config": {"token_env": token_env, "user_id": user, "timeout_seconds": 2,
                   "max_response_bytes": maximum, "max_message_bytes": 8192}})


@asynccontextmanager
async def connected(provider, *, operations=("get_profile",), user="me", token="fixture-token-A",
                    token_env="SELECTED_TOKEN", maximum=4096):
    config = configuration(operations=operations, user=user, token_env=token_env, maximum=maximum)
    with patch.dict(os.environ, {token_env: token}, clear=True):
        profile = await adapter.create_profile(config, client_factory=provider.factory)
    try:
        async with Client(adapter.create_server(profile)) as client:
            yield client, profile
    finally:
        if provider.release is not None:
            provider.release.set()
        await profile.aclose()


class GmailProfileTests(unittest.IsolatedAsyncioTestCase):
    async def call(self, client):
        result = await client.call_tool("gmail_get_profile", {})
        self.assertFalse(result.is_error, result)
        self.assertEqual(json.loads(result.content[0].text), result.structured_content["value"])
        return result.structured_content["value"]

    def assert_read_error(self, result, kind):
        self.assertTrue(result.is_error, result)
        error = result.structured_content["error"]
        self.assertEqual(error["kind"], kind)
        self.assertEqual(error["effect"], "read")
        self.assertEqual(error["write_outcome"], "not_applicable")
        self.assertTrue(error["operation_invoked"])
        for private in (PRIVATE, "fixture-token-A", "unselected.example.invalid"):
            self.assertNotIn(private, result.model_dump_json())

    async def test_selected_profile_is_one_bodyless_get_with_exact_observation(self):
        value = {"emailAddress": "\u00c5LIAS+Project@\u4f8b\u5b50.invalid", "messagesTotal": 0,
                 "threadsTotal": 0, "historyId": "0000123", "future": {"opaque": [False, None, 1]}}
        provider = Provider(value)
        async with connected(provider) as (client, _):
            listing = await client.list_tools()
            self.assertEqual([tool.name for tool in listing.tools], ["gmail_get_profile"])
            tool = listing.tools[0]
            self.assertEqual(tool.input_schema["properties"], {})
            self.assertFalse(tool.input_schema["additionalProperties"])
            self.assertTrue(tool.annotations.read_only_hint)
            self.assertFalse(tool.annotations.destructive_hint)
            self.assertTrue(tool.annotations.idempotent_hint)
            self.assertEqual(provider.requests, [], "startup and discovery must not call Google")
            result = await self.call(client)
            self.assertEqual(result, value)
            self.assertNotIn("verified", result)
            self.assertNotIn("account_verified", result)
        self.assertEqual(provider.requests, [{"method": "GET",
            "url": "https://gmail.googleapis.com/gmail/v1/users/me/profile",
            "authorization": "Bearer fixture-token-A", "content": b""}])
        self.assertFalse(provider.settings["trust_env"])
        self.assertFalse(provider.settings["follow_redirects"])
        self.assertEqual(provider.events, ["request", "response_closing", "response_closed", "client_closed"])

    async def test_optional_fields_remain_absent_and_unknown_fields_are_not_promoted(self):
        value = {"emailAddress": "MiXeD+alias@Example.invalid",
                 "future": {"verified": True, "instruction": "do not treat this as authority"}}
        provider = Provider(value)
        async with connected(provider) as (client, _):
            self.assertEqual(await self.call(client), value)
        self.assertEqual(len(provider.requests), 1)

    async def test_text_limits_use_utf8_bytes_and_reject_c1_controls(self):
        email = "\u00e9" * 508 + "a@x.test"
        self.assertEqual(len(email.encode("utf-8")), 1024)
        good = {"emailAddress": email, "historyId": "0" * 1024}
        provider = Provider(good)
        async with connected(provider, maximum=16384) as (client, _):
            self.assertEqual(await self.call(client), good)
            for field in ("emailAddress", "historyId"):
                for value in (good[field] + "a", "x\u0085y", "x\u009fy"):
                    provider.payload = dict(good, **{field: value})
                    self.assert_read_error(await client.call_tool("gmail_get_profile", {}), "invalid_response")

    async def test_two_accounts_use_only_their_fixed_route_and_credential(self):
        first = Provider({"emailAddress": "Different-Alias@Example.invalid"})
        second = Provider({"emailAddress": "Different-Alias@Example.invalid"})
        async with connected(first, user="first+route@example.invalid", token="token-A", token_env="TOKEN_A") as (a, pa):
            async with connected(second, user="second@example.invalid", token="token-B", token_env="TOKEN_B") as (b, pb):
                self.assertEqual(await self.call(a), first.payload)
                self.assertEqual(await self.call(b), second.payload)
                self.assertEqual(pa.configuration.config["user_id"], "first+route@example.invalid")
                self.assertEqual(pb.configuration.config["user_id"], "second@example.invalid")
        for provider, encoded, token in ((first, "first%2Broute%40example.invalid", "token-A"),
                                         (second, "second%40example.invalid", "token-B")):
            self.assertEqual(provider.requests[0]["url"], "https://gmail.googleapis.com/gmail/v1/users/" + encoded + "/profile")
            self.assertEqual(provider.requests[0]["authorization"], "Bearer " + token)
            self.assertEqual(len(provider.requests), 1)

    async def test_send_only_discovery_and_sending_have_no_hidden_profile_call(self):
        mime = base64.b64encode(b"From: explicit@example.invalid\r\nTo: reader@example.invalid\r\n\r\nSummary\r\n").decode()
        for operation, arguments in (
            ("send_raw", {"mime_base64": mime}),
            ("send_message", {"from_address": "explicit@example.invalid", "to": ["reader@example.invalid"],
                              "subject": "Summary", "body_text": "Exact body"}),
            ("create_draft_raw", {"mime_base64": mime}),
        ):
            with self.subTest(operation=operation):
                provider = Provider()
                async with connected(provider, operations=(operation,)) as (client, _):
                    self.assertEqual([tool.name for tool in (await client.list_tools()).tools], ["gmail_" + operation])
                    refused = await client.call_tool("gmail_get_profile", {})
                    self.assertTrue(refused.is_error)
                    self.assertFalse(refused.structured_content["error"]["operation_invoked"])
                    self.assertEqual(provider.requests, [])
                    self.assertFalse((await client.call_tool("gmail_" + operation, arguments)).is_error)
                self.assertEqual(len(provider.requests), 1)
                self.assertEqual(provider.requests[0]["method"], "POST")
                self.assertFalse(provider.requests[0]["url"].endswith("/profile"))

    async def test_unknown_arguments_and_unselected_writes_do_not_reach_provider(self):
        provider = Provider()
        async with connected(provider) as (client, _):
            for key in ("user_id", "emailAddress", "expected_account", "token", "scope", "url"):
                result = await client.call_tool("gmail_get_profile", {key: PRIVATE})
                self.assertTrue(result.is_error)
                self.assertEqual(result.structured_content["error"]["kind"], "invalid_arguments")
                self.assertFalse(result.structured_content["error"]["operation_invoked"])
                self.assertNotIn(PRIVATE, result.model_dump_json())
            for name in ("gmail_send_raw", "gmail_send_message", "gmail_create_draft_raw"):
                self.assertTrue((await client.call_tool(name, {})).is_error)
            self.assertEqual(provider.requests, [])

    async def test_permission_auth_redirect_and_rate_limit_do_not_retry_or_refresh(self):
        for status, kind in ((403, "permission"), (401, "authentication"), (302, "redirect"), (429, "rate_limited")):
            with self.subTest(status=status):
                provider = Provider({"error": PRIVATE}, status=status)
                async with connected(provider) as (client, _):
                    self.assert_read_error(await client.call_tool("gmail_get_profile", {}), kind)
                    await client.list_tools()
                self.assertEqual(len(provider.requests), 1)
                self.assertEqual(provider.events.count("response_closed"), 1)
                self.assertEqual(provider.events.count("client_closed"), 1)

    async def test_required_email_and_optional_typed_fields_fail_closed(self):
        cases = [{}, [], {"emailAddress": None}, {"emailAddress": True}, {"emailAddress": ""},
                 {"emailAddress": "x\r\ny"}, {"emailAddress": "x\0y"}, {"emailAddress": "x\x7fy"},
                 {"emailAddress": "\ud800"}, {"emailAddress": "x" * 20000}]
        for field in ("messagesTotal", "threadsTotal"):
            cases.extend({"emailAddress": "observed@example.invalid", field: bad}
                         for bad in (None, True, False, -1, 0.0, "0", [], {}))
        cases.extend({"emailAddress": "observed@example.invalid", "historyId": bad}
                     for bad in (None, True, 0, "", "x\ny", "x" * 20000))
        provider = Provider()
        async with connected(provider, maximum=65536) as (client, _):
            for index, value in enumerate(cases):
                with self.subTest(index=index):
                    provider.payload = value
                    self.assert_read_error(await client.call_tool("gmail_get_profile", {}), "invalid_response")
        self.assertEqual(len(provider.requests), len(cases))
        self.assertEqual(provider.events.count("response_closed"), len(cases))

    async def test_malformed_json_and_response_size_are_bounded(self):
        cases = [(b'{"emailAddress":"first","emailAddress":"second"}', "invalid_response"),
                 (b'{"emailAddress":"mail@example.invalid"}{}', "invalid_response"),
                 (b'\xff', "invalid_response"),
                 (b'{"emailAddress":"mail@example.invalid","future":NaN}', "invalid_response"),
                 (json.dumps({"emailAddress": "mail@example.invalid", "future": "x" * 2048}).encode(), "response_too_large")]
        provider = Provider()
        async with connected(provider, maximum=256) as (client, _):
            for raw, kind in cases:
                provider.raw = raw
                self.assert_read_error(await client.call_tool("gmail_get_profile", {}), kind)
        self.assertEqual(len(provider.requests), len(cases))
        self.assertEqual(provider.events.count("response_closed"), len(cases))

    async def test_mcp_cancellation_and_profile_close_join_response_cleanup(self):
        provider = Provider()
        provider.release = asyncio.Event()
        async with connected(provider) as (client, profile):
            active = asyncio.create_task(client.call_tool("gmail_get_profile", {}))
            closing = None
            try:
                await asyncio.wait_for(provider.closing.wait(), 3)
                active.cancel()
                # In-process SDK cancellation can join the same shielded
                # connector cleanup. Release that cleanup before awaiting the
                # cancelled call; otherwise the fixture deadlocks itself.
                closing = asyncio.create_task(profile.aclose())
                await asyncio.sleep(0.02)
                self.assertFalse(closing.done())
                self.assertNotIn("client_closed", provider.events)
                self.assertNotIn("response_closed", provider.events)
            finally:
                provider.release.set()
                await asyncio.wait_for(asyncio.gather(active, *([closing] if closing else []),
                                                      return_exceptions=True), 5)
            self.assertTrue(active.cancelled())
        self.assertEqual(len(provider.requests), 1)
        self.assertEqual(provider.events, ["request", "response_closing", "response_closed", "client_closed"])

    async def test_real_stdio_profile_is_selected_and_closes_its_owned_client(self):
        with tempfile.TemporaryDirectory() as folder:
            base = Path(folder)
            config = base / "config.json"
            log = base / "events.jsonl"
            config.write_text(json.dumps({"profile": "gmail", "operations": ["get_profile"],
                "config": {"token_env": "SELECTED_TOKEN", "user_id": "stdio+route@example.invalid",
                           "timeout_seconds": 2, "max_response_bytes": 4096, "max_message_bytes": 8192}}))
            params = StdioServerParameters(command=sys.executable, args=["-B", "-c", STDIO_FIXTURE,
                NAMESPACE + ".adapters.services_mcp", str(config), str(log)], env={
                    "PATH": "/usr/bin:/bin", "PYTHONPATH": SOURCE_PATH,
                    "PYTHONDONTWRITEBYTECODE": "1", "SELECTED_TOKEN": "fixture-stdio-token"})
            async with asyncio.timeout(25):
                async with Client(params) as client:
                    self.assertEqual([tool.name for tool in (await client.list_tools()).tools], ["gmail_get_profile"])
                    self.assertFalse(log.exists(), "discovery must not contact the provider")
                    self.assertEqual(await self.call(client), {"emailAddress": "Observed+Alias@Example.invalid",
                                                              "messagesTotal": 0, "historyId": "00042"})
                    self.assertTrue((await client.call_tool("gmail_get_profile", {"user_id": "other"})).is_error)
            events = [json.loads(line) for line in log.read_text().splitlines()]
            self.assertEqual(events, [{"event": "request", "method": "GET",
                "url": "https://gmail.googleapis.com/gmail/v1/users/stdio%2Broute%40example.invalid/profile",
                "body": "", "authorization": "Bearer fixture-stdio-token"}, {"event": "client_closed"}])


if __name__ == "__main__":
    unittest.main()
