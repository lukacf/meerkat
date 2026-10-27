"""Calendar/Drive quota refusals through real HTTPX custody and standard MCP."""
from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager
import importlib
import json
import os
import unittest

import httpx
from mcp import Client

NAMESPACE = os.environ.get("TOOLKIT_SERVICES_TEST_NAMESPACE", "meerkat_toolkit")
calendar = importlib.import_module(NAMESPACE + ".connectors.google_calendar")
drive = importlib.import_module(NAMESPACE + ".connectors.google_drive")
config = importlib.import_module(NAMESPACE + ".adapters.services_config")
adapter = importlib.import_module(NAMESPACE + ".adapters.services_mcp")
prepare = importlib.import_module(NAMESPACE + ".httpx_client").prepare_httpx_client
PRIVATE = "private-quota-token-and-provider-prose"
SERVICES = ("google-calendar", "google-drive")


def document(reason="rateLimitExceeded", domain="usageLimits"):
    return {"error": {"code": 403, "message": PRIVATE, "errors": [
        {"domain": domain, "reason": reason, "message": PRIVATE}]}}


class Stream(httpx.AsyncByteStream):
    def __init__(self, chunks, *, block=False, hold_close=False):
        self.chunks, self.reads, self.closed = chunks, 0, 0
        self.reading, self.closing = asyncio.Event(), asyncio.Event()
        self.release, self.close_release = asyncio.Event(), asyncio.Event()
        if not block:
            self.release.set()
        if not hold_close:
            self.close_release.set()

    async def __aiter__(self):
        self.reading.set()
        for chunk in self.chunks:
            self.reads += 1
            if isinstance(chunk, Exception):
                raise chunk
            yield chunk
        await self.release.wait()

    async def aclose(self):
        self.closing.set()
        await self.close_release.wait()
        self.closed += 1


@asynccontextmanager
async def fixture(service, raw, *, status=403, headers=None, maximum=2_097_152,
                  timeout=2, stream=None, consumed=False):
    stream = stream if stream is not None else Stream([raw])
    requests = []

    async def handle(request):
        requests.append(request)
        return httpx.Response(status, headers=headers if headers is not None else {
            "Content-Type": "application/json", "Retry-After": "17"},
            **({"content": raw} if consumed else {"stream": stream}))

    async with httpx.AsyncClient(base_url="https://www.googleapis.com/",
            headers={"Authorization": "Bearer " + PRIVATE}, trust_env=False,
            transport=httpx.MockTransport(handle)) as client:
        prepare(client)
        options = {"timeout_seconds": timeout, "max_response_bytes": maximum}
        ops = (calendar.GoogleCalendarOperations(client, calendar_id="selected", **options)
               if service == SERVICES[0] else drive.GoogleDriveOperations(client, **options))
        try:
            yield ops, client, stream, requests
        finally:
            stream.release.set()
            stream.close_release.set()


def operation(service, ops, write):
    if service == SERVICES[0]:
        return ops.delete_event("selected") if write else ops.get_event("selected")
    return ops.delete_file("selected") if write else ops.get_file("selected")


