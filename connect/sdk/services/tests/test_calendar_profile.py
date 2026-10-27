"""Selected Calendar reads through real MCP and a controlled HTTP transport."""
from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager
from copy import deepcopy
import importlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import httpx
from mcp import Client, StdioServerParameters

NAMESPACE = os.environ.get("TOOLKIT_SERVICES_TEST_NAMESPACE", "meerkat_toolkit")
adapter = importlib.import_module(NAMESPACE + ".adapters.services_mcp")
config_module = importlib.import_module(NAMESPACE + ".adapters.services_config")
prepare = importlib.import_module(NAMESPACE + ".httpx_client").prepare_httpx_client
SOURCE_PATH = os.environ.get("PYTHONPATH", str(Path(__file__).resolve().parents[3] / "sdk/python/src"))
PRIVATE = "provider-private-error-body"
EVENT = {"id": "event-1", "etag": '"v1"', "updated": "2026-10-04T10:00:00Z",
         "start": {"dateTime": "2026-10-05T10:00:00+02:00", "timeZone": "Europe/Stockholm"},
         "end": {"dateTime": "2026-10-05T11:00:00+02:00", "timeZone": "Europe/Stockholm"},
         "future": {"absent_is_not_null": None, "false": False}}
PAGE = {"kind": "calendar#events", "timeZone": "Europe/Stockholm", "accessRole": "reader",
        "items": [EVENT, {"id": "cancelled-1", "status": "cancelled"},
                  {"id": "all-day", "etag": '"a1"', "updated": "2026-10-04T10:00:00Z",
                   "start": {"date": "2026-10-05"}, "end": {"date": "2026-10-07"}}],
        "nextPageToken": "opaque+/=", "future": []}
PAGE["items"].append({"id": "dst-boundary", "etag": '"dst1"', "updated": "2026-10-04T10:00:00Z",
    "start": {"dateTime": "2026-10-25T02:30:00+02:00", "timeZone": "Europe/Stockholm"},
    "end": {"dateTime": "2026-10-25T02:30:00+01:00", "timeZone": "Europe/Stockholm"}})

# Host-only fixture injection. The production CLI accepts neither a URL nor a transport.
STDIO_FIXTURE = r'''
import asyncio,importlib,json,sys
from pathlib import Path
import httpx
from mcp.server.stdio import stdio_server
adapter=importlib.import_module(sys.argv[1])
config=adapter.load_configuration(sys.argv[2])
payload=json.loads(Path(sys.argv[3]).read_text())
log=Path(sys.argv[4])
def emit(**row):
    with log.open('a') as out:
        out.write(json.dumps(row)+'\n')
async def handle(request):
    emit(event='request',method=request.method,url=str(request.url),body=request.content.decode())
    return httpx.Response(200,json=payload,request=request)
class OwnedClient(httpx.AsyncClient):
    async def aclose(self):
        await super().aclose()
        emit(event='client_closed')
async def main():
    profile=await adapter.create_profile(config,client_factory=lambda **kw: OwnedClient(
        transport=httpx.MockTransport(handle),**kw))
    try:
        async with stdio_server() as (read,write):
            server=adapter.create_server(profile)
            await server.run(read,write,server.create_initialization_options())
    finally:
        await profile.aclose()
asyncio.run(main())
'''


def selection(*, calendar="team+calendar@example.invalid", operations=None, maximum=65536):
    return {"profile": "google-calendar", "operations": operations or ["list_events_page", "get_event"],
            "config": {"token_env": "CALENDAR_TOKEN", "calendar_id": calendar,
                       "timeout_seconds": 2, "max_response_bytes": maximum}}


class Provider:
    def __init__(self):
        self.payload, self.raw, self.status = deepcopy(PAGE), None, 200
        self.requests, self.events = [], []
        self.release = None
        self.closing = asyncio.Event()

    async def handle(self, request):
        self.requests.append(request)
        owner = self
        raw = self.raw if self.raw is not None else json.dumps(self.payload).encode()

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
            "location": "https://unselected.invalid/"})

    def factory(self, **kwargs):
        self.settings = kwargs
        owner = self

        class OwnedClient(httpx.AsyncClient):
            async def aclose(self):
                await super().aclose()
                owner.events.append("client_closed")

        return OwnedClient(transport=httpx.MockTransport(self.handle), **kwargs)


