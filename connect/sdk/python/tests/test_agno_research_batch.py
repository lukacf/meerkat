"""Pinned Cal.com, Jina, OpenRouteService and Zendesk source ports, offline."""
from __future__ import annotations

import asyncio
import base64
from contextlib import ExitStack
import hashlib
import inspect
import io
import json
import os
from pathlib import Path
import threading
import traceback
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from agno.tools.calcom import CalComTools
from agno.tools.jina import JinaReaderTools
from agno.tools.openrouteservice import OpenRouteServiceTools
from agno.tools.zendesk import ZendeskTools
from jsonschema import Draft202012Validator
from mcp import Client
import requests

from meerkat_toolkit.adapters.agno_mcp import create_server
from meerkat_toolkit.adapters.agno_profiles import build_profile, validate_configuration, ProfileConfigError


REFERENCE = "TOOLKIT_RESEARCH_FIXTURE_KEY"
KEY = "research-selected-token"
CANARY = "research-ambient-or-provider-secret"
HOSTILE = {REFERENCE: KEY, **{key: CANARY for key in (
    "CALCOM_API_KEY", "CALCOM_EVENT_TYPE_ID", "JINA_API_KEY", "ORS_API_KEY",
    "ZENDESK_USERNAME", "ZENDESK_PASSWORD", "ZENDESK_COMPANY_NAME", "REQUESTS_CA_BUNDLE",
)}, "HTTPS_PROXY": "http://ambient.invalid", "NETRC": "/nonexistent/ambient-netrc"}
OPERATIONS = {
    "calcom": ["get_available_slots", "get_upcoming_bookings"],
    "jina": ["read_url", "search_query"],
    "openrouteservice": ["geocode_location", "get_directions"],
    "zendesk": ["search_zendesk"],
}
DONORS = {"calcom": CalComTools, "jina": JinaReaderTools,
          "openrouteservice": OpenRouteServiceTools, "zendesk": ZendeskTools}
HASHES = {
    "calcom": "6cc4e8bd4d061a6d0ff14fd89ff825acb444812e6085d2f4613aad3ee22635e5",
    "jina": "ae8b552966500ecddf5fb4abfb75f05f50645361e92fb058afb5ff4473371883",
    "openrouteservice": "f2529e30b1996cb5f6965dd84f6d90bcbd14cc748fd882d84157fe5611f33014",
    "zendesk": "d90078c60a8efa4d21cabddd328ff6bf0558b173414bb3710eba2e30021da821",
}


def config(name):
    return {"timeout": 3, "api_key_env": REFERENCE, **{
        "calcom": {"event_type_id": 42, "user_timezone": "Europe/Stockholm"},
        "jina": {"max_content_length": 80, "search_query_content": False},
        "zendesk": {"subdomain": "selected-site", "account_email": "reader@example.invalid"},
    }.get(name, {})}


def arguments(operation):
    return {
        "get_available_slots": {"start_date": "2026-03-29", "end_date": "2026-03-29"},
        "get_upcoming_bookings": {"email": "attendee@example.invalid"},
        "read_url": {"url": "https://example.invalid/article?q=one"},
        "search_query": {"query": "caf\u00e9 &query=other"},
        "geocode_location": {"location": "Stockholm"},
        "get_directions": {"start_location": "Stockholm", "end_location": "Falun", "profile": "cycling-regular"},
        "search_zendesk": {"search_string": "caf\u00e9 &page=2"},
    }[operation]


