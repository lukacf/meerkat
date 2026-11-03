"""Gmail 403 diagnostics through actual HTTPX and standard MCP, without real accounts."""
from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager
import json
import unittest

import httpx
from mcp import Client

from meerkat_toolkit.adapters import services_config, services_mcp
from meerkat_toolkit.connectors.gmail import GmailError, GmailOperations
from meerkat_toolkit.httpx_client import prepare_httpx_client
from test_home_assistant import Server, client, respond


PRIVATE = "private-provider-credential-message-canary"
MIME = b"From: sender@example.invalid\r\nTo: receiver@example.invalid\r\n\r\nSummary"
REASONS = {
    "rateLimitExceeded": ("usageLimits", "rate_limited"),
    "userRateLimitExceeded": ("usageLimits", "rate_limited"),
    "dailyLimitExceeded": ("usageLimits", "quota_exceeded"),
    "domainPolicy": ("global", "permission"),
}


def document(reason):
    return {"error": {"code": 403, "message": PRIVATE, "errors": [
        {"domain": REASONS[reason][0], "reason": reason, "message": PRIVATE}]}}


class GmailErrorReasonTests(unittest.IsolatedAsyncioTestCase):
    async def test_documented_403_reasons_over_loopback_preserve_single_attempt_and_outcome(self):
        for reason, (_, kind) in REASONS.items():
            for write in (False, True):
                with self.subTest(reason=reason, write=write):
                    async def handle(_request, writer):
                        await respond(writer, document(reason), status=403,
                                      headers={"Retry-After": "17"})
                    async with Server(handle) as server:
                        async with client(server, PRIVATE) as transport:
                            ops = GmailOperations(transport, user_id="me")
                            with self.assertRaises(GmailError) as caught:
                                await (ops.send_raw(MIME) if write else ops.list_messages_page())
                        error = caught.exception
                        self.assertEqual(error.kind.value, kind)
                        self.assertEqual(getattr(error, "provider_reason", None), reason)
                        self.assertEqual(error.status_code, 403)
                        self.assertEqual(error.retry_after_seconds, 17)
                        self.assertEqual(error.write_outcome.value, "unknown" if write else "not_applicable")
                        self.assertNotIn(PRIVATE, str(error))
                        self.assertEqual(len(server.requests), 1)
                    self.assertTrue(server.joined)

    async def test_standard_mcp_projects_only_safe_reason(self):
        requests = []
        mixed = document("rateLimitExceeded")
        mixed["error"]["errors"].append(document("domainPolicy")["error"]["errors"][0])
        cases = [(document(reason), kind, reason) for reason, (_, kind) in REASONS.items()]
        cases.append((mixed, "permission", None))
        responses = iter(case[0] for case in cases)
        async def handle(request):
            requests.append(request)
            return httpx.Response(403, json=next(responses))
        async with httpx.AsyncClient(base_url="https://gmail.googleapis.com/",
                transport=httpx.MockTransport(handle), trust_env=False) as transport:
            prepare_httpx_client(transport)
            config = services_config.validate_configuration({"profile": "gmail",
                "operations": ["send_message"], "config": {"token_env": "UNUSED_TEST_REFERENCE",
                "timeout_seconds": 2, "max_response_bytes": 4096, "max_message_bytes": 4096,
                "user_id": "me"}})
            profile = services_mcp.create_profile_from_client(config, transport)
            try:
                async with Client(services_mcp.create_server(profile)) as mcp:
                    for _, kind, reason in cases:
                        result = await mcp.call_tool("gmail_send_message", {
                            "from_address": "sender@example.invalid", "to": ["receiver@example.invalid"],
                            "subject": "Summary", "body_text": "Selected content"})
                        self.assertTrue(result.is_error)
                        error = result.structured_content["error"]
                        self.assertEqual(error["kind"], kind)
                        self.assertEqual(error.get("provider_reason"), reason)
                        self.assertEqual("provider_reason" in error, reason is not None)
                        self.assertEqual(error["write_outcome"], "unknown")
                        self.assertNotIn(PRIVATE, result.model_dump_json())
                self.assertEqual(len(requests), len(cases))
            finally:
                await profile.aclose()


