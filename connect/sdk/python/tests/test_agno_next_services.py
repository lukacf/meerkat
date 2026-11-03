"""Real donor/SDK bodies for four selected read profiles, local mock provider only."""
from __future__ import annotations

import asyncio
from contextlib import ExitStack
import hashlib
import inspect
import io
import json
import os
from pathlib import Path
import sys
import threading
import traceback
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from agno.tools.brightdata import BrightDataTools
from agno.tools.linkup import LinkupTools
from agno.tools.todoist import TodoistTools
from agno.tools.zoom import ZoomTools
from jsonschema import Draft202012Validator
from linkup import LinkupClient
from mcp import Client
from todoist_api_python.api import TodoistAPI
from todoist_api_python.models import Task
import httpx
import requests

from meerkat_toolkit.adapters.agno_mcp import create_server
from meerkat_toolkit.adapters.agno_profiles import build_profile, validate_configuration, ProfileConfigError


REFERENCE = "TOOLKIT_NEXT_SERVICE_FIXTURE_KEY"
KEY = "selected-service-token"
CANARY = "ambient-or-provider-secret"
HOSTILE = {REFERENCE: KEY, **{key: CANARY for key in (
    "BRIGHT_DATA_API_KEY", "BRIGHT_DATA_WEB_UNLOCKER_ZONE", "BRIGHT_DATA_SERP_ZONE",
    "LINKUP_API_KEY", "TODOIST_API_TOKEN", "ZOOM_ACCOUNT_ID", "ZOOM_CLIENT_ID", "ZOOM_CLIENT_SECRET",
    "REQUESTS_CA_BUNDLE", "HTTPX_BASE_URL",
)}, "HTTPS_PROXY": "http://ambient.invalid", "NETRC": "/nonexistent/ambient-netrc"}
OPERATIONS = {"bright-data": ["scrape_as_markdown", "search_engine"], "linkup": ["web_search_with_linkup"],
              "todoist": ["get_task"], "zoom": ["list_meetings", "get_meeting"]}
DONORS = {"bright-data": BrightDataTools, "linkup": LinkupTools, "todoist": TodoistTools, "zoom": ZoomTools}
HASHES = {
    "bright-data": "b9a0fe8b6558fa8fb186b63d34f1c11186afe7df8822da420f332a5e9044fd83",
    "linkup": "5e276d832f9ca8f9a5d469f22cd445a1a815aa8420c7409da2e5dbdefceaac77",
    "todoist": "62560033c0ef1a8db07b01e4946a12daec2ac01ce101f5cfbf50d7fb05711325",
    "zoom": "b931845fd52a5d6fd7abc8275e9f51d1a67f11a17c252586873df92a45573e98",
}


def config(name):
    return {"timeout": 3, "api_key_env": REFERENCE, **{
        "bright-data": {"web_unlocker_zone": "selected-web", "serp_zone": "selected-serp"},
    }.get(name, {})}


def arguments(operation):
    return {"scrape_as_markdown": {"url": "https://example.invalid/article?q=one"},
            "search_engine": {"query": "caf\u00e9 &x=1", "engine": "google", "num_results": 7, "language": "en", "country_code": "se"},
            "web_search_with_linkup": {"query": "caf\u00e9 &x=1"},
            "get_task": {"task_id": "6Xabc123"}, "list_meetings": {"user_id": "selected-user", "type": "upcoming"},
            "get_meeting": {"meeting_id": "123456789"}}[operation]


def todoist_task():
    return {"id": "6Xabc123", "content": "Review useful profile", "description": "Specific source review",
            "project_id": "project-1", "section_id": None, "parent_id": None, "labels": ["review"], "priority": 3,
            "due": {"date": "2026-10-04", "string": "tomorrow", "lang": "en", "is_recurring": False, "timezone": None},
            "deadline": None, "duration": None, "collapsed": False, "child_order": 2,
            "responsible_uid": None, "assigned_by_uid": None, "completed_at": None, "added_by_uid": "creator-1",
            "added_at": "2026-10-03T10:00:00.000000Z", "updated_at": "2026-10-03T10:01:00.000000Z"}