def provider_value(request):
    parsed = urlsplit(request.url)
    if parsed.path == "/v2/slots":
        return {"status": "success", "data": {"2026-03-29": [
            {"start": "2026-03-29T00:30:00Z", "end": "2026-03-29T01:00:00Z"},
            {"start": "2026-03-29T01:30:00Z", "end": "2026-03-29T02:00:00Z"},
        ]}}
    if parsed.path == "/v2/bookings":
        return {"status": "success", "data": [{"uid": "fixture-uid", "title": "Fixture booking", "start": "2026-03-29T01:30:00Z", "status": "accepted"}],
                "pagination": {"hasMore": False, "nextCursor": None},
                "next": "https://other-account.invalid/next"}
    if parsed.hostname in ("r.jina.ai", "s.jina.ai"):
        return {"data": {"content": "Bounded donor fixture"}}
    if parsed.scheme == "https" and parsed.netloc == "api.heigit.org" and parsed.path == "/pelias/v1/search":
        location = parse_qs(parsed.query)["text"][0]
        coordinates = [18.05, 59.33] if location == "Stockholm" else [15.62, 60.61]
        return {"features": [{"geometry": {"coordinates": coordinates}, "properties": {"label": location + ", Sweden"}}]}
    if parsed.scheme == "https" and parsed.netloc == "api.heigit.org" and parsed.path == "/openrouteservice/v2/directions/cycling-regular":
        return {"routes": [{"summary": {"distance": 12345.678, "duration": 9876.54}}]}
    if parsed.hostname == "selected-site.zendesk.com" and parsed.path == "/api/v2/help_center/articles/search.json":
        return {"results": [{"body": "<h1>Title</h1><p>A &amp; B</p>"}],
                "next_page": "https://other-account.invalid/next", "count": 100}
    raise AssertionError("Unexpected provider fixture request")


def response(request, value=None, *, status=200, raw=None, headers=None):
    result = requests.Response()
    result.status_code = status
    result.url = request.url
    result.request = request
    result.headers.update({"Content-Type": "application/json", **(headers or {})})
    result.raw = io.BytesIO(raw if raw is not None else json.dumps(value).encode())
    return result


class OfflineMixin:
    def setUp(self):
        super().setUp()
        self.guards = ExitStack()
        self.addCleanup(self.guards.close)
        self.guards.enter_context(patch.dict(os.environ, HOSTILE, clear=True))
        for target in ("socket.socket.connect", "socket.socket.connect_ex", "socket.getaddrinfo"):
            self.guards.enter_context(patch(target, side_effect=AssertionError("network forbidden")))
        self.guards.enter_context(patch("requests.sessions.get_netrc_auth", side_effect=AssertionError("netrc forbidden")))
        # A port accidentally invoking the original module-level I/O must fail
        # before reaching a real network or a different credential/session.
        for target in ("httpx.get", "httpx.post", "httpx.Client.request", "requests.get"):
            self.guards.enter_context(patch(target, side_effect=AssertionError("unowned transport forbidden")))

    def profile(self, name, operations=None, settings=None):
        value = build_profile(name, operations=operations or OPERATIONS[name], config=settings or config(name))
        self.addCleanup(value.close)
        return value


