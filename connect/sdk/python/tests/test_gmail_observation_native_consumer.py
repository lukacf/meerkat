"""Gmail history through the existing native-required MCP consumer.

Only private-fields correlation is exercised here, using inert credentials and
provider transport. This does not establish native grant issuance or a source
collector registration.
"""
from dataclasses import asdict
import json
import os
import unittest
from unittest.mock import patch

import httpx
from mcp import Client

from meerkat_toolkit.adapters import services_config, services_mcp
from meerkat_toolkit.adapters._connector_call import BINDING_ENV, KEY
from meerkat_toolkit.connector_auth import VerifiedConnectorAccount


class GmailHistoryNativeConsumerTests(unittest.IsolatedAsyncioTestCase):
    async def test_history_uses_existing_per_call_binding_and_refuses_changed_account(self):
        account = asdict(VerifiedConnectorAccount("https://accounts.google.com", "google-subject", "fixture-subject"))
        origin = "https://gmail.googleapis.com"
        name = "gmail_history_page"
        startup = {"version": 1, "operations": {name: {"account": account, "destination_origin": origin}}}
        supplied = {KEY: {"authorization": "Bearer inert-test-credential", "destination_origin": origin,
                          "account": account, "scopes": ["https://www.googleapis.com/auth/gmail.readonly"],
                          "expires_at": "2099-01-01T00:00:00+00:00"}}
        configuration = services_config.validate_configuration({"profile": "gmail", "operations": ["history_page"],
            "config": {"token_env": "DO_NOT_RESOLVE", "user_id": "me", "timeout_seconds": 3,
                       "max_response_bytes": 65536, "max_message_bytes": 4096}})
        requests = []
        async def provider(_transport, request):
            requests.append(request)
            return httpx.Response(200, json={"historyId": "20", "history": []}, request=request)
        with patch.dict(os.environ, {BINDING_ENV: json.dumps(startup)}, clear=True), \
             patch.object(services_mcp, "selected_token", side_effect=AssertionError("no environment credential")), \
             patch.object(httpx.AsyncHTTPTransport, "handle_async_request", provider), \
             patch("socket.socket.connect", side_effect=AssertionError("no network")), \
             patch("socket.getaddrinfo", side_effect=AssertionError("no DNS")):
            async with Client(services_mcp.create_native_server(configuration)) as client:
                tool, = (await client.list_tools()).tools
                self.assertEqual(tool.name, name)
                self.assertTrue(tool.annotations.read_only_hint)
                args = {"start_history_id": "10", "max_results": 1, "history_types": ["messageAdded"]}
                refused = {KEY: {**supplied[KEY], "account": {**account, "subject": "other-subject"}}}
                self.assertTrue((await client.call_tool(name, args, meta=refused)).is_error)
                self.assertEqual(requests, [])
                result = await client.call_tool(name, args, meta=supplied)
                self.assertFalse(result.is_error)
                self.assertEqual(result.structured_content["value"], {"historyId": "20", "history": []})
        self.assertEqual(len(requests), 1)
        self.assertEqual(requests[0].url.path, "/gmail/v1/users/me/history")
        self.assertEqual(dict(requests[0].url.params), {"startHistoryId": "10", "maxResults": "1", "historyTypes": "messageAdded"})
        self.assertEqual(requests[0].headers["Authorization"], "Bearer inert-test-credential")
