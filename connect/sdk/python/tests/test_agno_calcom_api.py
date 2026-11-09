"""Current Cal.com read contracts through real Agno registration and offline MCP."""
from __future__ import annotations

from contextlib import ExitStack
import asyncio
import hashlib
import importlib.metadata
import inspect
import io
import json
import os
from pathlib import Path
import threading
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from mcp import Client
import requests

from meerkat_toolkit.adapters.agno_mcp import create_server
from meerkat_toolkit.adapters.agno_profiles import build_profile, configuration_help


REFERENCE = "TOOLKIT_CALCOM_TEST_KEY"
SECRET = "cal_live_selected_test_secret"
OPERATIONS = ["get_available_slots", "get_upcoming_bookings"]


def slot_page(*starts):
    return {"status": "success", "data": {"2026-03-29": [
        {"start": start, "end": "2026-03-29T03:00:00Z"} for start in starts
    ]}}


def booking_page(*, more=False):
    return {"status": "success", "data": [{"uid": "selected-booking", "title": "Selected booking",
        "start": "2026-03-29T01:30:00Z", "status": "accepted"}],
        "pagination": {"hasMore": more, "nextCursor": "opaque-cursor" if more else None}}


class CalComApiTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.stack.enter_context(patch.dict(os.environ, {REFERENCE: SECRET,
            "CALCOM_API_KEY": "ambient-secret", "HTTPS_PROXY": "http://ambient.invalid"}, clear=True))
        for target in ("socket.socket.connect", "socket.socket.connect_ex", "socket.getaddrinfo"):
            self.stack.enter_context(patch(target, side_effect=AssertionError("network forbidden")))
        for target in ("requests.get", "requests.sessions.get_netrc_auth"):
            self.stack.enter_context(patch(target, side_effect=AssertionError("unowned transport forbidden")))
        self.profile = build_profile("calcom", operations=OPERATIONS,
            config={"api_key_env": REFERENCE, "timeout": 3, "event_type_id": 42, "user_timezone": "Europe/Stockholm"})
        self.addCleanup(self.profile.close)
        self.calls = []
        self.bodies = []
        self.closed = []

    def respond(self, value, *, status=200, raw=None, headers=None):
        def send(adapter, request, **kwargs):
            self.calls.append((request, kwargs))
            result = requests.Response()
            result.status_code = status
            result.url = request.url
            result.request = request
            result.headers.update({"Content-Type": "application/json", **(headers or {})})
            result.raw = io.BytesIO(raw if raw is not None else json.dumps(value).encode())
            self.bodies.append(result.raw)
            original_close = result.close
            def close():
                self.closed.append(result)
                original_close()
            result.close = close
            return result
        self.stack.enter_context(patch.object(requests.adapters.HTTPAdapter, "send", send))

    async def call(self, name, arguments):
        async with Client(create_server(self.profile)) as client:
            result = await client.call_tool(name, arguments)
        self.assertNotIn(SECRET, result.model_dump_json())
        self.assertNotIn("ambient-secret", result.model_dump_json())
        return result

    async def test_current_slots_range_contract_and_dst(self):
        self.respond(slot_page("2026-03-29T00:30:00Z", "2026-03-29T01:30:00Z"))
        result = await self.call("get_available_slots", {"start_date": "2026-03-29", "end_date": "2026-03-29"})
        self.assertFalse(result.is_error)
        self.assertEqual(result.structured_content["value"], "Available slots: 2026-03-29 01:30 CET, 2026-03-29 03:30 CEST")
        request, options = self.calls[0]
        self.assertEqual(urlsplit(request.url).path, "/v2/slots")
        self.assertEqual(request.headers["cal-api-version"], "2024-09-04")
        self.assertEqual(parse_qs(urlsplit(request.url).query), {
            "start": ["2026-03-29T00:00:00Z"], "end": ["2026-03-29T23:59:59Z"],
            "eventTypeId": ["42"], "format": ["range"], "timeZone": ["UTC"]})
        self.assertEqual(request.headers["Authorization"], f"Bearer {SECRET}")
        self.assertEqual(options["timeout"], 3)
        self.assertEqual(len(self.closed), len(self.calls))

    async def test_current_bookings_version_and_incomplete_first_page(self):
        self.respond(booking_page(more=True))
        result = await self.call("get_upcoming_bookings", {"email": "attendee@example.test"})
        self.assertFalse(result.is_error)
        text = result.structured_content["value"]
        self.assertIn("selected-booking", text)
        self.assertIn("More upcoming bookings are available; only the first page is shown.", text)
        self.assertNotIn("opaque-cursor", text)
        self.assertEqual(len(self.calls), 1)
        request, _ = self.calls[0]
        self.assertEqual(request.headers["cal-api-version"], "2026-05-01")
        self.assertEqual(parse_qs(urlsplit(request.url).query), {
            "status": ["upcoming"], "limit": ["100"], "attendeeEmail": ["attendee@example.test"]})

    async def test_actual_pinned_donor_and_only_two_read_tools(self):
        from agno.tools.calcom import CalComTools
        self.assertEqual(importlib.metadata.version("agno"), "3.0.11")
        self.assertEqual(hashlib.sha256(Path(inspect.getfile(CalComTools)).read_bytes()).hexdigest(),
                         "6cc4e8bd4d061a6d0ff14fd89ff825acb444812e6085d2f4613aad3ee22635e5")
        owner = inspect.unwrap(next(iter(self.profile.adapter._bindings.values())).entrypoint).__self__
        self.assertIsInstance(owner, CalComTools)
        async with Client(create_server(self.profile)) as client:
            listed = await client.list_tools()
            encoded = listed.model_dump_json()
            self.assertEqual({tool.name for tool in listed.tools}, set(OPERATIONS))
            self.assertTrue(all(tool.annotations.read_only_hint for tool in listed.tools))
            self.assertNotIn(SECRET, encoded)
            self.assertNotIn("ambient-secret", encoded)
        guidance = json.dumps(configuration_help("calcom"))
        self.assertNotIn(SECRET, guidance)
        self.assertIn("2026-05-01", guidance)
        self.assertIn("first page", json.dumps(self.profile.descriptions))

    async def test_empty_terminal_and_empty_nonterminal_pages_are_distinct(self):
        for more, expected in [(False, "No upcoming bookings found."),
                               (True, "No upcoming bookings in this page.\nMore upcoming bookings are available; only the first page is shown.")]:
            with self.subTest(more=more):
                page = booking_page(more=more); page["data"] = []
                self.respond(page)
                result = await self.call("get_upcoming_bookings", {})
                self.assertFalse(result.is_error)
                self.assertEqual(result.structured_content["value"], expected)
        self.respond({"status": "success", "data": {}})
        result = await self.call("get_available_slots", {"start_date": "2026-03-29", "end_date": "2026-03-29"})
        self.assertEqual(result.structured_content["value"], "Available slots: ")

    async def test_invalid_calendar_dates_and_span_fail_before_http(self):
        self.respond({"status": "success", "data": {}})
        for start, end in [("2026-02-29", "2026-03-01"), ("2026-04-31", "2026-05-01"),
                           ("2026-03-30", "2026-03-29"), ("2026-01-01", "2026-02-01"),
                           ("1969-12-31", "1970-01-01"), ("2038-01-01", "2038-01-01"),
                           ("2026-03-29\n", "2026-03-30"), ("20260329", "2026-03-30")]:
            with self.subTest(start=start, end=end):
                result = await self.call("get_available_slots", {"start_date": start, "end_date": end})
                self.assertTrue(result.is_error)
        self.assertEqual(self.calls, [])
        for start, end in [("2026-03-01", "2026-03-31"), ("2024-02-29", "2024-02-29"),
                           ("1970-01-01", "1970-01-01"), ("2037-12-31", "2037-12-31")]:
            result = await self.call("get_available_slots", {"start_date": start, "end_date": end})
            self.assertFalse(result.is_error)
            query = parse_qs(urlsplit(self.calls[-1][0].url).query)
            self.assertEqual(query["start"], [start + "T00:00:00Z"])
            self.assertEqual(query["end"], [end + "T23:59:59Z"])

    async def test_fall_dst_and_offset_timestamps_retain_exact_instants(self):
        self.respond({"status": "success", "data": {"2026-10-25": [
            {"start": "2026-10-25T02:30:00+02:00", "end": "2026-10-25T03:00:00+02:00"},
            {"start": "2026-10-25T02:30:00+01:00", "end": "2026-10-25T03:00:00+01:00"}]}})
        result = await self.call("get_available_slots", {"start_date": "2026-10-25", "end_date": "2026-10-25"})
        self.assertEqual(result.structured_content["value"], "Available slots: 2026-10-25 02:30 CEST, 2026-10-25 02:30 CET")
        page = booking_page(); page["data"][0]["start"] = "2026-03-28T23:30:00Z"
        self.respond(page)
        result = await self.call("get_upcoming_bookings", {})
        self.assertIn("2026-03-29 00:30 CET", result.structured_content["value"])

    async def test_slot_shape_date_and_range_mismatches_fail_closed(self):
        cases = [{"data": {}}, {"status": "error", "data": {}, "error": SECRET},
                 {"status": "success", "data": {"slots": {}}},
                 {"status": "success", "data": {"2026-03-29": ["2026-03-29T00:30:00Z"]}}]
        for field, value in [("start", "2026-03-29T01:30:00"), ("start", "2026-03-28T23:30:00Z"),
                             ("start", "2038-01-01T00:00:00Z"), ("start", None),
                             ("end", "2026-03-29T00:00:00Z"), ("end", "2026-03-29T01:30:00"),
                             ("end", "not-a-time")]:
            page = slot_page("2026-03-29T01:30:00Z"); page["data"]["2026-03-29"][0][field] = value
            cases.append(page)
        for page in cases:
            with self.subTest(page=page):
                self.respond(page)
                result = await self.call("get_available_slots", {"start_date": "2026-03-29", "end_date": "2026-03-29"})
                self.assertTrue(result.is_error)
        self.assertEqual(len(self.closed), len(self.calls))

    async def test_booking_timestamps_must_be_aware_and_supported_in_utc_and_local_time(self):
        for stamp in ["2026-03-29T01:30:00", "2038-01-01T00:00:00Z", "1969-12-31T23:00:00Z",
                      "2037-12-31T23:30:00Z", "2037-12-31T23:30:00-12:00", SECRET]:
            with self.subTest(timestamp=stamp):
                page = booking_page(); page["data"][0]["start"] = stamp
                self.respond(page)
                self.assertTrue((await self.call("get_upcoming_bookings", {})).is_error)

    async def test_pagination_and_bookings_require_current_well_formed_shape(self):
        bad_pages = [{"data": []}, {"status": "error", **{key: value for key, value in booking_page().items() if key != "status"}}]
        for pagination in [None, {}, {"hasMore": "false", "nextCursor": None},
                           {"hasMore": True, "nextCursor": None}, {"hasMore": True, "nextCursor": ""},
                           {"hasMore": False, "nextCursor": "unexpected"}, {"hasMore": False}]:
            page = booking_page(); page["pagination"] = pagination; bad_pages.append(page)
        for data in [None, {}, [None], [{"uid": "id"}], [booking_page()["data"][0]] * 101]:
            page = booking_page(); page["data"] = data; bad_pages.append(page)
        for page in bad_pages:
            with self.subTest(page=page):
                self.respond(page)
                self.assertTrue((await self.call("get_upcoming_bookings", {})).is_error)

    async def test_full_page_is_bounded_without_following_a_cursor_or_next_url(self):
        page = booking_page(more=True)
        page["data"] = [{**page["data"][0], "uid": f"booking-{n}"} for n in range(100)]
        page["pagination"]["nextCursor"] = "https://other-account.invalid/next?key=" + SECRET
        page["next"] = page["pagination"]["nextCursor"]
        self.respond(page)
        result = await self.call("get_upcoming_bookings", {})
        self.assertFalse(result.is_error)
        self.assertIn("booking-99", result.structured_content["value"])
        self.assertEqual(len(self.calls), 1)
        self.assertNotIn("other-account", result.model_dump_json())
        self.assertNotIn("eventTypeId", parse_qs(urlsplit(self.calls[0][0].url).query))

    async def test_http_errors_redirects_bad_json_and_limits_close_without_disclosing_details(self):
        for operation, arguments in [("get_upcoming_bookings", {}),
                                     ("get_available_slots", {"start_date": "2026-03-29", "end_date": "2026-03-29"})]:
            for options in [{"status": status} for status in (201, 302, 401, 403, 429, 500)] + [
                {"raw": b"not-json"}, {"raw": b"\xff"}, {"raw": b" " * 1_048_577},
                {"headers": {"Content-Length": "1048577"}}]:
                with self.subTest(operation=operation, options={k: v for k, v in options.items() if k != "raw"}):
                    count = len(self.calls)
                    self.respond({"error": SECRET}, **options)
                    result = await self.call(operation, arguments)
                    self.assertTrue(result.is_error)
                    self.assertEqual(len(self.calls), count + 1)
                    self.assertEqual(len(self.closed), len(self.calls))

    async def test_selected_accounts_remain_distinct_after_environment_changes(self):
        os.environ[REFERENCE] = "cal_second_selected_secret"
        other = build_profile("calcom", operations=OPERATIONS, config={"api_key_env": REFERENCE,
            "timeout": 3, "event_type_id": 99, "user_timezone": "America/New_York"})
        self.addCleanup(other.close)
        os.environ[REFERENCE] = "changed-environment-secret"
        self.respond(booking_page())
        first = await self.call("get_upcoming_bookings", {})
        self.profile.close()
        async with Client(create_server(other)) as client:
            second = await client.call_tool("get_upcoming_bookings", {})
        self.assertIn("03:30 CEST", first.structured_content["value"])
        self.assertIn("21:30 EDT", second.structured_content["value"])
        self.assertEqual([request.headers["Authorization"] for request, _ in self.calls],
                         [f"Bearer {SECRET}", "Bearer cal_second_selected_secret"])
        self.assertNotIn("cal_second_selected_secret", second.model_dump_json())
        self.assertNotIn("changed-environment-secret", second.model_dump_json())

    async def test_cancellation_waits_for_actual_response_cleanup(self):
        entered, release = threading.Event(), threading.Event()
        closed = []
        def send(adapter, request, **kwargs):
            response = requests.Response(); response.status_code = 200
            response.raw = io.BytesIO(json.dumps(booking_page(more=True)).encode())
            original = response.close
            def close():
                entered.set()
                if not release.wait(3):
                    raise AssertionError("cleanup fixture timed out")
                original(); closed.append(True)
            response.close = close
            return response
        with patch.object(requests.adapters.HTTPAdapter, "send", send):
            async with Client(create_server(self.profile)) as client:
                owner = asyncio.create_task(client.call_tool("get_upcoming_bookings", {}))
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
        self.assertEqual(closed, [True])


if __name__ == "__main__":
    unittest.main()