def provider_value(request):
    host, path = urlsplit(request.url).hostname, urlsplit(request.url).path
    if host == "api.linkup.so":
        if json.loads(request.body)["outputType"] == "sourcedAnswer":
            return {"answer": "The source says yes.", "sources": [{"name": "Source", "url": "https://example.invalid/source", "snippet": "Yes", "favicon": ""}]}
        return {"results": [{"type": "text", "name": "Useful source", "url": "https://example.invalid/source", "content": "Reviewed content", "favicon": ""}]}
    if host == "api.todoist.com":
        return todoist_task()
    if path.endswith("/meetings"):
        return {"meetings": [{"id": 123456789, "topic": "Review"}], "page_count": 1, "page_number": 1, "page_size": 30,
                "total_records": 1, "next_page_token": "https://elsewhere.invalid/next"}
    return {"id": 123456789, "topic": "Review", "type": 2, "duration": 30, "timezone": "UTC", "start_time": "2026-10-04T10:00:00Z",
            "join_url": "https://example.invalid/meeting", "settings": {"waiting_room": True}}


def response(request, value=None, *, status=200, raw=None, headers=None):
    result = requests.Response()
    result.status_code, result.url, result.request = status, request.url, request
    result.headers.update({"Content-Type": "application/json", **(headers or {})})
    if raw is None:
        raw = b"# Useful page\nCaf\xc3\xa9" if urlsplit(request.url).hostname == "api.brightdata.com" and value is None else json.dumps(value).encode()
    result.raw = io.BytesIO(raw)
    return result


def good_response(request):
    return response(request) if urlsplit(request.url).hostname == "api.brightdata.com" else response(request, provider_value(request))


class OfflineMixin:
    def setUp(self):
        super().setUp()
        self.guards = ExitStack()
        self.addCleanup(self.guards.close)
        self.guards.enter_context(patch.dict(os.environ, HOSTILE, clear=True))
        for target in ("socket.socket.connect", "socket.socket.connect_ex", "socket.getaddrinfo",
                       "requests.sessions.get_netrc_auth", "requests.get", "requests.post",
                       "httpx.HTTPTransport.handle_request", "httpx.get", "httpx.post"):
            self.guards.enter_context(patch(target, side_effect=AssertionError("unowned I/O forbidden")))

    def profile(self, name, operations=None, settings=None):
        value = build_profile(name, operations=operations or OPERATIONS[name], config=settings or config(name))
        self.addCleanup(value.close)
        return value

    def owner(self, profile):
        return inspect.unwrap(next(iter(profile.adapter._bindings.values())).entrypoint).__self__