@asynccontextmanager
async def connected(provider, **options):
    config = config_module.validate_configuration(selection(**options))
    with patch.dict(os.environ, {"CALENDAR_TOKEN": "fixture-token"}, clear=True):
        profile = await adapter.create_profile(config, client_factory=provider.factory)
    try:
        async with Client(adapter.create_server(profile)) as client:
            yield client, profile
    finally:
        if provider.release is not None:
            provider.release.set()
        await profile.aclose()


class CalendarProfileTests(unittest.IsolatedAsyncioTestCase):
    async def test_page_fields_and_explicit_pagination_are_preserved(self):
        provider = Provider()
        async with connected(provider) as (client, _):
            tools = (await client.list_tools()).tools
            self.assertEqual([t.name for t in tools], ["google-calendar_list_events_page", "google-calendar_get_event"])
            self.assertTrue(all(t.annotations.read_only_hint and not t.annotations.destructive_hint for t in tools))
            self.assertIn("nextPageToken", tools[0].description)
            self.assertIn("incomplete", tools[0].description)
            self.assertEqual(provider.requests, [])
            arguments = dict(time_min="2026-10-01T00:00:00+02:00", time_max="2026-10-08T00:00:00+02:00",
                             single_events=True, order_by="startTime", show_deleted=True, max_results=2, q="review")
            page = await client.call_tool(tools[0].name, arguments)
            self.assertFalse(page.is_error, page)
            self.assertEqual(page.structured_content, {"value": PAGE})
            self.assertEqual(json.loads(page.content[0].text), PAGE)
            self.assertEqual(len(provider.requests), 1, "no automatic traversal")
            provider.payload = {"kind": "calendar#events", "nextSyncToken": "retain-not-consume"}
            final = await client.call_tool(tools[0].name, dict(arguments, page_token="opaque+/="))
            self.assertEqual(final.structured_content["value"], provider.payload)
            self.assertNotIn("items", final.structured_content["value"])
        first, second = provider.requests
        self.assertEqual(first.method, "GET")
        self.assertEqual(first.url.raw_path.split(b"?")[0], b"/calendar/v3/calendars/team%2Bcalendar%40example.invalid/events")
        self.assertEqual(dict(first.url.params), {"timeMin": arguments["time_min"], "timeMax": arguments["time_max"],
            "singleEvents": "true", "orderBy": "startTime", "showDeleted": "true", "maxResults": "2", "q": "review"})
        self.assertEqual(second.url.params["pageToken"], "opaque+/=")
        self.assertEqual(first.headers["authorization"], "Bearer fixture-token")
        self.assertEqual(first.content, b"")
        self.assertFalse(provider.settings["trust_env"])
        self.assertFalse(provider.settings["follow_redirects"])
        self.assertEqual(provider.events.count("response_closed"), 2)
        self.assertEqual(provider.events[-1], "client_closed")

    async def test_get_exact_event_and_minimal_cancelled_tombstone(self):
        provider = Provider()
        async with connected(provider, operations=["get_event"]) as (client, _):
            self.assertEqual(len((await client.list_tools()).tools), 1)
            for value in (EVENT, {"id": "event-1", "status": "cancelled"}):
                provider.payload = value
                result = await client.call_tool("google-calendar_get_event", {"event_id": "event-1"})
                self.assertFalse(result.is_error, result)
                self.assertEqual(result.structured_content["value"], value)
            provider.payload = dict(EVENT, id="other")
            result = await client.call_tool("google-calendar_get_event", {"event_id": "event-1"})
            self.assertEqual(result.structured_content["error"]["kind"], "invalid_response")

    async def test_no_call_can_switch_calendar_account_origin_or_add_writes(self):
        provider = Provider()
        async with connected(provider) as (client, _):
            for field in ("calendar_id", "calendarId", "base_url", "token_env", "user_id", "sync_token"):
                refused = await client.call_tool("google-calendar_list_events_page", {field: PRIVATE})
                self.assertTrue(refused.is_error)
                self.assertFalse(refused.structured_content["error"]["operation_invoked"])
            for name in ("insert_event", "patch_event", "delete_event", "list_calendars"):
                self.assertTrue((await client.call_tool("google-calendar_" + name, {})).is_error)
            for arguments in ({"max_results": 2501}, {"max_results": True}, {"single_events": "true"},
                              {"order_by": "unsupported"}, {"time_min": ""}):
                self.assertTrue((await client.call_tool("google-calendar_list_events_page", arguments)).is_error)
            # Cross-field validation remains in the existing operation owner.
            self.assertTrue((await client.call_tool("google-calendar_list_events_page", {"order_by": "startTime"})).is_error)
            self.assertEqual(provider.requests, [])

    async def test_borrowed_client_preserves_host_credential_and_fixed_calendar(self):
        provider = Provider()
        client = httpx.AsyncClient(base_url="https://www.googleapis.com/", headers={"Authorization": "Bearer host-token"},
                                   transport=httpx.MockTransport(provider.handle))
        prepare(client)
        with patch.dict(os.environ, {}, clear=True):
            profile = adapter.create_profile_from_client(config_module.validate_configuration(selection(calendar="primary")), client)
        try:
            async with Client(adapter.create_server(profile)) as mcp:
                self.assertFalse((await mcp.call_tool("google-calendar_list_events_page", {})).is_error)
                refused = await mcp.call_tool("google-calendar_get_event", {"event_id": "event-1", "calendar_id": "other"})
                self.assertTrue(refused.is_error)
            await profile.aclose()
            self.assertFalse(client.is_closed)
            self.assertEqual(len(provider.requests), 1)
            self.assertEqual(provider.requests[0].headers["authorization"], "Bearer host-token")
            self.assertEqual(provider.requests[0].url.path, "/calendar/v3/calendars/primary/events")
        finally:
            await client.aclose()

    async def test_redacted_failures_no_retry_or_partial_success(self):
        provider = Provider()
        async with connected(provider, maximum=1024) as (client, _):
            for status, kind in ((401, "authentication"), (403, "permission"), (404, "not_found"),
                                 (429, "rate_limited"), (302, "redirect")):
                provider.status, provider.payload = status, {"error": PRIVATE}
                before = len(provider.requests)
                result = await client.call_tool("google-calendar_list_events_page", {})
                error = result.structured_content["error"]
                self.assertEqual(error["kind"], kind)
                self.assertEqual(error["write_outcome"], "not_applicable")
                self.assertEqual(error["status_code"], status)
                self.assertEqual(len(provider.requests), before + 1)
                self.assertNotIn(PRIVATE, result.model_dump_json())
                self.assertNotIn("fixture-token", result.model_dump_json())
            provider.status = 200
            for raw, kind in ((b'{"kind":"calendar#events","items":[', "invalid_response"),
                              (b'{"kind":"calendar#events","items":[],"items":[]}', "invalid_response"),
                              (b"x" * 1025, "response_too_large")):
                provider.raw = raw
                result = await client.call_tool("google-calendar_list_events_page", {})
                self.assertTrue(result.is_error)
                self.assertEqual(result.structured_content["error"]["kind"], kind)
                self.assertNotIn("value", result.structured_content)

    async def test_next_page_error_keeps_first_observation_and_does_not_claim_completion(self):
        provider = Provider()
        async with connected(provider) as (client, _):
            first = await client.call_tool("google-calendar_list_events_page", {})
            provider.status, provider.payload = 429, {"error": PRIVATE}
            second = await client.call_tool("google-calendar_list_events_page", {"page_token": "opaque+/="})
            self.assertEqual(first.structured_content, {"value": PAGE})
            self.assertTrue(second.is_error)
            self.assertEqual(second.structured_content["error"]["kind"], "rate_limited")
            self.assertEqual(second.structured_content["error"]["retry_after_seconds"], 3)
            self.assertNotIn("value", second.structured_content)
            self.assertEqual(len(provider.requests), 2)

    async def test_real_stdio_uses_selected_profile_and_installed_namespace(self):
        with tempfile.TemporaryDirectory() as folder:
            base = Path(folder)
            config, payload, log = [base / name for name in ("config.json", "payload.json", "log.jsonl")]
            config.write_text(json.dumps(selection(calendar="primary", operations=["list_events_page"])))
            payload.write_text(json.dumps(PAGE))
            params = StdioServerParameters(command=sys.executable, args=["-B", "-c", STDIO_FIXTURE,
                NAMESPACE + ".adapters.services_mcp", str(config), str(payload), str(log)], env={
                    "PATH": "/usr/bin:/bin", "PYTHONPATH": SOURCE_PATH,
                    "PYTHONDONTWRITEBYTECODE": "1", "CALENDAR_TOKEN": "fixture-token"})
            async with asyncio.timeout(20):
                async with Client(params) as client:
                    self.assertEqual([tool.name for tool in (await client.list_tools()).tools], ["google-calendar_list_events_page"])
                    self.assertFalse(log.exists())
                    result = await client.call_tool("google-calendar_list_events_page", {})
                    self.assertEqual(result.structured_content, {"value": PAGE})
                    self.assertTrue((await client.call_tool("google-calendar_get_event", {"event_id": "event-1"})).is_error)
            self.assertEqual([json.loads(line) for line in log.read_text().splitlines()], [
                {"event": "request", "method": "GET", "url": "https://www.googleapis.com/calendar/v3/calendars/primary/events", "body": ""},
                {"event": "client_closed"}])

    async def test_cancellation_joins_response_before_profile_closes(self):
        provider = Provider()
        provider.release = asyncio.Event()
        async with connected(provider) as (client, profile):
            pending = asyncio.create_task(client.call_tool("google-calendar_list_events_page", {}))
            await asyncio.wait_for(provider.closing.wait(), 3)
            pending.cancel()
            close = asyncio.create_task(profile.aclose())
            try:
                await asyncio.sleep(0.02)
                self.assertFalse(close.done())
                self.assertNotIn("client_closed", provider.events)
            finally:
                provider.release.set()
                await asyncio.wait_for(asyncio.gather(pending, close, return_exceptions=True), 5)
            self.assertTrue(pending.cancelled())
            self.assertEqual(provider.events[-2:], ["response_closed", "client_closed"])


