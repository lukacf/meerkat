"""Actual loopback HTTPX wire tests; no Google account or external effects.

Reuse the independently verified HA fixture's synchronous socket admission and
retained handler joins. Importing that fixture does not run its test cases.
"""

from __future__ import annotations

import asyncio
import base64
import json
import traceback
from typing import Any
from urllib.parse import parse_qs, urlsplit
import unittest

import httpx
from meerkat_toolkit.httpx_client import prepare_httpx_client

from meerkat_toolkit.connectors.gmail import (
    FailureKind,
    GmailError,
    GmailOperations,
    SendCancelled,
    WriteOutcome,
)
from test_home_assistant import Request, Server, client, respond


MIME = (
    b"From: sender@example.com\r\nTo: recipient@example.com\r\n"
    b"Cc: copy@example.com\r\nBcc: hidden@example.com\r\n"
    b"Message-ID: <unchanged@example.com>\r\nSubject: Summary\r\n"
    b"MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n"
    b"Exact selected summary.\r\n"
)
RECEIPT = {"id": "sent_01", "threadId": "thread_01", "labelIds": ["SENT"], "historyId": "123"}


class GmailTests(unittest.IsolatedAsyncioTestCase):
    async def test_pages_preserve_absence_order_tokens_and_estimates(self) -> None:
        pages = [
            {
                "messages": [{"id": "b", "threadId": "t"}, {"id": "a", "threadId": "t"}],
                "nextPageToken": "opaque+/= token",
                "resultSizeEstimate": 999,
                "future": {"keep": [True, None]},
            },
            {"messages": [{"id": "b", "threadId": "t"}], "resultSizeEstimate": 1},
            {},
        ]
        replies = iter(pages)

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, next(replies))

        async with Server(handler) as server:
            async with client(
                server, params={"q": "unrequested", "pageToken": "wrong"}
            ) as transport:
                operations = GmailOperations(transport, user_id="me")
                first = await operations.list_messages_page(
                    query="from:sender@example.com after:123",
                    label_ids=["INBOX", "Label_2"],
                    max_results=2,
                )
                self.assertEqual(first, pages[0])
                self.assertEqual(len(server.requests), 1, "listing one page must not auto-follow")
                second = await operations.list_messages_page(page_token=first["nextPageToken"])
                self.assertEqual(
                    second, pages[1], "duplicates are provider data, not silently removed"
                )
                self.assertEqual(await operations.list_messages_page(), {})
                queries = [parse_qs(urlsplit(request.target).query) for request in server.requests]
                self.assertEqual(
                    queries[0],
                    {
                        "q": ["from:sender@example.com after:123"],
                        "labelIds": ["INBOX", "Label_2"],
                        "maxResults": ["2"],
                        "includeSpamTrash": ["false"],
                    },
                )
                self.assertEqual(queries[1]["pageToken"], ["opaque+/= token"])
                self.assertNotIn("q", queries[1])
                self.assertNotIn("pageToken", queries[2])
        self.assertTrue(server.joined)

    async def test_later_page_error_does_not_become_complete_or_repeat(self) -> None:
        async def handler(request: Request, writer: asyncio.StreamWriter) -> None:
            if "pageToken" in request.target:
                await respond(
                    writer,
                    {"error": "private provider detail"},
                    status=429,
                    headers={"Retry-After": "9"},
                )
            else:
                await respond(
                    writer,
                    {"nextPageToken": "continuation", "messages": [{"id": "a", "threadId": "t"}]},
                )

        async with Server(handler) as server:
            async with client(server) as transport:
                operations = GmailOperations(transport, user_id="me")
                page = await operations.list_messages_page()
                with self.assertRaises(GmailError) as caught:
                    await operations.list_messages_page(page_token=page["nextPageToken"])
                self.assertEqual(caught.exception.kind, FailureKind.RATE_LIMITED)
                self.assertEqual(caught.exception.retry_after_seconds, 9)
                self.assertEqual(caught.exception.write_outcome, WriteOutcome.NOT_APPLICABLE)
            self.assertEqual(len(server.requests), 2)
        self.assertTrue(server.joined)

    async def test_repeated_cursor_is_exposed_without_automatic_request(self) -> None:
        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, {"messages": [], "nextPageToken": "same"})

        async with Server(handler) as server:
            async with client(server) as transport:
                result = await GmailOperations(transport, user_id="me").list_messages_page(
                    page_token="same"
                )
                self.assertEqual(result, {"messages": [], "nextPageToken": "same"})
            self.assertEqual(len(server.requests), 1)
        self.assertTrue(server.joined)

    async def test_nested_message_formats_and_exact_identity_preserved(self) -> None:
        full = {
            "id": "a",
            "threadId": "t",
            "internalDate": "1790000000000",
            "snippet": "private",
            "payload": {
                "mimeType": "multipart/mixed",
                "headers": [
                    {"name": "Subject", "value": "First"},
                    {"name": "Subject", "value": "Second"},
                ],
                "parts": [
                    {"mimeType": "text/plain", "body": {"data": "aMOpbGxv", "size": 6}},
                    {"body": {"attachmentId": "external", "size": 12}},
                ],
            },
            "future": {"nested": [1, True, None]},
        }
        raw = {"id": "a", "threadId": "t", "raw": base64.urlsafe_b64encode(MIME).decode()}
        replies = iter([full, raw, {"id": "a", "threadId": "t"}, full])

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, next(replies))

        async with Server(handler) as server:
            async with client(server) as transport:
                operations = GmailOperations(transport, user_id="me")
                self.assertEqual(await operations.get_message("a"), full)
                self.assertEqual(await operations.get_message("a", format="raw"), raw)
                self.assertEqual(
                    await operations.get_message("a", format="minimal"),
                    {"id": "a", "threadId": "t"},
                )
                self.assertEqual(
                    await operations.get_message(
                        "a", format="metadata", metadata_headers=["Subject", "From"]
                    ),
                    full,
                )
                self.assertEqual(
                    parse_qs(urlsplit(server.requests[-1].target).query),
                    {"format": ["metadata"], "metadataHeaders": ["Subject", "From"]},
                )
        self.assertTrue(server.joined)

    async def test_two_accounts_same_endpoint_and_frozen_base(self) -> None:
        async def handler(request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(
                writer,
                {"id": "same", "threadId": "t", "accountReceipt": request.headers["authorization"]},
            )

        async with Server(handler) as server:
            async with client(server, "A") as a, client(server, "B") as b:
                selected_a = GmailOperations(a, user_id="first+selected@example.com")
                selected_b = GmailOperations(b, user_id="second@example.com")
                a.base_url = "http://unused.invalid/"
                result_a, result_b = await asyncio.gather(
                    selected_a.get_message("same"), selected_b.get_message("same")
                )
                self.assertEqual(result_a["accountReceipt"], "Bearer A")
                self.assertEqual(result_b["accountReceipt"], "Bearer B")
                self.assertEqual(
                    {request.target.split("?")[0] for request in server.requests},
                    {
                        "/gmail/v1/users/first%2Bselected%40example.com/messages/same",
                        "/gmail/v1/users/second%40example.com/messages/same",
                    },
                )
                self.assertFalse(a.is_closed or b.is_closed)
        self.assertTrue(server.joined)

    async def test_send_preserves_mime_bytes_and_provider_receipt_one_attempt(self) -> None:
        async def handler(request: Request, writer: asyncio.StreamWriter) -> None:
            self.assertEqual(
                json.loads(request.body), {"raw": base64.urlsafe_b64encode(MIME).decode()}
            )
            await respond(writer, RECEIPT)

        async with Server(handler) as server:
            async with prepare_httpx_client(httpx.AsyncClient(
                base_url=server.url + "proxy/",
                headers={"Authorization": "Bearer selected"},
                params={"uploadType": "other"},
                trust_env=False,
            )) as transport:
                result = await GmailOperations(transport, user_id="me").send_raw(MIME)
                self.assertEqual(result, RECEIPT)
                self.assertFalse(transport.is_closed)
            self.assertEqual(len(server.requests), 1)
            request = server.requests[0]
            self.assertEqual(
                (request.method, request.target), ("POST", "/proxy/gmail/v1/users/me/messages/send")
            )
            self.assertEqual(request.headers["content-type"], "application/json")
            self.assertEqual(request.headers["accept-encoding"], "identity")
        self.assertTrue(server.joined)

    async def test_invalid_local_inputs_never_enter_transport(self) -> None:
        async def unexpected(_request: Request, _writer: asyncio.StreamWriter) -> None:
            self.fail("invalid input reached HTTP")

        async with Server(unexpected) as server:
            async with client(server) as transport:
                for user in ["", "../me", "a@example.com/../../me", "me?x=1", "me#other"]:
                    with self.subTest(user=user), self.assertRaises(ValueError):
                        GmailOperations(transport, user_id=user)
                operations = GmailOperations(transport, user_id="me", max_message_bytes=4)
                for mime in [b"", b"12345", bytearray(b"a"), "a"]:
                    with self.subTest(mime=type(mime)), self.assertRaises(ValueError):
                        await operations.send_raw(mime)
                for options in [
                    {"query": "\n"},
                    {"page_token": ""},
                    {"max_results": True},
                    {"max_results": 501},
                    {"label_ids": "INBOX"},
                    {"label_ids": ["../other"]},
                    {"include_spam_trash": "true"},
                ]:
                    with self.subTest(options=options), self.assertRaises(ValueError):
                        await operations.list_messages_page(**options)
                for message, options in [
                    ("../send", {}),
                    ("id?x=1", {}),
                    ("id", {"format": "other"}),
                    ("id", {"metadata_headers": ["Subject"]}),
                    ("id", {"format": "metadata", "metadata_headers": ["X\r\nBad"]}),
                ]:
                    with self.subTest(message=message), self.assertRaises(ValueError):
                        await operations.get_message(message, **options)
            self.assertEqual(server.requests, [])
        self.assertTrue(server.joined)

    async def test_constructor_refuses_ambiguous_base_and_limits(self) -> None:
        for base in [
            "https://user:secret@example.com/",
            "https://example.com/?q=secret",
            "https://example.com/#fragment",
            "ftp://example.com/",
        ]:
            async with prepare_httpx_client(httpx.AsyncClient(base_url=base, trust_env=False)) as transport:
                with self.subTest(base=base), self.assertRaises(ValueError) as caught:
                    GmailOperations(transport, user_id="me")
                self.assertNotIn("secret", str(caught.exception))
        async with prepare_httpx_client(httpx.AsyncClient(
            base_url="https://gmail.googleapis.com/", trust_env=False
        )) as transport:
            for options in [
                {"timeout_seconds": 0},
                {"timeout_seconds": float("nan")},
                {"timeout_seconds": True},
                {"max_response_bytes": 0},
                {"max_message_bytes": False},
            ]:
                with self.subTest(options=options), self.assertRaises(ValueError):
                    GmailOperations(transport, user_id="me", **options)

    async def test_http_refusals_preserve_status_without_secrets_or_retry(self) -> None:
        expected = {
            400: FailureKind.HTTP_STATUS,
            401: FailureKind.AUTHENTICATION,
            403: FailureKind.PERMISSION,
            404: FailureKind.NOT_FOUND,
            429: FailureKind.RATE_LIMITED,
            503: FailureKind.HTTP_STATUS,
        }
        statuses = iter(expected)
        secret = "private-provider-and-credential-canary"

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(
                writer, {"error": secret}, status=next(statuses), headers={"Retry-After": "17"}
            )

        async with Server(handler) as server:
            async with client(server, secret) as transport:
                operations = GmailOperations(transport, user_id="me")
                for status, kind in expected.items():
                    with self.subTest(status=status), self.assertRaises(GmailError) as caught:
                        await operations.send_raw(MIME)
                    error = caught.exception
                    self.assertEqual(
                        (error.kind, error.status_code, error.write_outcome),
                        (kind, status, WriteOutcome.UNKNOWN),
                    )
                    self.assertEqual(error.retry_after_seconds, 17)
                    self.assertNotIn(secret, "".join(traceback.format_exception(error)))
                    self.assertIsNone(error.__context__)
                    self.assertIsNone(error.__cause__)
            self.assertEqual(len(server.requests), len(expected))
        self.assertTrue(server.joined)

    async def test_redirect_never_receives_credentials_or_a_second_send(self) -> None:
        async def unexpected(_request: Request, _writer: asyncio.StreamWriter) -> None:
            self.fail("redirect was followed")

        async with Server(unexpected) as destination:

            async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
                await respond(writer, {}, status=307, headers={"Location": destination.url})

            async with Server(handler) as source:
                async with client(source, follow_redirects=True) as transport:
                    with self.assertRaises(GmailError) as caught:
                        await GmailOperations(transport, user_id="me").send_raw(MIME)
                    self.assertEqual(caught.exception.kind, FailureKind.REDIRECT)
                    self.assertEqual(caught.exception.write_outcome, WriteOutcome.UNKNOWN)
                self.assertEqual(len(source.requests), 1)
                self.assertEqual(destination.requests, [])
        self.assertTrue(source.joined and destination.joined)

    async def test_malformed_send_receipt_remains_unknown(self) -> None:
        replies = iter(
            [
                b'{"id":"a","id":"b","threadId":"t"}',
                b'{"id":"a","threadId":"t","bad":NaN}',
                b'{"id":"a","threadId":"t","bad":1e999}',
                b'{"id":"a"}',
                b"[]",
                b'{"id":"","threadId":"t"}',
                b'{"id":"a","threadId":"t","payload":[]}',
                b"not JSON",
            ]
        )

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, None, raw=next(replies))

        async with Server(handler) as server:
            async with client(server) as transport:
                for _ in range(8):
                    with self.assertRaises(GmailError) as caught:
                        await GmailOperations(transport, user_id="me").send_raw(MIME)
                    self.assertEqual(caught.exception.kind, FailureKind.INVALID_RESPONSE)
                    self.assertEqual(caught.exception.write_outcome, WriteOutcome.UNKNOWN)
            self.assertEqual(len(server.requests), 8)
        self.assertTrue(server.joined)

    async def test_invalid_read_shapes_and_wrong_message_refuse(self) -> None:
        replies = iter(
            [
                {"messages": None},
                {"messages": [{"id": "a"}]},
                {"nextPageToken": ""},
                {"resultSizeEstimate": True},
                {"id": "different", "threadId": "t"},
                {"id": "a", "threadId": "t"},
            ]
        )

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, next(replies))

        async with Server(handler) as server:
            async with client(server) as transport:
                operations = GmailOperations(transport, user_id="me")
                for _ in range(4):
                    with self.assertRaises(GmailError) as caught:
                        await operations.list_messages_page()
                    self.assertEqual(caught.exception.write_outcome, WriteOutcome.NOT_APPLICABLE)
                for options in [{}, {"format": "raw"}]:
                    with self.assertRaises(GmailError) as caught:
                        await operations.get_message("a", **options)
                    self.assertEqual(caught.exception.kind, FailureKind.INVALID_RESPONSE)
        self.assertTrue(server.joined)

    async def test_content_type_encoding_and_known_or_streaming_size_bounds(self) -> None:
        count = 0

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            nonlocal count
            count += 1
            if count == 1:
                await respond(writer, RECEIPT, headers={"Content-Type": "text/html"})
            elif count == 2:
                await respond(writer, RECEIPT, headers={"Content-Encoding": "gzip"})
            elif count == 3:
                writer.write(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 999999\r\n\r\n"
                )
            else:
                writer.write(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n30\r\n"
                    + b"x" * 48
                    + b"\r\n0\r\n\r\n"
                )
            await writer.drain()

        async with Server(handler) as server:
            async with client(server) as transport:
                for kind in [
                    FailureKind.INVALID_RESPONSE,
                    FailureKind.UNSUPPORTED_ENCODING,
                    FailureKind.RESPONSE_TOO_LARGE,
                    FailureKind.RESPONSE_TOO_LARGE,
                ]:
                    with self.subTest(kind=kind), self.assertRaises(GmailError) as caught:
                        await GmailOperations(
                            transport, user_id="me", max_response_bytes=32
                        ).send_raw(MIME)
                    self.assertEqual(caught.exception.kind, kind)
                    self.assertEqual(caught.exception.write_outcome, WriteOutcome.UNKNOWN)
        self.assertTrue(server.joined)

    async def test_post_acceptance_disconnect_is_unknown_not_retried(self) -> None:
        received: list[bytes] = []

        async def handler(request: Request, _writer: asyncio.StreamWriter) -> None:
            received.append(base64.urlsafe_b64decode(json.loads(request.body)["raw"]))
            # The HTTP peer closes after observing the entire accepted body.

        async with Server(handler) as server:
            async with client(server) as transport:
                with self.assertRaises(GmailError) as caught:
                    await GmailOperations(transport, user_id="me").send_raw(MIME)
                self.assertEqual(received, [MIME])
                self.assertEqual(caught.exception.kind, FailureKind.TRANSPORT)
                self.assertEqual(caught.exception.write_outcome, WriteOutcome.UNKNOWN)
                self.assertNotIn(server.url, str(caught.exception))
            self.assertEqual(len(server.requests), 1)
        self.assertTrue(server.joined)

    async def test_timeout_retains_unknown_and_joins_fixture_after_late_completion(self) -> None:
        entered, release, completed = asyncio.Event(), asyncio.Event(), asyncio.Event()

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            entered.set()
            await release.wait()
            completed.set()
            try:
                await respond(writer, RECEIPT)
            except (BrokenPipeError, ConnectionResetError):
                pass

        async with Server(handler) as server:
            server.releases.append(release)
            async with client(server) as transport:
                with self.assertRaises(GmailError) as caught:
                    await GmailOperations(transport, user_id="me", timeout_seconds=0.5).send_raw(
                        MIME
                    )
                self.assertTrue(entered.is_set(), "must reach actual peer before timeout")
                self.assertFalse(completed.is_set())
                self.assertEqual(caught.exception.kind, FailureKind.TIMEOUT)
                self.assertEqual(caught.exception.write_outcome, WriteOutcome.UNKNOWN)
                release.set()
                await asyncio.wait_for(completed.wait(), 2)
            self.assertEqual(len(server.requests), 1)
        self.assertTrue(server.joined)

    async def test_stream_chunks_do_not_reset_total_deadline(self) -> None:
        entered = asyncio.Event()

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            writer.write(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n"
            )
            entered.set()
            try:
                for part in [b"{", *([b" "] * 8), b"}"]:
                    writer.write(b"1\r\n" + part + b"\r\n")
                    await writer.drain()
                    await asyncio.sleep(0.25)
                writer.write(b"0\r\n\r\n")
                await writer.drain()
            except (BrokenPipeError, ConnectionResetError):
                pass

        async with Server(handler) as server:
            async with client(server) as transport:
                with self.assertRaises(GmailError) as caught:
                    await GmailOperations(
                        transport, user_id="me", timeout_seconds=1.0
                    ).list_messages_page()
                self.assertTrue(entered.is_set())
                self.assertEqual(caught.exception.kind, FailureKind.TIMEOUT)
                self.assertEqual(caught.exception.write_outcome, WriteOutcome.NOT_APPLICABLE)
        self.assertTrue(server.joined)

    async def test_send_cancellation_is_cancelled_with_unknown_effect(self) -> None:
        entered, release, completed = asyncio.Event(), asyncio.Event(), asyncio.Event()

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            entered.set()
            await release.wait()
            completed.set()
            try:
                await respond(writer, RECEIPT)
            except (BrokenPipeError, ConnectionResetError):
                pass

        async with Server(handler) as server:
            server.releases.append(release)
            async with client(server) as transport:
                operation = asyncio.create_task(
                    GmailOperations(transport, user_id="me").send_raw(MIME)
                )
                try:
                    await asyncio.wait_for(entered.wait(), 2)
                    operation.cancel()
                    with self.assertRaises(SendCancelled) as caught:
                        await operation
                    self.assertEqual(caught.exception.write_outcome, WriteOutcome.UNKNOWN)
                    self.assertFalse(completed.is_set())
                finally:
                    release.set()
                    if not operation.done():
                        operation.cancel()
                    await asyncio.gather(operation, return_exceptions=True)
                await asyncio.wait_for(completed.wait(), 2)
                self.assertFalse(transport.is_closed)
            self.assertEqual(len(server.requests), 1)
        self.assertTrue(server.joined)

    async def test_read_cancellation_is_not_a_send_outcome(self) -> None:
        entered, release = asyncio.Event(), asyncio.Event()

        async def handler(_request: Request, _writer: asyncio.StreamWriter) -> None:
            entered.set()
            await release.wait()

        async with Server(handler) as server:
            server.releases.append(release)
            async with client(server) as transport:
                operation = asyncio.create_task(
                    GmailOperations(transport, user_id="me").get_message("a")
                )
                try:
                    await asyncio.wait_for(entered.wait(), 2)
                    operation.cancel()
                    with self.assertRaises(asyncio.CancelledError) as caught:
                        await operation
                    self.assertNotIsInstance(caught.exception, SendCancelled)
                finally:
                    release.set()
                    if not operation.done():
                        operation.cancel()
                    await asyncio.gather(operation, return_exceptions=True)
        self.assertTrue(server.joined)

    async def test_host_hook_error_is_safe_and_does_not_claim_nonoccurrence(self) -> None:
        secret = "private-host-hook-canary"

        async def hook(_request: httpx.Request) -> None:
            raise RuntimeError(secret)

        async def unexpected(_request: Request, _writer: asyncio.StreamWriter) -> None:
            self.fail("hook refused before transport")

        async with Server(unexpected) as server:
            async with client(server, event_hooks={"request": [hook]}) as transport:
                with self.assertRaises(GmailError) as caught:
                    await GmailOperations(transport, user_id="me").send_raw(MIME)
                error = caught.exception
                self.assertEqual(error.kind, FailureKind.CLIENT)
                self.assertEqual(error.write_outcome, WriteOutcome.UNKNOWN)
                self.assertNotIn(secret, "".join(traceback.format_exception(error)))
                self.assertIsNone(error.__context__)
                self.assertIsNone(error.__cause__)
            self.assertEqual(server.requests, [])
        self.assertTrue(server.joined)

    async def test_body_failure_preserves_operation_and_all_fixture_joins(self) -> None:
        entered, release, completed = asyncio.Event(), asyncio.Event(), asyncio.Event()

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            entered.set()
            await release.wait()
            completed.set()
            await respond(writer, RECEIPT)

        server = Server(handler)
        operation: asyncio.Task[Any] | None = None
        with self.assertRaisesRegex(AssertionError, "original body failure"):
            async with server:
                server.releases.append(release)
                async with client(server) as transport:
                    operation = asyncio.create_task(
                        GmailOperations(transport, user_id="me").send_raw(MIME)
                    )
                    try:
                        await asyncio.wait_for(entered.wait(), 2)
                        raise AssertionError("original body failure")
                    finally:
                        release.set()
                        await asyncio.gather(operation, return_exceptions=True)
        self.assertTrue(completed.is_set())
        self.assertTrue(server.joined)
        self.assertTrue(operation is not None and operation.done())


if __name__ == "__main__":
    unittest.main()
