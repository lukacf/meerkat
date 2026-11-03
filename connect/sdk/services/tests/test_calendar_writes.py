"""Configured Calendar writes over real MCP with inert provider transports."""
from __future__ import annotations

import asyncio
import json
from pathlib import Path
import sys
import tempfile
import unittest

import httpx
from mcp import Client, StdioServerParameters

import test_calendar_profile as fixture
from test_services_mcp import eventually


WRITES = ["insert_event", "patch_event", "delete_event"]
BODY = {"id": "event-1", "summary": "Synthetic appointment", "location": None,
        "description": "", "attendees": [], "guestsCanModify": False,
        "start": {"dateTime": "2026-10-25T02:30:00+02:00", "timeZone": "Europe/Stockholm"},
        "end": {"dateTime": "2026-10-25T02:30:00+01:00", "timeZone": "Europe/Stockholm"},
        "recurrence": ["RRULE:FREQ=WEEKLY;COUNT=2"]}


STDIO = r'''
import asyncio, importlib, json, sys
from pathlib import Path
import httpx
from mcp.server.stdio import stdio_server
module, config_path, log_path, release_path, mode = sys.argv[1:]
adapter = importlib.import_module(module)
log, release = Path(log_path), Path(release_path)
state = None
def emit(kind, **data):
    with log.open('a') as out:
        out.write(json.dumps({'event': kind, **data}) + '\n')
class Stream(httpx.AsyncByteStream):
    def __init__(self, body):
        self.body = body
    async def __aiter__(self):
        yield self.body
    async def aclose(self):
        emit('response_closing')
        if mode == 'gated':
            async with asyncio.timeout(10):
                while not release.exists():
                    await asyncio.sleep(.01)
        emit('response_closed')
async def handle(request):
    global state
    emit('request', method=request.method, url=str(request.url),
         body=request.content.decode(), if_match=request.headers.get('if-match'))
    if request.method == 'POST':
        state = json.loads(request.content)
        state.update(etag='"v1"', updated='2026-10-04T10:00:00Z')
    elif request.method == 'PATCH':
        assert request.headers['if-match'] == state['etag']
        state.update(json.loads(request.content))
        state.update(etag='"v2"', updated='2026-10-04T10:01:00Z')
    elif request.method == 'DELETE':
        assert request.headers['if-match'] == state['etag']
        state = None
    elif request.method != 'GET':
        raise AssertionError('unexpected method')
    return httpx.Response(204 if request.method == 'DELETE' else 200,
        headers={'content-type': 'application/json'},
        stream=Stream(b'' if state is None else json.dumps(state).encode()), request=request)
class OwnedClient(httpx.AsyncClient):
    async def aclose(self):
        await super().aclose()
        emit('client_closed')
async def main():
    profile = await adapter.create_profile(adapter.load_configuration(config_path),
        client_factory=lambda **kw: OwnedClient(transport=httpx.MockTransport(handle), **kw))
    emit('module', path=adapter.__file__)
    try:
        async with stdio_server() as (read, write):
            server = adapter.create_server(profile)
            await server.run(read, write, server.create_initialization_options())
    finally:
        await profile.aclose()
asyncio.run(main())
'''


class StdioFixture:
    def __init__(self, directory, mode="normal"):
        self.base = Path(directory)
        self.log, self.release = self.base / "log.jsonl", self.base / "release"
        config = self.base / "config.json"
        config.write_text(json.dumps(fixture.selection(operations=["get_event", *WRITES])))
        self.params = StdioServerParameters(command=sys.executable,
            args=["-B", "-c", STDIO, fixture.NAMESPACE + ".adapters.services_mcp", str(config),
                  str(self.log), str(self.release), mode],
            env={"PATH": "/usr/bin:/bin", "PYTHONPATH": fixture.SOURCE_PATH,
                 "PYTHONDONTWRITEBYTECODE": "1", "CALENDAR_TOKEN": "inert-test-token"})

    def events(self, kind=None):
        rows = [] if not self.log.exists() else [json.loads(line) for line in self.log.read_text().splitlines()]
        return rows if kind is None else [row for row in rows if row["event"] == kind]