class GoogleQuotaTests(unittest.IsolatedAsyncioTestCase):
    async def failure(self, service, ops, *, write=False):
        with self.assertRaises((calendar.CalendarError, drive.DriveError)) as caught:
            await operation(service, ops, write)
        error = caught.exception
        self.assertNotIn(PRIVATE, str(error))
        self.assertIsNone(error.__context__)
        self.assertIsNone(error.__cause__)
        self.assertEqual(error.write_outcome.value, "unknown" if write else "not_applicable")
        return error

    async def test_documented_quota_is_rate_limited_for_reads_and_writes(self):
        for service in SERVICES:
            for reason in ("rateLimitExceeded", "userRateLimitExceeded"):
                for write in (False, True):
                    with self.subTest(service=service, reason=reason, write=write):
                        async with fixture(service, json.dumps(document(reason)).encode()) as (ops, _, stream, requests):
                            error = await self.failure(service, ops, write=write)
                            self.assertEqual(error.kind.value, "rate_limited")
                            self.assertEqual((error.status_code, error.retry_after_seconds), (403, 17))
                            self.assertEqual(len(requests), 1)
                            self.assertEqual(stream.closed, 1)

    async def test_standard_mcp_projects_rate_limit_without_diagnostic_prose(self):
        for service in SERVICES:
            for reason in ("rateLimitExceeded", "userRateLimitExceeded"):
                with self.subTest(service=service, reason=reason):
                    async with fixture(service, json.dumps(document(reason)).encode()) as (_, client, stream, requests):
                        name = "delete_event" if service == SERVICES[0] else "get_file"
                        settings = {"token_env": "UNUSED_SYNTHETIC_TOKEN", "timeout_seconds": 2,
                                    "max_response_bytes": 4096}
                        if service == SERVICES[0]:
                            settings["calendar_id"] = "selected"
                        else:
                            settings["max_content_bytes"] = 4096
                        profile = adapter.create_profile_from_client(config.validate_configuration({
                            "profile": service, "operations": [name], "config": settings}), client)
                        try:
                            async with Client(adapter.create_server(profile)) as mcp:
                                result = await mcp.call_tool(service + "_" + name, {
                                    "event_id" if service == SERVICES[0] else "file_id": "selected"})
                            self.assertTrue(result.is_error)
                            error = result.structured_content["error"]
                            self.assertEqual(error, {"kind": "rate_limited", "status_code": 403,
                                "retry_after_seconds": 17, "operation_invoked": True,
                                "effect": "write" if service == SERVICES[0] else "read",
                                "write_outcome": "unknown" if service == SERVICES[0] else "not_applicable"})
                            self.assertNotIn(PRIVATE, result.model_dump_json())
                            self.assertEqual(len(requests), 1)
                            self.assertEqual(stream.closed, 1)
                        finally:
                            await profile.aclose()

    async def test_unknown_conflicting_and_malformed_evidence_stays_permission(self):
        entry = document()["error"]["errors"][0]
        values = [None, {}, {"error": {"code": "403", "errors": [entry]}},
                  {"error": {"code": 429, "errors": [entry]}},
                  document(domain="global"), document(reason="quotaExceeded"),
                  document(reason="insufficientPermissions")]
        for entries in ([], [None], [entry] * 33,
                        [entry, document("userRateLimitExceeded")["error"]["errors"][0]],
                        [entry, document("forbidden", "global")["error"]["errors"][0]]):
            values.append({"error": {"code": 403, "errors": entries}})
        raws = [json.dumps(value).encode() for value in values] + [b"not json",
            b'{"error":{"code":403,"code":403,"errors":[]}}',
            json.dumps(document()).replace('"message": "' + PRIVATE + '"', '"message": NaN').encode(),
            json.dumps(document()).encode() + b"\xff"]
        for service in SERVICES:
            for index, raw in enumerate(raws):
                with self.subTest(service=service, case=index):
                    async with fixture(service, raw) as (ops, _, stream, requests):
                        error = await self.failure(service, ops, write=True)
                        self.assertEqual((error.kind.value, error.status_code), ("permission", 403))
                        self.assertEqual(len(requests), 1)
                        self.assertEqual(stream.closed, 1)

    async def test_repeated_identical_evidence_and_consumed_responses(self):
        value = document()
        value["error"]["errors"] *= 32
        for service in SERVICES:
            for consumed in (False, True):
                with self.subTest(service=service, consumed=consumed):
                    async with fixture(service, json.dumps(value).encode(), consumed=consumed) as (ops, _, _, requests):
                        self.assertEqual((await self.failure(service, ops)).kind.value, "rate_limited")
                        self.assertEqual(len(requests), 1)

    async def test_metadata_and_byte_limits_refuse_unusable_diagnostics(self):
        raw = json.dumps(document()).encode()
        bad_headers = [{"Content-Type": "text/plain"},
            {"Content-Type": "application/json", "Content-Encoding": "gzip"},
            *[{"Content-Type": "application/json", "Content-Length": size}
              for size in ("-1", "2", "65537", "1, 1")]]
        for service in SERVICES:
            for headers in bad_headers:
                async with fixture(service, raw, headers=headers) as (ops, _, stream, requests):
                    self.assertEqual((await self.failure(service, ops)).kind.value, "permission")
                    self.assertEqual((len(requests), stream.closed), (1, 1))
            for maximum in (64, 100_000):
                stream = Stream([b"x" * 1024] * 100)
                async with fixture(service, b"", maximum=maximum, stream=stream) as (ops, _, _, requests):
                    self.assertEqual((await self.failure(service, ops)).kind.value, "permission")
                    self.assertLessEqual(stream.reads, 1 if maximum == 64 else 65)
                    self.assertEqual((len(requests), stream.closed), (1, 1))

    async def test_io_failure_and_plain_permission_preserve_refusal_and_429(self):
        for service in SERVICES:
            stream = Stream([httpx.ReadError(PRIVATE)])
            async with fixture(service, b"", stream=stream) as (ops, _, _, requests):
                self.assertEqual((await self.failure(service, ops)).kind.value, "permission")
                self.assertEqual((len(requests), stream.closed), (1, 1))
            for status, kind in ((403, "permission"), (429, "rate_limited")):
                for retry, expected in (("17", 17), ("private-bad-header", None)):
                    async with fixture(service, b"{}", status=status, headers={
                            "Content-Type": "application/json", "Retry-After": retry}) as (ops, _, _, requests):
                        error = await self.failure(service, ops, write=True)
                        self.assertEqual((error.kind.value, error.status_code, error.retry_after_seconds),
                                         (kind, status, expected))
                        self.assertEqual(len(requests), 1)

    async def test_diagnostic_cancel_joins_response_close_and_preserves_unknown_write(self):
        for service in SERVICES:
            stream = Stream([], block=True, hold_close=True)
            async with fixture(service, b"", stream=stream) as (ops, _, _, requests):
                task = asyncio.create_task(operation(service, ops, True))
                try:
                    await asyncio.wait_for(stream.reading.wait(), 2)
                    task.cancel()
                    await asyncio.wait_for(stream.closing.wait(), 2)
                    self.assertFalse(task.done())
                    stream.close_release.set()
                    with self.assertRaises(asyncio.CancelledError) as caught:
                        await asyncio.wait_for(task, 2)
                    self.assertEqual(caught.exception.write_outcome.value, "unknown")
                    self.assertEqual((len(requests), stream.closed), (1, 1))
                finally:
                    stream.release.set()
                    stream.close_release.set()
                    if not task.done():
                        task.cancel()
                    await asyncio.gather(task, return_exceptions=True)

    async def test_diagnostic_uses_existing_total_deadline(self):
        for service in SERVICES:
            stream = Stream([], block=True)
            async with fixture(service, b"", stream=stream, timeout=0.02) as (ops, _, _, requests):
                error = await asyncio.wait_for(self.failure(service, ops, write=True), 2)
                self.assertEqual((error.kind.value, error.status_code), ("timeout", None))
                self.assertEqual((len(requests), stream.closed), (1, 1))