class DiagnosticStream(httpx.AsyncByteStream):
    def __init__(self, chunks, *, block_read=False, block_close=False):
        self.chunks = chunks
        self.reads = 0
        self.closed = 0
        self.reading, self.closing = asyncio.Event(), asyncio.Event()
        self.read_release, self.close_release = asyncio.Event(), asyncio.Event()
        if not block_read:
            self.read_release.set()
        if not block_close:
            self.close_release.set()

    async def __aiter__(self):
        self.reading.set()
        for chunk in self.chunks:
            self.reads += 1
            if isinstance(chunk, Exception):
                raise chunk
            yield chunk
        await self.read_release.wait()

    async def aclose(self):
        self.closing.set()
        await self.close_release.wait()
        self.closed += 1


@asynccontextmanager
async def diagnostic_client(raw, *, status=403, headers=None, maximum=2_097_152,
                            timeout=2, stream=None, consumed=False, user="me"):
    requests = []
    if stream is None:
        stream = DiagnosticStream([raw])
    response_headers = {"Content-Type": "application/json", "Retry-After": "11"} if headers is None else headers

    async def handle(request):
        requests.append(request)
        return httpx.Response(status, headers=response_headers,
                              **({"content": raw} if consumed else {"stream": stream}))

    async with httpx.AsyncClient(base_url="https://gmail.googleapis.com/",
            transport=httpx.MockTransport(handle), headers={"Authorization": "Bearer " + PRIVATE},
            trust_env=False) as transport:
        prepare_httpx_client(transport)
        yield GmailOperations(transport, user_id=user, max_response_bytes=maximum,
                              timeout_seconds=timeout), stream, requests