class ConfigurationTests(OfflineMixin, unittest.TestCase):
    def test_real_pinned_donor_classes_bodies_and_sdk_instances(self):
        for name, donor in DONORS.items():
            self.assertEqual(hashlib.sha256(Path(inspect.getfile(donor)).read_bytes()).hexdigest(), HASHES[name])
            owner = self.owner(self.profile(name))
            self.assertIsInstance(owner, donor)
            for operation in OPERATIONS[name]:
                if name == "zoom":
                    self.assertIsNot(getattr(type(owner), operation), getattr(donor, operation))
                else:
                    self.assertIs(getattr(type(owner), operation), getattr(donor, operation))
            if name == "todoist":
                self.assertIsInstance(owner.api, TodoistAPI)
                self.assertIs(type(owner)._task_to_dict, TodoistTools._task_to_dict)
            if name == "linkup":
                self.assertIsInstance(owner.linkup, LinkupClient)

    def test_configuration_and_selected_secrets_are_explicit(self):
        for name in OPERATIONS:
            for delta in ({"timeout": True}, {"timeout": 61}, {"base_url": "https://other.invalid"}, {"api_key_env": "BAD\n"}):
                with self.assertRaises(ProfileConfigError):
                    validate_configuration(name, operations=OPERATIONS[name], config={**config(name), **delta})
            for value in (None, "", KEY + "\n"):
                with patch.dict(os.environ, HOSTILE, clear=True):
                    if value is None: os.environ.pop(REFERENCE)
                    else: os.environ[REFERENCE] = value
                    with self.assertRaises(ProfileConfigError): self.profile(name)
        for key in ("web_unlocker_zone", "serp_zone"):
            for value in (None, "", "ambient\n", "bad/zone", "x" * 65):
                with self.assertRaises(ProfileConfigError): self.profile("bright-data", settings={**config("bright-data"), key: value})
        for delta in ({"depth": "flash"}, {"output_type": "structured"}):
            with self.assertRaises(ProfileConfigError): self.profile("linkup", settings={**config("linkup"), **delta})

    def test_partial_build_failure_closes_httpx_client_and_requests_session(self):
        sessions, clients = [], []
        session_close, client_close = requests.Session.close, httpx.Client.close
        def close_session(owner): sessions.append(owner); session_close(owner)
        def close_client(owner): clients.append(owner); client_close(owner)
        with patch.object(requests.Session, "close", close_session), patch.object(httpx.Client, "close", close_client):
            with patch("meerkat_toolkit.adapters.agno.AgnoToolkitAdapter", side_effect=ValueError(CANARY)):
                with self.assertRaises(ProfileConfigError) as caught: self.profile("todoist")
        self.assertEqual(len(sessions), 1)
        self.assertEqual(len(clients), 1)
        self.assertTrue(clients[0].is_closed)
        self.assertNotIn(CANARY, "".join(traceback.format_exception(caught.exception)))