class ConfigurationTests(OfflineMixin, unittest.TestCase):
    def test_pinned_real_toolkit_classes_and_original_pure_helpers_are_used(self):
        for name, donor in DONORS.items():
            with self.subTest(profile=name):
                self.assertEqual(hashlib.sha256(Path(inspect.getfile(donor)).read_bytes()).hexdigest(), HASHES[name])
                profile = self.profile(name)
                owner = inspect.unwrap(next(iter(profile.adapter._bindings.values())).entrypoint).__self__
                self.assertIsInstance(owner, donor)
                for operation in OPERATIONS[name]:
                    self.assertIsNot(getattr(type(owner), operation), getattr(donor, operation), "explicit port must not be labelled inherited")
                for helper in {"calcom": ["_get_headers"], "jina": ["_get_headers", "_truncate_content"],
                               "openrouteservice": ["_auth_headers", "_validate_profile", "_parse_geocode", "_format_directions"]}.get(name, []):
                    self.assertIs(getattr(type(owner), helper), getattr(donor, helper))

    def test_configuration_is_explicit_and_secrets_never_become_configuration(self):
        invalid = {
            "calcom": [{"event_type_id": True}, {"event_type_id": 0}, {"user_timezone": ""}, {"user_timezone": "UTC\n"}],
            "jina": [{"max_content_length": 0}, {"max_content_length": 100001}, {"search_query_content": 1}, {"base_url": "https://elsewhere.invalid"}],
            "openrouteservice": [{"base_url": "https://elsewhere.invalid"}],
            "zendesk": [{"subdomain": "site.evil.invalid"}, {"subdomain": "site\n"}, {"account_email": "user:password@example.invalid"}],
        }
        for name in OPERATIONS:
            for delta in invalid[name]:
                with self.subTest(profile=name, delta=delta), self.assertRaises(ProfileConfigError):
                    validate_configuration(name, operations=OPERATIONS[name], config={**config(name), **delta})
            with self.assertRaises(ProfileConfigError):
                validate_configuration(name, operations=["write_unknown"], config=config(name))
            for secret in (None, "", KEY + "\n"):
                with patch.dict(os.environ, HOSTILE, clear=True):
                    if secret is None:
                        os.environ.pop(REFERENCE)
                    else:
                        os.environ[REFERENCE] = secret
                    with self.assertRaises(ProfileConfigError):
                        self.profile(name)
        with self.assertRaises(ProfileConfigError):
            self.profile("calcom", settings={**config("calcom"), "user_timezone": "Unknown/Nowhere"})

    def test_partial_construction_closes_owned_session_and_redacts_error(self):
        close = requests.Session.close
        closed = []

        def tracked(session):
            closed.append(session)
            close(session)

        with patch.object(requests.Session, "close", tracked):
            for name in OPERATIONS:
                with patch("meerkat_toolkit.adapters.agno.AgnoToolkitAdapter", side_effect=ValueError(KEY)):
                    before = len(closed)
                    with self.assertRaises(ProfileConfigError) as caught:
                        self.profile(name)
                    self.assertEqual(len(closed), before + 1)
                    self.assertNotIn(KEY, "".join(traceback.format_exception(caught.exception)))
                    self.assertIsNone(caught.exception.__context__)