class CalendarWriteTests(unittest.IsolatedAsyncioTestCase):
    async def test_actual_stdio_create_read_patch_delete_preserves_provider_fields(self):
        for mode in ("auto", "legacy"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                host = StdioFixture(directory)
                async with asyncio.timeout(20):
                    async with Client(host.params, mode=mode) as client:
                        tools = (await client.list_tools()).tools
                        self.assertEqual([tool.name for tool in tools],
                            ["google-calendar_" + op for op in ["get_event", *WRITES]])
                        for tool in tools[1:]:
                            self.assertFalse(tool.annotations.read_only_hint)
                            self.assertTrue(tool.annotations.destructive_hint)
                            self.assertFalse(tool.annotations.idempotent_hint)
                        self.assertEqual(host.events("request"), [])
                        inserted = await client.call_tool("google-calendar_insert_event", {"event": BODY})
                        self.assertFalse(inserted.is_error, inserted)
                        expected = dict(BODY, etag='"v1"', updated="2026-10-04T10:00:00Z")
                        self.assertEqual(inserted.structured_content, {"value": expected})
                        current = await client.call_tool("google-calendar_get_event", {"event_id": "event-1"})
                        self.assertEqual(current.structured_content, inserted.structured_content)
                        patch = {"location": "Room 2", "description": None, "attendees": [],
                                 "start": {"date": "2026-10-25"}, "end": {"date": "2026-10-27"}}
                        updated = await client.call_tool("google-calendar_patch_event", {
                            "event_id": "event-1", "patch": patch, "expected_etag": expected["etag"]})
                        expected.update(patch, etag='"v2"', updated="2026-10-04T10:01:00Z")
                        self.assertEqual(updated.structured_content, {"value": expected})
                        deleted = await client.call_tool("google-calendar_delete_event", {
                            "event_id": "event-1", "expected_etag": expected["etag"]})
                        self.assertFalse(deleted.is_error, deleted)
                        self.assertEqual(deleted.structured_content, {"value": None})
                requests = host.events("request")
                self.assertEqual([row["method"] for row in requests], ["POST", "GET", "PATCH", "DELETE"])
                self.assertEqual(json.loads(requests[0]["body"]), BODY)
                self.assertEqual(json.loads(requests[2]["body"]), patch)
                self.assertTrue(all(row["url"].startswith(
                    "https://www.googleapis.com/calendar/v3/calendars/team%2Bcalendar%40example.invalid/events")
                    and "?" not in row["url"] for row in requests))
                self.assertEqual([row["if_match"] for row in requests], [None, None, '"v1"', '"v2"'])
                self.assertEqual(host.events("module"), [{"event": "module", "path": fixture.adapter.__file__}])
                self.assertEqual(len(host.events("client_closed")), 1)
                self.assertEqual(len(host.events("response_closed")), 4)

    async def test_read_only_and_operation_subset_never_admit_other_writes(self):
        for operations in (["get_event"], ["insert_event"], ["patch_event"], ["delete_event"]):
            provider = fixture.Provider()
            async with fixture.connected(provider, operations=operations) as (client, _):
                for name in set(WRITES) - set(operations):
                    result = await client.call_tool("google-calendar_" + name, {})
                    self.assertTrue(result.is_error)
                self.assertEqual(provider.requests, [])

    async def test_bad_arguments_cannot_override_calendar_or_invoke_provider(self):
        provider = fixture.Provider()
        async with fixture.connected(provider, operations=WRITES) as (client, _):
            for name, arguments in (("insert_event", {"event": []}), ("patch_event", {"event_id": "id", "patch": None}),
                                    ("delete_event", {}), ("delete_event", {"event_id": "id", "expected_etag": 3})):
                result = await client.call_tool("google-calendar_" + name, arguments)
                self.assertEqual(result.structured_content["error"]["write_outcome"], "not_attempted")
            for field in ("calendar_id", "calendarId", "base_url", "token_env", "user_id", "send_updates"):
                result = await client.call_tool("google-calendar_insert_event", {"event": BODY, field: fixture.PRIVATE})
                self.assertFalse(result.structured_content["error"]["operation_invoked"])
            self.assertEqual(provider.requests, [])

    async def test_owner_rejects_invalid_etag_and_oversized_body_without_io(self):
        provider = fixture.Provider()
        async with fixture.connected(provider, operations=WRITES) as (client, _):
            for etag in ("*", '"a", "b"', '"x\r\ny"', ""):
                result = await client.call_tool("google-calendar_patch_event", {
                    "event_id": "event-1", "patch": {}, "expected_etag": etag})
                self.assertEqual(result.structured_content["error"]["kind"],
                                 "invalid_arguments" if etag == "" else "operation_arguments")
                self.assertEqual(result.structured_content["error"]["write_outcome"],
                                 "not_attempted" if etag == "" else "unknown")
            result = await client.call_tool("google-calendar_insert_event", {"event": {"summary": "x" * 2_097_152}})
            self.assertEqual(result.structured_content["error"]["kind"], "operation_arguments")
            self.assertEqual(provider.requests, [])

    async def test_unconditional_patch_does_not_manufacture_an_etag(self):
        provider = fixture.Provider()
        provider.payload = fixture.EVENT
        async with fixture.connected(provider, operations=["patch_event"]) as (client, _):
            for optional in ({}, {"expected_etag": None}):
                result = await client.call_tool("google-calendar_patch_event", {
                    "event_id": "event-1", "patch": {"attendees": [], "location": None}, **optional})
                self.assertFalse(result.is_error, result)
        self.assertTrue(all("if-match" not in request.headers for request in provider.requests))

    async def test_provider_failures_are_redacted_unknown_and_never_retried(self):
        provider = fixture.Provider()
        async with fixture.connected(provider, operations=WRITES) as (client, _):
            for status, kind in ((401, "authentication"), (403, "permission"), (409, "conflict"),
                                 (412, "precondition"), (429, "rate_limited"), (302, "redirect"), (500, "http_status")):
                provider.status, provider.payload = status, {"error": fixture.PRIVATE}
                count = len(provider.requests)
                result = await client.call_tool("google-calendar_patch_event", {
                    "event_id": "event-1", "patch": {}, "expected_etag": '"old"'})
                self.assertEqual(result.structured_content["error"], {"kind": kind, "effect": "write",
                    "operation_invoked": True, "write_outcome": "unknown", "status_code": status, "retry_after_seconds": 3})
                self.assertNotIn(fixture.PRIVATE, result.model_dump_json())
                self.assertEqual(len(provider.requests), count + 1)

    async def test_commit_then_disconnect_is_unknown_with_one_request(self):
        provider = fixture.Provider()
        stored = {}
        async def handle(request):
            provider.requests.append(request)
            if request.method == "POST":
                event = json.loads(request.content)
                self.assertNotIn(event["id"], stored, "no repeated creation")
                stored[event["id"]] = dict(event, etag='"committed"', updated="2026-10-04T10:00:00Z")
                raise httpx.ReadError(fixture.PRIVATE, request=request)
            self.assertEqual(request.method, "GET")
            return httpx.Response(200, json=stored["event-1"], request=request)
        provider.handle = handle
        async with fixture.connected(provider, operations=["insert_event", "get_event"]) as (client, _):
            result = await client.call_tool("google-calendar_insert_event", {"event": BODY})
            self.assertEqual(result.structured_content["error"]["kind"], "transport")
            self.assertEqual(result.structured_content["error"]["write_outcome"], "unknown")
            self.assertNotIn(fixture.PRIVATE, result.model_dump_json())
            self.assertEqual(len(provider.requests), 1)
            readback = await client.call_tool("google-calendar_get_event", {"event_id": "event-1"})
            self.assertEqual(readback.structured_content, {"value": stored["event-1"]})
        self.assertEqual([request.method for request in provider.requests], ["POST", "GET"])
        self.assertEqual(list(stored), ["event-1"])
        self.assertEqual(json.loads(provider.requests[0].content), BODY)

    async def test_timeout_is_unknown_and_does_not_retry(self):
        provider = fixture.Provider()
        stopped = asyncio.Event()
        async def handle(request):
            provider.requests.append(request)
            try:
                await asyncio.sleep(10)
            finally:
                stopped.set()
        provider.handle = handle
        async with fixture.connected(provider, operations=["insert_event"]) as (client, _):
            result = await client.call_tool("google-calendar_insert_event", {"event": BODY})
            self.assertEqual(result.structured_content["error"]["kind"], "timeout")
            self.assertEqual(result.structured_content["error"]["write_outcome"], "unknown")
            self.assertTrue(stopped.is_set())
        self.assertEqual(len(provider.requests), 1)

    async def test_borrowed_clients_keep_distinct_calendar_and_credential_selection(self):
        for name in ("alice", "bob"):
            provider = fixture.Provider()
            provider.payload = fixture.EVENT
            host_client = fixture.prepare(httpx.AsyncClient(base_url="https://www.googleapis.com/",
                headers={"authorization": "Bearer inert-" + name}, transport=httpx.MockTransport(provider.handle)))
            config = fixture.config_module.validate_configuration(fixture.selection(
                calendar=name + "@example.invalid", operations=["insert_event"]))
            profile = fixture.adapter.create_profile_from_client(config, host_client)
            try:
                async with Client(fixture.adapter.create_server(profile)) as client:
                    self.assertFalse((await client.call_tool("google-calendar_insert_event", {"event": BODY})).is_error)
                    self.assertTrue((await client.call_tool("google-calendar_insert_event", {
                        "event": BODY, "calendar_id": "other"})).is_error)
                await profile.aclose()
                self.assertFalse(host_client.is_closed)
                self.assertEqual(len(provider.requests), 1)
                self.assertEqual(provider.requests[0].headers["authorization"], "Bearer inert-" + name)
                self.assertEqual(provider.requests[0].url.path,
                                 "/calendar/v3/calendars/" + name + "@example.invalid/events")
            finally:
                await host_client.aclose()

    async def test_incomplete_success_receipt_and_nonempty_delete_are_unknown(self):
        provider = fixture.Provider()
        async with fixture.connected(provider, operations=WRITES) as (client, _):
            for payload in ({"id": "event-1"}, dict(fixture.EVENT, id="wrong")):
                provider.payload = payload
                result = await client.call_tool("google-calendar_insert_event", {"event": BODY})
                self.assertEqual(result.structured_content["error"]["kind"], "invalid_response")
                self.assertEqual(result.structured_content["error"]["write_outcome"], "unknown")
            provider.status, provider.raw = 204, b"not-empty"
            result = await client.call_tool("google-calendar_delete_event", {"event_id": "event-1"})
            self.assertEqual(result.structured_content["error"]["write_outcome"], "unknown")
        self.assertEqual(len(provider.requests), 3)

    async def test_cancelled_stdio_write_joins_cleanup_before_next_call(self):
        with tempfile.TemporaryDirectory() as directory:
            host = StdioFixture(directory, "gated")
            async with asyncio.timeout(20):
                async with Client(host.params, mode="legacy") as client:
                    active = asyncio.create_task(client.call_tool("google-calendar_insert_event", {"event": BODY}))
                    following = None
                    try:
                        await eventually(lambda: host.events("response_closing"))
                        active.cancel()
                        with self.assertRaises(asyncio.CancelledError):
                            await active
                        await asyncio.wait_for(client.send_ping(), 1)
                        following = asyncio.create_task(client.call_tool("google-calendar_get_event", {"event_id": "event-1"}))
                        await asyncio.sleep(.05)
                        self.assertFalse(following.done())
                        self.assertEqual(len(host.events("request")), 1)
                    finally:
                        host.release.touch()
                        results = await asyncio.gather(*[task for task in (active, following) if task], return_exceptions=True)
                    self.assertFalse(results[-1].is_error)
            rows = host.events()
            self.assertLess(next(i for i, row in enumerate(rows) if row["event"] == "response_closed"),
                            [i for i, row in enumerate(rows) if row["event"] == "request"][1])
            self.assertEqual([row["method"] for row in host.events("request")], ["POST", "GET"])
            self.assertEqual(rows[-1]["event"], "client_closed")


if __name__ == "__main__":
    unittest.main()