class ProtocolTests(OfflineMixin, unittest.IsolatedAsyncioTestCase):
    async def test_six_operations_execute_through_real_mcp_in_both_modes(self):
        for mode in ("auto", "legacy"):
            calls, closed, sdk_calls = [], [], []
            sdk_task, sdk_search = TodoistAPI.get_task, LinkupClient.search
            def actual_task(owner, task_id): sdk_calls.append("todoist"); return sdk_task(owner, task_id)
            def actual_search(owner, *a, **kw): sdk_calls.append("linkup"); return sdk_search(owner, *a, **kw)
            def send(adapter, request, **kwargs):
                calls.append(request)
                self.assertEqual(kwargs["timeout"], 3)
                self.assertEqual(kwargs["proxies"], {})
                self.assertTrue(kwargs["verify"])
                self.assertTrue(kwargs["stream"])
                self.assertEqual(adapter.max_retries.total, 0)
                self.assertEqual(request.headers["Authorization"], "Bearer " + KEY)
                self.assertNotIn(CANARY, request.url + str(request.headers))
                value = good_response(request); original = value.close
                value.close = lambda: (closed.append(value), original())[-1]
                return value
            with patch.object(requests.adapters.HTTPAdapter, "send", send), patch.object(TodoistAPI, "get_task", actual_task), patch.object(LinkupClient, "search", actual_search):
                for name in OPERATIONS:
                    os.environ[REFERENCE] = KEY
                    profile = self.profile(name)
                    os.environ[REFERENCE] = CANARY
                    async with Client(create_server(profile), mode=mode) as client:
                        listing = await client.list_tools()
                        self.assertEqual({tool.name for tool in listing.tools}, set(OPERATIONS[name]))
                        for operation in OPERATIONS[name]:
                            result = await client.call_tool(operation, arguments(operation))
                            self.assertFalse(result.is_error, (name, operation, result.model_dump_json()))
                            value = result.structured_content["value"]
                            if name == "bright-data": self.assertEqual(value, {"content": "# Useful page\nCaf\u00e9"})
                            elif name == "linkup": self.assertEqual(value["results"][0]["name"], "Useful source")
                            elif name == "todoist":
                                task = json.loads(value)
                                self.assertEqual(task["id"], "6Xabc123"); self.assertEqual(task["content"], "Review useful profile")
                                self.assertEqual(task["due"]["date"], "2026-10-04")
                                self.assertIn("6Xabc123", task["url"])
                            elif operation == "list_meetings": self.assertEqual(json.loads(value)["meetings"][0]["topic"], "Review")
                            else: self.assertEqual(json.loads(value)["duration"], 30)
            self.assertEqual(len(calls), 6); self.assertEqual(len(closed), 6)
            self.assertEqual(sdk_calls, ["linkup", "todoist"])
            self.assertEqual(calls[3].url, "https://api.todoist.com/api/v1/tasks/6Xabc123")
            self.assertEqual(parse_qs(urlsplit(calls[4].url).query), {"type": ["upcoming"]})
            self.assertEqual(json.loads(calls[2].body), {"q": "caf\u00e9 &x=1", "depth": "standard", "outputType": "searchResults"})

    async def test_published_schemas_reject_bad_arguments_without_entering_transport(self):
        cases = {"scrape_as_markdown": [{"url": "file:///etc/passwd"}],
                 "search_engine": [{"query": "x", "num_results": True}, {"query": "x", "num_results": 51}, {"query": "x", "language": "en\n"}, {"query": "x", "engine": "elsewhere"}],
                 "web_search_with_linkup": [{"query": ""}, {"query": "x", "depth": "flash"}, {"query": "x", "output_type": "structured"}],
                 "get_task": [{"task_id": "x\n"}, {"task_id": "../other"}, {"task_id": "x%2Fother"}],
                 "list_meetings": [{"user_id": "x\n"}, {"user_id": "x?token=other"}, {"type": "../token"}],
                 "get_meeting": [{"meeting_id": "123\n"}, {"meeting_id": "x"}, {"meeting_id": "123/other"}]}
        with patch.object(requests.adapters.HTTPAdapter, "send", side_effect=AssertionError("must not enter")) as send:
            for name in OPERATIONS:
                profile = self.profile(name)
                async with Client(create_server(profile)) as client:
                    for description in profile.descriptions:
                        op = description["name"]
                        for args in [*cases[op], {**arguments(op), "run_context": {}}]:
                            self.assertFalse(Draft202012Validator(description["parameters"]).is_valid(args))
                            result = await client.call_tool(op, args)
                            self.assertTrue(result.is_error); self.assertFalse(result.structured_content["error"]["entered"])
                    self.assertTrue((await client.call_tool("send_sms", {})).is_error)
            send.assert_not_called()

    async def test_brightdata_real_query_and_zone_selection_and_page_text_error_prefix(self):
        profile = self.profile("bright-data")
        calls = []
        def send(adapter, request, **kwargs): calls.append(request); return response(request, raw=b"Error scraping URL is the actual article title")
        with patch.object(requests.adapters.HTTPAdapter, "send", send):
            async with Client(create_server(profile)) as client:
                for engine in ("google", "bing", "yandex"):
                    result = await client.call_tool("search_engine", {**arguments("search_engine"), "engine": engine})
                    self.assertFalse(result.is_error); self.assertEqual(result.structured_content["value"]["content"], "Error scraping URL is the actual article title")
                self.assertFalse((await client.call_tool("scrape_as_markdown", arguments("scrape_as_markdown"))).is_error)
        for i, request in enumerate(calls):
            body = json.loads(request.body)
            self.assertEqual(body["format"], "raw"); self.assertEqual(body["data_format"], "markdown")
            self.assertEqual(body["zone"], "selected-web" if i == 3 else "selected-serp")
            query = parse_qs(urlsplit(body["url"]).query)
            if i == 0: self.assertEqual(query, {"q": ["caf\u00e9 &x=1"], "num": ["7"], "hl": ["en"], "gl": ["se"]})
            elif i < 3: self.assertEqual(query, {"q" if i == 1 else "text": ["caf\u00e9 &x=1"]})

    async def test_brightdata_malformed_targets_fail_before_provider_and_bytes_bound_before_decode(self):
        profile = self.profile("bright-data")
        with patch.object(requests.adapters.HTTPAdapter, "send", side_effect=AssertionError("must not enter")) as send:
            async with Client(create_server(profile)) as client:
                for target in ("https://user:password@example.invalid", "https://example.invalid:99999", "https://example.invalid/\\x", "https://example.invalid/\ud800"):
                    self.assertTrue((await client.call_tool("scrape_as_markdown", {"url": target})).is_error)
            send.assert_not_called()
        for raw, error in ((b"", False), (b"\xff", True), (b"\xc3\xa9" * 524289, True)):
            closed = []
            def send(adapter, request, **kwargs):
                value = response(request, raw=raw); original = value.close
                value.close = lambda: (closed.append(value), original())[-1]
                return value
            with patch.object(requests.adapters.HTTPAdapter, "send", send):
                async with Client(create_server(profile)) as client:
                    result = await client.call_tool("scrape_as_markdown", arguments("scrape_as_markdown"))
                    self.assertEqual(result.is_error, error)
            self.assertEqual(len(closed), 1)

    async def test_linkup_both_real_model_outputs_and_empty_results(self):
        profile = self.profile("linkup", settings={**config("linkup"), "depth": "deep", "output_type": "sourcedAnswer"})
        seen = []
        def send(adapter, request, **kwargs): seen.append(json.loads(request.body)); return good_response(request)
        with patch.object(requests.adapters.HTTPAdapter, "send", send):
            async with Client(create_server(profile)) as client:
                answer = await client.call_tool("web_search_with_linkup", {"query": "fixture"})
                self.assertEqual(answer.structured_content["value"]["sources"][0]["snippet"], "Yes")
                search = await client.call_tool("web_search_with_linkup", {"query": "fixture", "output_type": "searchResults", "depth": "standard"})
                self.assertEqual(search.structured_content["value"]["results"][0]["content"], "Reviewed content")
        self.assertEqual(seen[0]["depth"], "deep"); self.assertEqual(seen[1]["depth"], "standard")
        for payload in ({"results": []}, {"results": {}}, {}, {"answer": CANARY}, {"results": [{"type": "text", "name": CANARY}]}):
            with patch.object(requests.adapters.HTTPAdapter, "send", autospec=True, side_effect=lambda a,r,**kw: response(r,payload)):
                async with Client(create_server(profile)) as client:
                    result = await client.call_tool("web_search_with_linkup", {"query": "fixture", "output_type": "searchResults"})
                    self.assertEqual(result.is_error, payload != {"results": []})
                    self.assertNotIn(CANARY, result.model_dump_json())

    async def test_todoist_real_model_and_donor_formatter_and_malformed_shapes(self):
        profile = self.profile("todoist")
        formatter, tasks = TodoistTools._task_to_dict, []
        def format_task(owner, task): tasks.append(task); return formatter(owner, task)
        with patch.object(TodoistTools, "_task_to_dict", format_task), patch.object(requests.adapters.HTTPAdapter, "send", autospec=True, side_effect=lambda a,r,**kw: good_response(r)):
            async with Client(create_server(profile)) as client: self.assertFalse((await client.call_tool("get_task", arguments("get_task"))).is_error)
        self.assertEqual(len(tasks), 1); self.assertIsInstance(tasks[0], Task)
        cases = [{}, {**todoist_task(), "id": "other-task"}, {**todoist_task(), "priority": True}, {**todoist_task(), "child_order": False},
                 {**todoist_task(), "labels": {}}, {**todoist_task(), "description": False}, {**todoist_task(), "due": {}},
                 {**todoist_task(), "added_at": CANARY}, {**todoist_task(), "due": {"date": CANARY, "string": "fixture"}}]
        traces = []
        def log(*args, **kw): traces.append("".join(traceback.format_exception(*sys.exc_info())))
        with patch("agno.utils.log.logger.exception", side_effect=log):
            for payload in cases:
                with patch.object(requests.adapters.HTTPAdapter, "send", autospec=True, side_effect=lambda a,r,**kw: response(r,payload)):
                    async with Client(create_server(profile)) as client:
                        result = await client.call_tool("get_task", arguments("get_task"))
                        self.assertTrue(result.is_error); self.assertNotIn(CANARY, result.model_dump_json())
        self.assertNotIn(CANARY, "".join(traces))

    async def test_todoist_httpx_transport_cannot_route_around_selected_authority(self):
        profile = self.profile("todoist"); owner = self.owner(profile)
        with patch.object(requests.adapters.HTTPAdapter, "send", side_effect=AssertionError("must not enter")) as send:
            for url in ("https://api.todoist.com.evil.invalid/api/v1/tasks/x", "http://api.todoist.com/api/v1/tasks/x",
                        "https://api.todoist.com/api/v1/tasks/x?token=other", "https://api.todoist.com/api/v1/tasks/x%2Fy",
                        "https://other@api.todoist.com/api/v1/tasks/x", "https://api.todoist.com/api/v1/projects/x"):
                with self.assertRaises(ValueError): owner.api._client.get(url, headers={"Authorization": "Bearer " + KEY})
            with self.assertRaises(ValueError): owner.api._client.get("https://api.todoist.com/api/v1/tasks/x", headers={"Authorization": "Bearer other"})
            with self.assertRaises(ValueError): owner.api._client.post("https://api.todoist.com/api/v1/tasks/x", headers={"Authorization": "Bearer " + KEY})
            send.assert_not_called()

    async def test_zoom_absent_fields_stay_absent_and_malformed_collections_fail(self):
        profile = self.profile("zoom")
        for op, payload, error in (("list_meetings", {"meetings": []}, False), ("list_meetings", {}, True),
                                   ("list_meetings", {"meetings": {}}, True), ("list_meetings", {"meetings": [{}]}, True),
                                   ("list_meetings", {"meetings": [], "total_records": False}, True),
                                   ("get_meeting", {"id": 123456789, "topic": "Review"}, False),
                                   ("get_meeting", {}, True), ("get_meeting", {"id": True, "topic": "Review"}, True),
                                   ("get_meeting", {"id": 123456789, "topic": "Review", "duration": False}, True)):
            with patch.object(requests.adapters.HTTPAdapter, "send", autospec=True, side_effect=lambda a,r,**kw: response(r,payload)):
                async with Client(create_server(profile)) as client:
                    result = await client.call_tool(op, arguments(op))
                    self.assertEqual(result.is_error, error)
                    if not error:
                        data = json.loads(result.structured_content["value"])
                        self.assertNotIn("duration", data); self.assertNotIn("total_records", data)

    async def test_status_redirect_size_transport_and_parse_failures_close_and_recover(self):
        for name in OPERATIONS:
            profile = self.profile(name); operation = OPERATIONS[name][0]
            for mode in ("status", "redirect", "utf8", "length", "partial", "transport", "too_large"):
                calls, closed = [], []
                def send(adapter, request, **kwargs):
                    calls.append(request)
                    if len(calls) > 1: value = good_response(request)
                    elif mode == "transport": raise requests.ConnectionError(KEY + CANARY)
                    else:
                        options = {"status": 403, "raw": CANARY.encode()} if mode == "status" else \
                            {"status": 302, "headers": {"Location": "https://other-account.invalid/" + KEY}} if mode == "redirect" else \
                            {"headers": {"Content-Length": "1048577"}} if mode == "length" else \
                            {"raw": b"x" * 1048577} if mode == "too_large" else {"raw": b"\xff"}
                        value = response(request, **options)
                    original = value.close
                    value.close = lambda: (closed.append(value), original())[-1]
                    if len(calls) == 1 and mode in ("status", "redirect", "length"):
                        value.raw.read = lambda *args: self.fail("rejected response body read")
                    if len(calls) == 1 and mode == "partial":
                        def chunks(size): yield b'partial'; raise requests.ConnectionError(CANARY)
                        value.iter_content = chunks
                    return value
                with patch.object(requests.adapters.HTTPAdapter, "send", send):
                    async with Client(create_server(profile)) as client:
                        result = await client.call_tool(operation, arguments(operation))
                        self.assertTrue(result.is_error, (name, mode)); self.assertNotIn(CANARY, result.model_dump_json()); self.assertNotIn(KEY, result.model_dump_json())
                        self.assertFalse((await client.call_tool(operation, arguments(operation))).is_error)
                self.assertEqual(len(calls), 2); self.assertEqual(len(closed), 1 if mode == "transport" else 2)

    async def test_concurrent_accounts_and_retirement_stay_separate(self):
        for name in OPERATIONS:
            profiles, calls = [], []
            for key in ("first-selected", "second-selected"):
                os.environ[REFERENCE] = key; profiles.append(self.profile(name))
            os.environ[REFERENCE] = CANARY
            entered, release = threading.Event(), threading.Event()
            def send(adapter, request, **kwargs):
                token = request.headers["Authorization"]
                calls.append(token)
                if token == "Bearer first-selected":
                    entered.set()
                    if not release.wait(3): raise AssertionError("fixture wait failed")
                return good_response(request)
            op = OPERATIONS[name][0]
            with patch.object(requests.adapters.HTTPAdapter, "send", send):
                async with Client(create_server(profiles[0])) as first, Client(create_server(profiles[1])) as second:
                    pending = asyncio.create_task(first.call_tool(op, arguments(op)))
                    try:
                        async with asyncio.timeout(2):
                            while not entered.is_set(): await asyncio.sleep(0.005)
                        other = await second.call_tool(op, arguments(op)); self.assertFalse(other.is_error)
                        self.assertFalse(pending.done())
                    finally: release.set()
                    self.assertFalse((await pending).is_error)
                profiles[0].close()
                async with Client(create_server(profiles[1])) as second:
                    self.assertFalse((await second.call_tool(op, arguments(op))).is_error)
            self.assertEqual(calls, ["Bearer first-selected", "Bearer second-selected", "Bearer second-selected"])

    async def test_cancellation_joins_body_cleanup_and_then_owned_clients_retire(self):
        for name in OPERATIONS:
            os.environ[REFERENCE] = KEY
            profile = self.profile(name); owner = self.owner(profile)
            entered, release = threading.Event(), threading.Event(); closed = []
            def send(adapter, request, **kwargs):
                value = good_response(request); original = value.close
                def close():
                    entered.set()
                    if not release.wait(3): raise AssertionError("cleanup fixture timeout")
                    original(); closed.append(value)
                value.close = close
                return value
            op = OPERATIONS[name][0]
            with patch.object(requests.adapters.HTTPAdapter, "send", send):
                async with Client(create_server(profile)) as client:
                    pending = asyncio.create_task(client.call_tool(op, arguments(op)))
                    try:
                        async with asyncio.timeout(2):
                            while not entered.is_set(): await asyncio.sleep(0.005)
                        pending.cancel(); pending.cancel(); await asyncio.sleep(0.03)
                        self.assertFalse(pending.done()); self.assertEqual(closed, [])
                        if name == "todoist": self.assertFalse(owner.api._client.is_closed)
                    finally:
                        release.set()
                        with self.assertRaises(asyncio.CancelledError): await pending
            self.assertEqual(len(closed), 1)
            profile.close()
            if name == "todoist": self.assertTrue(owner.api._client.is_closed)

    async def test_pre_cancelled_calls_never_enter_transport(self):
        with patch.object(requests.adapters.HTTPAdapter, "send", side_effect=AssertionError("must not enter")) as send:
            for name in OPERATIONS:
                async with Client(create_server(self.profile(name))) as client:
                    op = OPERATIONS[name][0]
                    pending = asyncio.create_task(client.call_tool(op, arguments(op))); pending.cancel()
                    with self.assertRaises(asyncio.CancelledError): await pending
            send.assert_not_called()


if __name__ == "__main__":
    unittest.main()