class CalendarConfigurationTests(unittest.TestCase):
    def test_inert_help_and_config_are_calendar_specific(self):
        help_value = config_module.configuration_help("google-calendar")
        self.assertEqual(set(help_value["config_fields"]), set(selection()["config"]))
        self.assertEqual([row["operation"] for row in help_value["operations"]],
                         ["list_events_page", "get_event", "insert_event", "patch_event", "delete_event"])
        self.assertEqual([row["effect"] for row in help_value["operations"]],
                         ["read", "read", "write", "write", "write"])
        text = json.dumps(help_value)
        self.assertIn("calendar.events.readonly", text)
        self.assertNotIn("gmail.readonly", text)
        self.assertNotIn("users:read", text)
        code = "import importlib,sys; m=importlib.import_module('" + NAMESPACE + ".adapters.services_mcp'); "
        code += "assert m.main(['config-help','--profile','google-calendar'])==0; assert 'httpx' not in sys.modules; assert 'mcp' not in sys.modules"
        result = subprocess.run([sys.executable, "-S", "-B", "-c", code], env={"PYTHONPATH": SOURCE_PATH}, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_calendar_id_and_profile_fields_are_exact(self):
        for calendar in ("primary", "team+calendar@example.invalid", "resource/with%literal", "日程"):
            self.assertEqual(config_module.validate_configuration(selection(calendar=calendar)).config["calendar_id"], calendar)
        for calendar in (None, True, "", ".", "..", "x\ny", "x\x7fy", "\ud800", "日" * 2731):
            with self.subTest(calendar=repr(calendar)):
                with self.assertRaises(config_module.ConfigurationError) as caught:
                    config_module.validate_configuration(selection(calendar=calendar))
                self.assertEqual(caught.exception.diagnostic, {"version": 1, "code": "invalid_calendar_id", "field_path": ["config", "calendar_id"]})
                self.assertIsNone(caught.exception.__context__)
        for field in ("allowed_channel_ids", "allowed_recipient_addresses", "user_id", "max_message_bytes", "base_url"):
            value = selection()
            value["config"][field] = []
            with self.assertRaises(config_module.ConfigurationError):
                config_module.validate_configuration(value)
        with self.assertRaises(config_module.ConfigurationError):
            config_module.validate_configuration(selection(operations=["update_event"]))


if __name__ == "__main__":
    unittest.main()