class GmailDiagnosticAdversarialTests(unittest.IsolatedAsyncioTestCase):
    def assert_generic(self, error):
        self.assertEqual(error.kind.value, "permission")
        self.assertEqual(error.status_code, 403)
        self.assertIsNone(error.provider_reason)
        self.assertNotIn(PRIVATE, str(error))
        self.assertIsNone(error.__context__)
        self.assertIsNone(error.__cause__)

    async def failure(self, ops, *, write=False):
        with self.assertRaises(GmailError) as caught:
            await (ops.send_raw(MIME) if write else ops.list_messages_page())
        return caught.exception

    async def test_consistent_repeated_reasons_and_consumed_responses(self):
        value = document("userRateLimitExceeded")
        value["error"]["errors"] *= 32
        value["future"] = {"private": PRIVATE}
        raw = json.dumps(value).encode()
        for consumed in (False, True):
            async with diagnostic_client(raw, consumed=consumed) as (ops, stream, requests):
                error = await self.failure(ops)
                self.assertEqual((error.kind.value, error.provider_reason),
                                 ("rate_limited", "userRateLimitExceeded"))
                self.assertEqual(len(requests), 1)
                if not consumed:
                    self.assertEqual(stream.closed, 1)

    async def test_mixed_unknown_and_malformed_diagnostics_remain_generic(self):
        entry = {"domain": "usageLimits", "reason": "rateLimitExceeded"}
        errors = [None, [], "bad", [None], [PRIVATE], [{}],
                  [entry, {"domain": "usageLimits", "reason": "userRateLimitExceeded"}],
                  [entry, {"domain": "global", "reason": "domainPolicy"}],
                  [entry, {"domain": "usageLimits", "reason": PRIVATE}],
                  [{"domain": "global", "reason": "rateLimitExceeded"}],
                  [{"domain": "usageLimits", "reason": ["rateLimitExceeded"]}],
                  [entry] * 33]
        values = [None, [], PRIVATE, {}, {"error": []}]
        values += [{"error": {"code": 403, "errors": value}} for value in errors]
        values += [{"error": {"code": value, "errors": [entry]}}
                   for value in (None, True, 403.0, "403", 400, 429)]
        values += [{"error": {"errors": [entry]}}, {"error": {"code": 403}}]
        raws = [json.dumps(value).encode() for value in values]
        raw = json.dumps(document("rateLimitExceeded")).encode()
        raws += [b"", b"\xff", raw[:-1], raw.replace(b'"code": 403', b'"code": 403, "code": 403'),
                 raw.replace(b'"reason": "rateLimitExceeded"', b'"reason": "rateLimitExceeded", "reason": "domainPolicy"'),
                 raw.replace(b'"code": 403', b'"code": NaN'), b'[' * 1100 + b']' * 1100]
        for index, payload in enumerate(raws):
            with self.subTest(index=index):
                async with diagnostic_client(payload) as (ops, stream, requests):
                    self.assert_generic(await self.failure(ops))
                    self.assertEqual(len(requests), 1)
                    self.assertEqual(stream.closed, 1)

    async def test_only_403_is_read_for_diagnostics(self):
        raw = json.dumps(document("dailyLimitExceeded")).encode()
        for status, kind in ((400, "http_status"), (401, "authentication"), (404, "not_found"),
                             (429, "rate_limited"), (503, "http_status"), (302, "redirect")):
            with self.subTest(status=status):
                async with diagnostic_client(raw, status=status) as (ops, stream, requests):
                    error = await self.failure(ops, write=True)
                    self.assertEqual(error.kind.value, kind)
                    self.assertIsNone(error.provider_reason)
                    self.assertEqual(error.write_outcome.value, "unknown")
                    self.assertEqual(stream.reads, 0)
                    self.assertEqual(stream.closed, 1)
                    self.assertEqual(len(requests), 1)

    async def test_diagnostic_byte_cap_and_configured_lower_limit(self):
        raw = json.dumps(document("rateLimitExceeded")).encode()
        for maximum in (len(raw) + 3, 65_536, 131_072):
            cap = min(maximum, 65_536)
            for extra in (0, 1):
                padded = raw + b" " * (cap - len(raw) + extra)
                stream = DiagnosticStream([padded[:cap], padded[cap:]])
                async with diagnostic_client(padded, stream=stream, maximum=maximum) as (ops, observed, requests):
                    error = await self.failure(ops)
                    if extra:
                        self.assert_generic(error)
                    else:
                        self.assertEqual(error.provider_reason, "rateLimitExceeded")
                    self.assertEqual(len(requests), 1)
                    self.assertEqual(observed.closed, 1)
        async with diagnostic_client(raw, maximum=len(raw)-1) as (ops, stream, requests):
            self.assert_generic(await self.failure(ops))
            self.assertEqual(stream.closed, 1)

    async def test_invalid_metadata_and_length_use_generic_without_unbounded_read(self):
        raw = json.dumps(document("rateLimitExceeded")).encode()
        cases = [({}, False), ({"Content-Type": "text/html"}, False),
                 ({"Content-Type": "application/json", "Content-Encoding": "gzip"}, False),
                 ({"Content-Type": "application/json", "Content-Length": "65537"}, False),
                 ({"Content-Type": "application/json", "Content-Length": "-1"}, False),
                 ({"Content-Type": "application/json", "Content-Length": "x"}, False),
                 ({"Content-Type": "application/json", "Content-Length": str(len(raw)+1)}, True),
                 ({"Content-Type": "application/json", "Content-Length": str(len(raw)-1)}, True)]
        for headers, read in cases:
            with self.subTest(headers=headers):
                async with diagnostic_client(raw, headers=headers) as (ops, stream, requests):
                    self.assert_generic(await self.failure(ops))
                    self.assertEqual(stream.reads, 1 if read else 0)
                    self.assertEqual(stream.closed, 1)

    async def test_interrupted_body_keeps_http_refusal_and_drops_private_exception(self):
        for exception in (httpx.ReadError(PRIVATE), httpx.ReadTimeout(PRIVATE), RuntimeError(PRIVATE)):
            stream = DiagnosticStream([b'{"error":', exception])
            async with diagnostic_client(b"", stream=stream) as (ops, observed, requests):
                error = await self.failure(ops, write=True)
                self.assert_generic(error)
                self.assertEqual(error.write_outcome.value, "unknown")
                self.assertEqual(observed.closed, 1)
                self.assertEqual(len(requests), 1)

    async def test_cancellation_during_diagnostic_body_joins_close_even_if_cancelled_again(self):
        from meerkat_toolkit.connectors.gmail import SendCancelled
        for write in (False, True):
            stream = DiagnosticStream([b'{"error":'], block_read=True, block_close=True)
            async with diagnostic_client(b"", stream=stream) as (ops, observed, requests):
                task = asyncio.create_task(ops.send_raw(MIME) if write else ops.list_messages_page())
                try:
                    await asyncio.wait_for(observed.reading.wait(), 2)
                    task.cancel()
                    await asyncio.wait_for(observed.closing.wait(), 2)
                    task.cancel()
                    await asyncio.sleep(0)
                    self.assertFalse(task.done())
                    observed.close_release.set()
                    with self.assertRaises(asyncio.CancelledError) as caught:
                        await task
                    self.assertEqual(isinstance(caught.exception, SendCancelled), write)
                    if write:
                        self.assertEqual(caught.exception.write_outcome.value, "unknown")
                    self.assertEqual(observed.closed, 1)
                    self.assertEqual(len(requests), 1)
                finally:
                    observed.read_release.set()
                    observed.close_release.set()
                    if not task.done():
                        task.cancel()
                    await asyncio.gather(task, return_exceptions=True)

    async def test_total_deadline_during_diagnostics_joins_delayed_close(self):
        for write in (False, True):
            stream = DiagnosticStream([b'{"error":'], block_read=True, block_close=True)
            async with diagnostic_client(b"", stream=stream, timeout=.05) as (ops, observed, requests):
                task = asyncio.create_task(ops.send_raw(MIME) if write else ops.list_messages_page())
                try:
                    await asyncio.wait_for(observed.closing.wait(), 2)
                    self.assertFalse(task.done())
                    observed.close_release.set()
                    with self.assertRaises(GmailError) as caught:
                        await task
                    self.assertEqual(caught.exception.kind.value, "timeout")
                    self.assertIsNone(caught.exception.provider_reason)
                    self.assertEqual(caught.exception.write_outcome.value, "unknown" if write else "not_applicable")
                    self.assertEqual(observed.closed, 1)
                    self.assertEqual(len(requests), 1)
                finally:
                    observed.read_release.set()
                    observed.close_release.set()
                    if not task.done():
                        task.cancel()
                    await asyncio.gather(task, return_exceptions=True)

    async def test_same_endpoint_accounts_keep_separate_requests_and_diagnostics(self):
        for user, reason in (("one@example.invalid", "domainPolicy"),
                             ("two@example.invalid", "dailyLimitExceeded")):
            raw = json.dumps(document(reason)).encode()
            async with diagnostic_client(raw, user=user) as (ops, stream, requests):
                error = await self.failure(ops)
                self.assertEqual(error.provider_reason, reason)
                self.assertEqual(len(requests), 1)
                self.assertEqual(requests[0].url.path,
                                 "/gmail/v1/users/" + user + "/messages")
                self.assertEqual(requests[0].headers["Authorization"], "Bearer " + PRIVATE)
                self.assertEqual(stream.closed, 1)

    async def test_all_write_operations_retain_unknown_after_quota_refusal(self):
        raw = json.dumps(document("dailyLimitExceeded")).encode()
        for operation in ("send", "draft", "modify"):
            async with diagnostic_client(raw) as (ops, stream, requests):
                with self.assertRaises(GmailError) as caught:
                    if operation == "send":
                        await ops.send_raw(MIME)
                    elif operation == "draft":
                        await ops.create_draft_raw(MIME)
                    else:
                        await ops.modify_message("message1", add_label_ids=["STARRED"])
                self.assertEqual(caught.exception.kind.value, "quota_exceeded")
                self.assertEqual(caught.exception.provider_reason, "dailyLimitExceeded")
                self.assertEqual(caught.exception.write_outcome.value, "unknown")
                self.assertEqual(stream.closed, 1)
                self.assertEqual(len(requests), 1)