class ProtocolTests(OfflineMixin, unittest.IsolatedAsyncioTestCase):
    async def test_all_seven_ports_execute_real_framework_and_transport_in_both_mcp_modes(self):
        for mode in ("auto", "legacy"):
            calls, closed = [], []

            def send(adapter, request, **kwargs):
                calls.append(request)
                self.assertEqual(kwargs["timeout"], 3)
                self.assertEqual(kwargs["proxies"], {})
                self.assertTrue(kwargs["verify"])
                self.assertTrue(kwargs["stream"])
                self.assertEqual(adapter.max_retries.total, 0)
                self.assertNotIn(CANARY, request.url + str(request.headers))
                value = response(request, provider_value(request))
                original = value.close
                value.close = lambda: (closed.append(value), original())[-1]
                return value

            with patch.object(requests.adapters.HTTPAdapter, "send", send):
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
                            if operation == "get_available_slots":
                                self.assertEqual(value, "Available slots: 2026-03-29 01:30 CET, 2026-03-29 03:30 CEST")
                            elif operation == "get_upcoming_bookings":
                                self.assertEqual(value, "Upcoming bookings:\nuid: fixture-uid, Title: Fixture booking, Time: 2026-03-29 03:30 CEST, Status: accepted")
                            elif name == "jina":
                                self.assertEqual(value, str({"data": {"content": "Bounded donor fixture"}}))
                            elif operation == "geocode_location":
                                self.assertEqual(value, {"query": "Stockholm", "label": "Stockholm, Sweden", "latitude": 59.33, "longitude": 18.05})
                            elif operation == "get_directions":
                                self.assertEqual(value, {"start": "Stockholm, Sweden", "end": "Falun, Sweden", "profile": "cycling-regular",
                                                         "distance_km": 12.35, "distance_meters": 12345.7, "duration_seconds": 9876.5, "duration_minutes": 164.6})
                            else:
                                self.assertEqual(json.loads(value), ["TitleA &amp; B"])
            self.assertEqual(len(calls), 9)
            self.assertEqual(len(closed), 9)
            for request in calls:
                parsed = urlsplit(request.url)
                query = parse_qs(parsed.query)
                if parsed.hostname == "api.cal.com":
                    self.assertEqual(request.headers["Authorization"], "Bearer " + KEY)
                    if parsed.path.endswith("available"):
                        self.fail("Legacy Cal.com slot route")
                    if parsed.path == "/v2/slots":
                        self.assertEqual(request.headers["cal-api-version"], "2024-09-04")
                        self.assertEqual(query, {"start": ["2026-03-29T00:00:00Z"], "end": ["2026-03-29T23:59:59Z"], "eventTypeId": ["42"], "format": ["range"], "timeZone": ["UTC"]})
                    else:
                        self.assertEqual(request.headers["cal-api-version"], "2026-05-01")
                        self.assertEqual(query, {"status": ["upcoming"], "limit": ["100"], "attendeeEmail": ["attendee@example.invalid"]})
                elif parsed.hostname in ("r.jina.ai", "s.jina.ai"):
                    self.assertEqual(request.headers["Authorization"], "Bearer " + KEY)
                    self.assertEqual(request.headers["X-Timeout"], "3")
                    self.assertEqual(request.headers["Accept"], "application/json")
                    if parsed.hostname == "s.jina.ai":
                        self.assertEqual(request.method, "POST")
                        self.assertEqual(request.headers["X-Respond-With"], "no-content")
                        self.assertEqual(json.loads(request.body), {"q": arguments("search_query")["query"]})
                    else:
                        self.assertEqual(request.url, "https://r.jina.ai/https://example.invalid/article?q=one")
                elif parsed.hostname == "api.heigit.org":
                    self.assertEqual(parsed.scheme, "https")
                    self.assertEqual(parsed.netloc, "api.heigit.org")
                    if parsed.path == "/pelias/v1/search":
                        self.assertEqual(request.method, "GET")
                        self.assertIn(query["text"], (["Stockholm"], ["Falun"]))
                        self.assertEqual(query, {"api_key": [KEY], "text": query["text"], "size": ["1"]})
                        self.assertNotIn("Authorization", request.headers)
                        self.assertIsNone(request.body)
                    else:
                        self.assertEqual(parsed.path, "/openrouteservice/v2/directions/cycling-regular")
                        self.assertEqual(query, {})
                        self.assertEqual(request.headers["Authorization"], KEY)
                        self.assertEqual(request.headers["Content-Type"], "application/json")
                        self.assertEqual(request.method, "POST")
                        self.assertEqual(json.loads(request.body), {"coordinates": [[18.05, 59.33], [15.62, 60.61]]})
                else:
                    self.assertEqual(parsed.hostname, "selected-site.zendesk.com")
                    self.assertEqual(query, {"query": [arguments("search_zendesk")["search_string"]]})
                    decoded = base64.b64decode(request.headers["Authorization"].split(" ")[1]).decode()
                    self.assertEqual(decoded, "reader@example.invalid/token:" + KEY)

    async def test_directions_only_uses_two_encoded_pelias_reads_then_fixed_route(self):
        for mode in ("auto", "legacy"):
            profile = self.profile("openrouteservice", operations=["get_directions"])
            # The explicit account was bound at construction, before ambient mutation.
            os.environ[REFERENCE] = CANARY
            calls, closed = [], []
            start, end = "Stockholm", "M\u00fcnchen &api_key=other&size=99"

            def send(adapter, request, **kwargs):
                calls.append(request)
                value = response(request, provider_value(request))
                original = value.close
                value.close = lambda: (closed.append(value), original())[-1]
                return value

            with patch.object(requests.adapters.HTTPAdapter, "send", send):
                async with Client(create_server(profile), mode=mode) as client:
                    listing = await client.list_tools()
                    self.assertEqual([tool.name for tool in listing.tools], ["get_directions"])
                    self.assertTrue((await client.call_tool("geocode_location", {"location": start})).is_error)
                    self.assertEqual(calls, [])
                    result = await client.call_tool("get_directions", {
                        "start_location": start, "end_location": end, "profile": "cycling-regular",
                    })
                    self.assertFalse(result.is_error, result.model_dump_json())
                    self.assertEqual(result.structured_content["value"], {
                        "start": start + ", Sweden", "end": end + ", Sweden", "profile": "cycling-regular",
                        "distance_km": 12.35, "distance_meters": 12345.7,
                        "duration_seconds": 9876.5, "duration_minutes": 164.6,
                    })
            self.assertEqual(len(calls), 3)
            self.assertEqual(len(closed), 3)
            self.assertEqual([(request.method, urlsplit(request.url).scheme, urlsplit(request.url).netloc,
                               urlsplit(request.url).path) for request in calls], [
                ("GET", "https", "api.heigit.org", "/pelias/v1/search"),
                ("GET", "https", "api.heigit.org", "/pelias/v1/search"),
                ("POST", "https", "api.heigit.org", "/openrouteservice/v2/directions/cycling-regular"),
            ])
            for request, location in zip(calls[:2], (start, end)):
                self.assertEqual(parse_qs(urlsplit(request.url).query), {"api_key": [KEY], "text": [location], "size": ["1"]})
                self.assertNotIn("Authorization", request.headers)
                self.assertIsNone(request.body)
            self.assertEqual(urlsplit(calls[2].url).query, "")
            self.assertEqual(calls[2].headers["Authorization"], KEY)
            self.assertEqual(calls[2].headers["Content-Type"], "application/json")
            self.assertEqual(json.loads(calls[2].body), {"coordinates": [[18.05, 59.33], [15.62, 60.61]]})
            self.assertNotIn(CANARY, str([(request.url, dict(request.headers)) for request in calls]))
            os.environ[REFERENCE] = KEY

    async def test_published_schema_rejects_bounds_and_unselected_operations_before_http(self):
        cases = {
            "calcom": ("get_available_slots", [{"start_date": "2026-03-29\n", "end_date": "2026-03-30"}, {"start_date": "2038-01-01", "end_date": "2038-01-01"}]),
            "jina": ("read_url", [{"url": "file:///etc/passwd"}, {"url": "https://example.invalid/" + "a" * 4096}]),
            "openrouteservice": ("get_directions", [{"start_location": "x", "end_location": "y", "profile": "../matrix"}, {"start_location": "x", "end_location": ""}]),
            "zendesk": ("search_zendesk", [{"search_string": ""}, {"search_string": "a" * 1025}]),
        }
        with patch.object(requests.adapters.HTTPAdapter, "send", side_effect=AssertionError("must not enter")) as send:
            for name, (operation, values) in cases.items():
                profile = self.profile(name)
                schema = next(item["parameters"] for item in profile.descriptions if item["name"] == operation)
                async with Client(create_server(profile)) as client:
                    for args in [*values, {**arguments(operation), "run_context": {}}]:
                        self.assertFalse(Draft202012Validator(schema).is_valid(args))
                        result = await client.call_tool(operation, args)
                        self.assertTrue(result.is_error)
                        self.assertFalse(result.structured_content["error"]["entered"])
                    self.assertTrue((await client.call_tool("create_booking", {})).is_error)
                    self.assertTrue((await client.call_tool("get_distance_matrix", {})).is_error)
            send.assert_not_called()

    async def test_calcom_real_dates_order_span_and_naive_timestamps_fail_without_local_timezone_guessing(self):
        profile = self.profile("calcom")
        with patch.object(requests.adapters.HTTPAdapter, "send", side_effect=AssertionError("must not enter")) as send:
            async with Client(create_server(profile)) as client:
                for start, end in (("2026-02-29", "2026-03-01"), ("2026-04-31", "2026-05-01"),
                                   ("2026-03-30", "2026-03-29"), ("2026-01-01", "2026-02-01")):
                    self.assertTrue((await client.call_tool("get_available_slots", {"start_date": start, "end_date": end})).is_error)
            send.assert_not_called()
        for timestamp in ("2026-03-29T01:30:00", "2038-01-01T00:00:00Z",
                          "2037-12-31T23:30:00-12:00", "2037-12-31T23:30:00Z", "bad-" + CANARY):
            def send(adapter, request, **kwargs):
                return response(request, {"status": "success", "data": {"2026-03-29": [{"start": timestamp, "end": "2026-03-29T04:00:00Z"}]}})
            with patch.object(requests.adapters.HTTPAdapter, "send", send):
                async with Client(create_server(profile)) as client:
                    result = await client.call_tool("get_available_slots", arguments("get_available_slots"))
                    self.assertTrue(result.is_error)
                    self.assertNotIn(CANARY, result.model_dump_json())
        with patch.object(requests.adapters.HTTPAdapter, "send", side_effect=lambda a, r, **kw: response(r, provider_value(r), status=201), autospec=True):
            async with Client(create_server(profile)) as client:
                self.assertTrue((await client.call_tool("get_available_slots", arguments("get_available_slots"))).is_error)

    async def test_calcom_31_day_boundary_and_empty_upcoming_keep_original_output(self):
        profile = self.profile("calcom")
        calls = []
        original = CalComTools._convert_to_user_timezone
        conversions = []

        def convert(owner, instant):
            conversions.append(instant)
            return original(owner, instant)

        def send(adapter, request, **kwargs):
            calls.append(request)
            return response(request, {"status": "success", "data": [], "pagination": {"hasMore": False, "nextCursor": None}} if urlsplit(request.url).path == "/v2/bookings" else provider_value(request))

        with patch.object(requests.adapters.HTTPAdapter, "send", send), patch.object(CalComTools, "_convert_to_user_timezone", convert):
            async with Client(create_server(profile)) as client:
                self.assertFalse((await client.call_tool("get_available_slots", {"start_date": "2026-03-01", "end_date": "2026-03-31"})).is_error)
                upcoming = await client.call_tool("get_upcoming_bookings", {})
                self.assertEqual(upcoming.structured_content["value"], "No upcoming bookings found.")
        self.assertEqual(len(conversions), 2)
        self.assertEqual(parse_qs(urlsplit(calls[-1].url).query), {"status": ["upcoming"], "limit": ["100"]})

    async def test_jina_json_scalar_stringification_and_truncation_are_donor_behavior(self):
        profile = self.profile("jina", settings={**config("jina"), "max_content_length": 12, "search_query_content": True})
        for value in ({"a": "x" * 100}, [1, "two"], "Error reading URL: real article text", 42, True, None):
            calls = []
            def send(adapter, request, **kwargs):
                calls.append(request)
                return response(request, value)
            with patch.object(requests.adapters.HTTPAdapter, "send", send):
                async with Client(create_server(profile)) as client:
                    result = await client.call_tool("search_query", {"query": "fixture"})
                    self.assertFalse(result.is_error)
                    expected = str(value)
                    if len(expected) > 12:
                        expected = expected[:12] + "... (content truncated)"
                    self.assertEqual(result.structured_content["value"], expected)
                    self.assertNotIn("X-Respond-With", calls[0].headers)
        with patch.object(requests.adapters.HTTPAdapter, "send", side_effect=lambda a, r, **kw: response(r, raw=b"plain text", headers={"Content-Type": "text/plain"}), autospec=True):
            async with Client(create_server(profile)) as client:
                self.assertTrue((await client.call_tool("read_url", arguments("read_url"))).is_error)

    async def test_jina_userinfo_bad_ports_and_surrogates_do_not_reach_provider(self):
        profile = self.profile("jina")
        with patch.object(requests.adapters.HTTPAdapter, "send", side_effect=AssertionError("must not enter")) as send:
            async with Client(create_server(profile)) as client:
                for url in ("https://user:password@example.invalid", "https://example.invalid:99999", "https://example.invalid:bad", "https://example.invalid/\\evil", "https://example.invalid/\ud800"):
                    self.assertTrue((await client.call_tool("read_url", {"url": url})).is_error)
            send.assert_not_called()

    async def test_route_failures_stop_fanout_and_never_follow_provider_urls(self):
        profile = self.profile("openrouteservice")
        for fail_at, body in ((1, {"features": []}), (2, {"features": [{"geometry": {"coordinates": [181, 50]}}]}),
                              (3, {"routes": []})):
            calls = []
            def send(adapter, request, **kwargs):
                calls.append(request)
                return response(request, body if len(calls) == fail_at else provider_value(request))
            with patch.object(requests.adapters.HTTPAdapter, "send", send):
                async with Client(create_server(profile)) as client:
                    result = await client.call_tool("get_directions", arguments("get_directions"))
                    self.assertTrue(result.is_error)
            self.assertEqual(len(calls), fail_at)
            self.assertTrue(all(urlsplit(request.url).scheme == "https" and urlsplit(request.url).netloc == "api.heigit.org"
                                for request in calls))
            self.assertEqual([urlsplit(request.url).path for request in calls], [
                "/pelias/v1/search", "/pelias/v1/search", "/openrouteservice/v2/directions/cycling-regular",
            ][:fail_at])

    async def test_route_redirect_at_each_hop_closes_without_fallback_or_credential_disclosure(self):
        profile = self.profile("openrouteservice", operations=["get_directions"])
        for fail_at in (1, 2, 3):
            calls, closed = [], []

            def send(adapter, request, **kwargs):
                calls.append(request)
                self.assertEqual(adapter.max_retries.total, 0)
                value = response(request, status=302, headers={
                    "Location": "https://api.openrouteservice.org/geocode/search?api_key=" + KEY,
                }) if len(calls) == fail_at else response(request, provider_value(request))
                original = value.close
                value.close = lambda: (closed.append(value), original())[-1]
                if len(calls) == fail_at:
                    value.raw.read = lambda *args: self.fail("redirect body must not be read")
                return value

            with patch.object(requests.adapters.HTTPAdapter, "send", send):
                async with Client(create_server(profile)) as client:
                    result = await client.call_tool("get_directions", arguments("get_directions"))
                    self.assertTrue(result.is_error)
                    self.assertNotIn(KEY, result.model_dump_json())
                    self.assertNotIn("api.openrouteservice.org", result.model_dump_json())
            self.assertEqual(len(calls), fail_at)
            self.assertEqual(len(closed), fail_at)
            self.assertEqual([(urlsplit(request.url).scheme, urlsplit(request.url).netloc) for request in calls],
                             [("https", "api.heigit.org")] * fail_at)
            self.assertEqual([urlsplit(request.url).path for request in calls], [
                "/pelias/v1/search", "/pelias/v1/search", "/openrouteservice/v2/directions/cycling-regular",
            ][:fail_at])

    async def test_account_selection_and_other_profile_closure_do_not_cross_accounts(self):
        for name in OPERATIONS:
            profiles, seen = [], []
            for key in ("first-selected", "second-selected"):
                os.environ[REFERENCE] = key
                profiles.append(self.profile(name))
            os.environ[REFERENCE] = CANARY
            def send(adapter, request, **kwargs):
                auth = request.headers.get("Authorization")
                if name == "openrouteservice":
                    auth = parse_qs(urlsplit(request.url).query)["api_key"][0]
                if name == "zendesk":
                    auth = base64.b64decode(auth.split(" ")[1]).decode()
                seen.append(auth)
                return response(request, provider_value(request))
            operation = OPERATIONS[name][0]
            with patch.object(requests.adapters.HTTPAdapter, "send", send):
                for profile in profiles:
                    async with Client(create_server(profile)) as client:
                        self.assertFalse((await client.call_tool(operation, arguments(operation))).is_error)
                profiles[0].close()
                async with Client(create_server(profiles[1])) as client:
                    self.assertFalse((await client.call_tool(operation, arguments(operation))).is_error)
            self.assertIn("first-selected", seen[0])
            self.assertIn("second-selected", seen[1])
            self.assertEqual(seen[1], seen[2])
            self.assertNotIn(CANARY, str(seen))

    async def test_all_profiles_fail_closed_close_response_and_recover_after_errors(self):
        for name in OPERATIONS:
            profile = self.profile(name)
            operation = OPERATIONS[name][0]
            for mode in ("status", "redirect", "json", "utf8", "large", "length", "transport", "partial"):
                calls, closed = [], []
                def send(adapter, request, **kwargs):
                    calls.append(request)
                    if len(calls) > 1:
                        options = {"value": provider_value(request)}
                    elif mode == "transport":
                        raise requests.ConnectionError(KEY + CANARY)
                    else:
                        options = {"status": 403, "raw": CANARY.encode()} if mode == "status" else \
                            {"status": 302, "headers": {"Location": "https://other-account.invalid/" + KEY}} if mode == "redirect" else \
                            {"raw": b"[" + b" " * 1_048_576 + b"]"} if mode == "large" else \
                            {"headers": {"Content-Length": "1048577"}} if mode == "length" else \
                            {"raw": b'\xff'} if mode == "utf8" else {"raw": CANARY.encode()}
                    value = response(request, **options)
                    original = value.close
                    value.close = lambda: (closed.append(value), original())[-1]
                    if len(calls) == 1 and mode in ("status", "redirect", "length"):
                        value.raw.read = lambda *args: self.fail("rejected body read")
                    if len(calls) == 1 and mode == "partial":
                        def chunks(size):
                            yield b'{"partial":'
                            raise requests.ConnectionError(CANARY)
                        value.iter_content = chunks
                    return value
                with patch.object(requests.adapters.HTTPAdapter, "send", send):
                    async with Client(create_server(profile)) as client:
                        result = await client.call_tool(operation, arguments(operation))
                        self.assertTrue(result.is_error, (name, mode))
                        self.assertNotIn(KEY, result.model_dump_json())
                        self.assertNotIn(CANARY, result.model_dump_json())
                        self.assertFalse((await client.call_tool(operation, arguments(operation))).is_error)
                self.assertEqual(len(calls), 2)
                self.assertEqual(len(closed), 1 if mode == "transport" else 2)

    async def test_cancellation_joins_owned_response_cleanup_for_all_four_profiles(self):
        for name in OPERATIONS:
            with self.subTest(profile=name):
                profile = self.profile(name)
                operation = OPERATIONS[name][0]
                entered, release = threading.Event(), threading.Event()
                closed = []
                def send(adapter, request, **kwargs):
                    value = response(request, provider_value(request))
                    original = value.close
                    def close():
                        entered.set()
                        if not release.wait(3):
                            raise AssertionError("cleanup fixture timed out")
                        original()
                        closed.append(value)
                    value.close = close
                    return value
                with patch.object(requests.adapters.HTTPAdapter, "send", send):
                    async with Client(create_server(profile)) as client:
                        owner = asyncio.create_task(client.call_tool(operation, arguments(operation)))
                        try:
                            async with asyncio.timeout(2):
                                while not entered.is_set():
                                    await asyncio.sleep(0.005)
                            owner.cancel(); owner.cancel()
                            await asyncio.sleep(0.03)
                            self.assertFalse(owner.done())
                            self.assertEqual(closed, [])
                        finally:
                            release.set()
                            with self.assertRaises(asyncio.CancelledError):
                                await owner
                self.assertEqual(len(closed), 1)
                profile.close()


if __name__ == "__main__":
    unittest.main()
