"""Actual loopback Slack wire tests. No live Slack, OAuth or verified account."""

from __future__ import annotations

import asyncio
import json
import time
import traceback
from typing import Any
import unittest
from urllib.parse import parse_qs, urlsplit

import anyio
import httpx
from meerkat_toolkit.httpx_client import prepare_httpx_client

from meerkat_toolkit.connectors.slack import FailureKind, SlackError, SlackOperations
from test_home_assistant import Request, Server, client, respond


CANARY = "fixture-sensitive-token-message-cursor"


def page(ts: str = "1700000000.000001") -> dict[str, Any]:
    return {"ok": True, "messages": [{"type": "message", "ts": ts, "text": "read"}]}


class ReadableServer(Server):
    """Expose the same owned stream to observe actual client-side disconnect."""

    async def _serve(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        if not hasattr(self, "readers"):
            self.readers: dict[asyncio.StreamWriter, asyncio.StreamReader] = {}
        self.readers[writer] = reader
        try:
            await super()._serve(reader, writer)
        finally:
            del self.readers[writer]

    async def peer_closed(self, writer: asyncio.StreamWriter) -> bool:
        return await asyncio.wait_for(self.readers[writer].read(1), 2) == b""


class SlackTests(unittest.IsolatedAsyncioTestCase):
    def safe(self, error: BaseException) -> None:
        self.assertNotIn(CANARY, "".join(traceback.format_exception(error)))
        self.assertNotIn(CANARY, repr(vars(error)))
        self.assertIsNone(error.__context__)
        self.assertIsNone(error.__cause__)

    async def test_history_exact_prefix_query_timestamps_and_borrowed_client(self) -> None:
        result = page("1700000000.12345678901234567890")
        result.update(has_more=True, response_metadata={"next_cursor": "opaque+/=", "extra": None})
        result["messages"][0]["blocks"] = [{"type": "rich_text", "elements": [False, None]}]

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, result)

        async with Server(handler) as server:
            async with prepare_httpx_client(httpx.AsyncClient(
                base_url=server.url + "selected/proxy/",
                headers={"Authorization": "Bearer selected-account"},
                params={"channel": "CWRONG", "token": CANARY, "cursor": "wrong"},
                trust_env=False,
            )) as transport:
                operations = SlackOperations(transport)
                transport.base_url = "http://unused.invalid/"
                actual = await operations.history_page(
                    "CSELECTED",
                    oldest="0001700000000.000000000000000001",
                    latest="1700604800.999999999999999999",
                    inclusive=True,
                    cursor="opaque+/= with space",
                    limit=999,
                    include_all_metadata=True,
                )
                self.assertEqual(actual, result)
                self.assertFalse(transport.is_closed)
            self.assertEqual(len(server.requests), 1)
            request = server.requests[0]
            self.assertEqual(request.method, "GET")
            self.assertEqual(request.body, b"")
            self.assertEqual(
                urlsplit(request.target).path, "/selected/proxy/api/conversations.history"
            )
            self.assertEqual(
                parse_qs(urlsplit(request.target).query),
                {
                    "channel": ["CSELECTED"],
                    "limit": ["999"],
                    "inclusive": ["true"],
                    "include_all_metadata": ["true"],
                    "oldest": ["0001700000000.000000000000000001"],
                    "latest": ["1700604800.999999999999999999"],
                    "cursor": ["opaque+/= with space"],
                },
            )
            self.assertNotIn(CANARY, request.target)
            self.assertEqual(request.headers["authorization"], "Bearer selected-account")
            self.assertEqual(request.headers["accept-encoding"], "identity")
        self.assertTrue(server.joined)

    async def test_replies_preserves_parent_duplicates_nested_data_and_exact_ts(self) -> None:
        parent = {
            "ts": "1700000000.000001",
            "reply_users": ["B123", "U456"],
            "reply_count": 2,
            "metadata": {"event_payload": {"enabled": False, "empty": None}},
        }
        child = {"ts": "1700000000.000002", "thread_ts": parent["ts"], "text": "reply"}
        result = {"ok": True, "messages": [parent, child, child], "has_more": False}

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, result)

        async with Server(handler) as server:
            async with client(server) as transport:
                actual = await SlackOperations(transport).replies_page(
                    "GTHREAD", "1700000000.000001", oldest="0", latest="1800000000.000003"
                )
                self.assertEqual(actual, result)
            request = server.requests[0]
            self.assertEqual(urlsplit(request.target).path, "/api/conversations.replies")
            self.assertEqual(
                parse_qs(urlsplit(request.target).query),
                {
                    "channel": ["GTHREAD"],
                    "ts": ["1700000000.000001"],
                    "limit": ["15"],
                    "inclusive": ["false"],
                    "include_all_metadata": ["false"],
                    "oldest": ["0"],
                    "latest": ["1800000000.000003"],
                },
            )
        self.assertTrue(server.joined)

    async def test_two_selected_account_clients_same_endpoint_and_channel(self) -> None:
        entered = asyncio.Event()
        release = asyncio.Event()

        async def handler(request: Request, writer: asyncio.StreamWriter) -> None:
            if len(server.requests) == 2:
                entered.set()
            await release.wait()
            result = page()
            result["selected_fixture_account"] = {
                "Bearer workspace-a-user-1": "workspace-a-user-1",
                "Bearer workspace-b-user-2": "workspace-b-user-2",
            }[request.headers["authorization"]]
            await respond(writer, result)

        async with Server(handler) as server:
            server.releases.append(release)
            async with (
                client(server, "workspace-a-user-1") as a,
                client(server, "workspace-b-user-2") as b,
            ):
                tasks = [
                    asyncio.create_task(SlackOperations(a).history_page("CSAME")),
                    asyncio.create_task(SlackOperations(b).history_page("CSAME")),
                ]
                try:
                    await asyncio.wait_for(entered.wait(), 2)
                    release.set()
                    results = await asyncio.gather(*tasks)
                    self.assertEqual(
                        [r["selected_fixture_account"] for r in results],
                        ["workspace-a-user-1", "workspace-b-user-2"],
                    )
                    self.assertFalse(a.is_closed or b.is_closed)
                finally:
                    release.set()
                    for task in tasks:
                        if not task.done():
                            task.cancel()
                    await asyncio.gather(*tasks, return_exceptions=True)
            self.assertEqual(len(server.requests), 2)
            self.assertEqual(server.requests[0].target, server.requests[1].target)
        self.assertTrue(server.joined)

    async def test_short_empty_and_repeated_cursor_pages_are_not_followed(self) -> None:
        first = {
            "ok": True,
            "messages": [],
            "has_more": True,
            "response_metadata": {"next_cursor": "same="},
        }

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            if len(server.requests) <= 2:
                await respond(writer, first)
            else:
                await respond(writer, {"ok": False, "error": "invalid_cursor"})

        async with Server(handler) as server:
            async with client(server) as transport:
                operations = SlackOperations(transport)
                self.assertEqual(await operations.history_page("C1"), first)
                self.assertEqual(len(server.requests), 1)
                self.assertEqual(await operations.history_page("C1", cursor="same="), first)
                self.assertEqual(len(server.requests), 2)
                with self.assertRaises(SlackError) as caught:
                    await operations.history_page("C1", cursor="same=")
                self.assertEqual(caught.exception.provider_code, "invalid_cursor")
                self.assertEqual(len(server.requests), 3)
        self.assertTrue(server.joined)

    async def test_absent_null_empty_cursor_and_false_flags_remain_distinct(self) -> None:
        variants = [
            page(),
            {**page(), "response_metadata": {}},
            {**page(), "response_metadata": {"next_cursor": None}},
            {**page(), "has_more": False, "response_metadata": {"next_cursor": ""}},
        ]

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, variants[len(server.requests) - 1])

        async with Server(handler) as server:
            async with client(server) as transport:
                operations = SlackOperations(transport)
                for result in variants:
                    self.assertEqual(await operations.history_page("C1"), result)
            self.assertEqual(len(server.requests), len(variants))
        self.assertTrue(server.joined)

    async def test_invalid_inputs_never_enter_provider(self) -> None:
        async def handler(_request: Request, _writer: asyncio.StreamWriter) -> None:
            self.fail("invalid local input entered provider")

        async with Server(handler) as server:
            async with client(server) as transport:
                operations = SlackOperations(transport)
                for channel in ("", "#general", "C1/path", "C1\n", "C" * 129, None, 1):
                    with self.subTest(channel=channel), self.assertRaises(ValueError):
                        await operations.history_page(channel)  # type: ignore[arg-type]
                for argument, values in {
                    "limit": (0, 1000, True, 1.5),
                    "oldest": (1.5, "NaN", "1e9", "-1", "1.", "1" * 65, "1\n"),
                    "latest": (False, ".1", "1 2"),
                    "cursor": ("", "bad\r", "\ud800", "x" * 8193, 1),
                    "inclusive": (1, "true", None),
                    "include_all_metadata": (0, "false", None),
                }.items():
                    for value in values:
                        with (
                            self.subTest(argument=argument, value=value),
                            self.assertRaises(ValueError),
                        ):
                            await operations.history_page("C1", **{argument: value})
                for ts in (1.0, "-1", "1e3", "", "1" * 65):
                    with self.subTest(ts=ts), self.assertRaises(ValueError):
                        await operations.replies_page("C1", ts)  # type: ignore[arg-type]
            self.assertEqual(server.requests, [])
        self.assertTrue(server.joined)

    async def test_invalid_client_and_limits_refused_locally(self) -> None:
        for base in (
            "ftp://example.invalid/",
            "https://user:pass@example.invalid/",
            "https://example.invalid/?token=bad",
            "https://example.invalid/#bad",
        ):
            async with prepare_httpx_client(httpx.AsyncClient(base_url=base)) as transport:
                with self.subTest(base=base), self.assertRaises(ValueError):
                    SlackOperations(transport)
        async with prepare_httpx_client(httpx.AsyncClient(base_url="https://example.invalid/")) as transport:
            for value in (0, -1, float("inf"), float("nan"), True, "1"):
                with self.subTest(timeout=value), self.assertRaises(ValueError):
                    SlackOperations(transport, timeout_seconds=value)  # type: ignore[arg-type]
            for value in (0, -1, 1.0, True, "1"):
                with self.subTest(size=value), self.assertRaises(ValueError):
                    SlackOperations(transport, max_response_bytes=value)  # type: ignore[arg-type]

    async def test_provider_ok_false_maps_known_codes_without_body_leaks(self) -> None:
        cases = [
            ("not_authed", FailureKind.AUTHENTICATION),
            ("invalid_auth", FailureKind.AUTHENTICATION),
            ("token_expired", FailureKind.AUTHENTICATION),
            ("token_revoked", FailureKind.AUTHENTICATION),
            ("missing_scope", FailureKind.PERMISSION),
            ("not_in_channel", FailureKind.PERMISSION),
            ("not_allowed_token_type", FailureKind.PERMISSION),
            ("team_access_not_granted", FailureKind.PERMISSION),
            ("channel_not_found", FailureKind.NOT_FOUND),
            ("thread_not_found", FailureKind.NOT_FOUND),
            ("invalid_cursor", FailureKind.PROVIDER),
            ("ratelimited", FailureKind.RATE_LIMITED),
        ]

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(
                writer,
                {
                    "ok": False,
                    "error": cases[len(server.requests) - 1][0],
                    "needed": CANARY,
                    "response_metadata": {"messages": [CANARY]},
                },
                headers={"Retry-After": "17"},
            )

        async with Server(handler) as server:
            async with client(server) as transport:
                for code, kind in cases:
                    with self.subTest(code=code), self.assertRaises(SlackError) as caught:
                        await SlackOperations(transport).replies_page("C1", "1.000001")
                    self.assertEqual(caught.exception.kind, kind)
                    self.assertEqual(caught.exception.provider_code, code)
                    self.assertEqual(caught.exception.status_code, 200)
                    self.assertEqual(caught.exception.retry_after_seconds, 17)
                    self.safe(caught.exception)
            self.assertEqual(len(server.requests), len(cases))
        self.assertTrue(server.joined)

    async def test_unknown_provider_error_is_redacted_without_retry(self) -> None:
        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, {"ok": False, "error": CANARY, "messages": [CANARY]})

        async with Server(handler) as server:
            async with client(server) as transport:
                with self.assertRaises(SlackError) as caught:
                    await SlackOperations(transport).history_page("C1", cursor=CANARY)
                self.assertEqual(caught.exception.kind, FailureKind.PROVIDER)
                self.assertIsNone(caught.exception.provider_code)
                self.safe(caught.exception)
            self.assertEqual(len(server.requests), 1)
        self.assertTrue(server.joined)

    async def test_http_status_and_retry_after_metadata_never_retry(self) -> None:
        cases = [
            (401, "1", FailureKind.AUTHENTICATION),
            (403, "2", FailureKind.PERMISSION),
            (404, "0", FailureKind.NOT_FOUND),
            (429, "60", FailureKind.RATE_LIMITED),
            (429, CANARY, FailureKind.RATE_LIMITED),
            (429, "9" * 10, FailureKind.RATE_LIMITED),
            (503, "-1", FailureKind.HTTP_STATUS),
        ]

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            status, retry, _kind = cases[len(server.requests) - 1]
            await respond(
                writer,
                {"ok": True, "messages": [CANARY]},
                status=status,
                headers={"Retry-After": retry},
            )

        async with Server(handler) as server:
            async with client(server) as transport:
                for status, retry, kind in cases:
                    with (
                        self.subTest(status=status, retry=retry),
                        self.assertRaises(SlackError) as caught,
                    ):
                        await SlackOperations(transport).history_page("C1")
                    self.assertEqual(caught.exception.kind, kind)
                    self.assertEqual(caught.exception.status_code, status)
                    self.assertEqual(
                        caught.exception.retry_after_seconds,
                        int(retry) if retry.isdecimal() and len(retry) <= 9 else None,
                    )
                    self.safe(caught.exception)
            self.assertEqual(len(server.requests), len(cases))
        self.assertTrue(server.joined)

    async def test_redirect_never_enters_other_endpoint_even_with_client_follow_enabled(
        self,
    ) -> None:
        async def other(_request: Request, _writer: asyncio.StreamWriter) -> None:
            self.fail("redirect destination was entered")

        async with Server(other) as destination:

            async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
                await respond(
                    writer, {}, status=307, headers={"Location": destination.url + CANARY}
                )

            async with Server(handler) as server:
                async with client(server, follow_redirects=True) as transport:
                    with self.assertRaises(SlackError) as caught:
                        await SlackOperations(transport).history_page("C1")
                    self.assertEqual(caught.exception.kind, FailureKind.REDIRECT)
                    self.safe(caught.exception)
                self.assertEqual(len(server.requests), 1)
            self.assertEqual(destination.requests, [])
        self.assertTrue(server.joined and destination.joined)

    async def test_malformed_json_and_page_shapes_fail_without_leaking_response(self) -> None:
        invalid = [
            b'{"ok":true,"ok":false,"messages":[]}',
            b'{"ok":true,"messages":[],"x":NaN}',
            b'{"ok":true,"messages":[],"x":1e999}',
            b"\xff",
            b"{",
            b'{"ok":true,"messages":[],"nested":{"x":1,"x":2}}',
        ]
        invalid += [
            json.dumps(value).encode()
            for value in [
                None,
                [],
                {},
                {"ok": 1, "messages": []},
                {"ok": "true", "messages": []},
                {"ok": False},
                {"ok": False, "error": None},
                {"ok": True, "messages": None},
                {"ok": True, "messages": [1]},
                {"ok": True, "messages": [{"ts": 1.1}]},
                {**page(), "has_more": 0},
                {**page(), "response_metadata": None},
                {**page(), "response_metadata": {"next_cursor": False}},
            ]
        ]

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, None, raw=invalid[len(server.requests) - 1])

        async with Server(handler) as server:
            async with client(server) as transport:
                for raw in invalid:
                    with self.subTest(raw=raw), self.assertRaises(SlackError) as caught:
                        await SlackOperations(transport).history_page("C1")
                    self.assertEqual(caught.exception.kind, FailureKind.INVALID_RESPONSE)
                    self.safe(caught.exception)
            self.assertEqual(len(server.requests), len(invalid))
        self.assertTrue(server.joined)

    async def test_encoding_and_content_type_are_checked(self) -> None:
        headers = [{"Content-Encoding": "gzip"}, {"Content-Type": "text/html"}]

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, page(), headers=headers[len(server.requests) - 1])

        async with Server(handler) as server:
            async with client(server) as transport:
                for kind in (FailureKind.UNSUPPORTED_ENCODING, FailureKind.INVALID_RESPONSE):
                    with self.assertRaises(SlackError) as caught:
                        await SlackOperations(transport).history_page("C1")
                    self.assertEqual(caught.exception.kind, kind)
        self.assertTrue(server.joined)

    async def test_declared_oversized_body_is_closed_without_waiting_for_body(self) -> None:
        closed = asyncio.Event()

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            writer.write(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 999999\r\n\r\n"
            )
            await writer.drain()
            self.assertTrue(await server.peer_closed(writer))
            closed.set()

        async with ReadableServer(handler) as server:
            async with client(server) as transport:
                with self.assertRaises(SlackError) as caught:
                    await SlackOperations(transport, max_response_bytes=32).history_page("C1")
                self.assertEqual(caught.exception.kind, FailureKind.RESPONSE_TOO_LARGE)
                await asyncio.wait_for(closed.wait(), 2)
        self.assertTrue(server.joined)

    async def test_streamed_oversized_body_is_closed_without_end_chunk(self) -> None:
        closed = asyncio.Event()

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            writer.write(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n40\r\n"
                + b"x" * 64
                + b"\r\n"
            )
            await writer.drain()
            self.assertTrue(await server.peer_closed(writer))
            closed.set()

        async with ReadableServer(handler) as server:
            async with client(server) as transport:
                with self.assertRaises(SlackError) as caught:
                    await SlackOperations(transport, max_response_bytes=32).history_page("C1")
                self.assertEqual(caught.exception.kind, FailureKind.RESPONSE_TOO_LARGE)
                await asyncio.wait_for(closed.wait(), 2)
        self.assertTrue(server.joined)

    async def test_total_deadline_expires_despite_regular_streamed_bytes(self) -> None:
        closed = asyncio.Event()
        chunks = 0

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            nonlocal chunks
            writer.write(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n"
            )
            await writer.drain()
            # Keep every HTTPX read within its inactivity timeout while the
            # single operation deadline still expires. The same handler owns
            # both reads and writes; no detached producer task is needed.
            while True:
                try:
                    incoming = await asyncio.wait_for(server.readers[writer].read(1), 0.05)
                except TimeoutError:
                    writer.write(b"1\r\n \r\n")
                    await writer.drain()
                    chunks += 1
                else:
                    self.assertEqual(incoming, b"")
                    closed.set()
                    break

        async with ReadableServer(handler) as server:
            async with client(server) as transport:
                started = time.monotonic()
                with self.assertRaises(SlackError) as caught:
                    await SlackOperations(transport, timeout_seconds=0.4).history_page("C1")
                self.assertEqual(caught.exception.kind, FailureKind.TIMEOUT)
                self.assertLess(time.monotonic() - started, 2)
                self.assertGreaterEqual(chunks, 3)
                await asyncio.wait_for(closed.wait(), 2)
        self.assertTrue(server.joined)

    async def test_cancellation_before_response_headers_closes_accepted_socket(self) -> None:
        await self.cancel_case(False)

    async def test_cancellation_during_response_body_closes_accepted_socket(self) -> None:
        await self.cancel_case(True)

    async def cancel_case(self, send_headers: bool) -> None:
        entered = asyncio.Event()
        closed = asyncio.Event()

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            if send_headers:
                writer.write(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 10000\r\n\r\n{"
                )
                await writer.drain()
            entered.set()
            self.assertTrue(await server.peer_closed(writer))
            closed.set()

        async with ReadableServer(handler) as server:
            async with client(server) as transport:
                task = asyncio.create_task(
                    SlackOperations(transport).replies_page("C1", "1.000001")
                )
                try:
                    await asyncio.wait_for(entered.wait(), 2)
                    task.cancel(CANARY)
                    with self.assertRaises(asyncio.CancelledError) as caught:
                        await task
                    self.safe(caught.exception)
                    await asyncio.wait_for(closed.wait(), 2)
                    self.assertFalse(transport.is_closed)
                finally:
                    if not task.done():
                        task.cancel()
                    await asyncio.gather(task, return_exceptions=True)
            self.assertEqual(len(server.requests), 1)
        self.assertTrue(server.joined)

    async def test_premature_body_disconnect_is_transport_failure(self) -> None:
        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            writer.write(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 999\r\n\r\n"
                + CANARY.encode()
            )
            await writer.drain()

        async with Server(handler) as server:
            async with client(server) as transport:
                with self.assertRaises(SlackError) as caught:
                    await SlackOperations(transport).history_page("C1")
                self.assertEqual(caught.exception.kind, FailureKind.TRANSPORT)
                self.safe(caught.exception)
            self.assertEqual(len(server.requests), 1)
        self.assertTrue(server.joined)

    async def test_host_hook_error_is_safe_and_never_enters_provider(self) -> None:
        async def hook(_request: httpx.Request) -> None:
            raise RuntimeError(CANARY)

        async def handler(_request: Request, _writer: asyncio.StreamWriter) -> None:
            self.fail("failed host hook still sent a request")

        async with Server(handler) as server:
            async with client(server, event_hooks={"request": [hook]}) as transport:
                with self.assertRaises(SlackError) as caught:
                    await SlackOperations(transport).history_page("C1")
                self.assertEqual(caught.exception.kind, FailureKind.CLIENT)
                self.safe(caught.exception)
            self.assertEqual(server.requests, [])
        self.assertTrue(server.joined)

    async def test_fixture_joins_actual_handlers_after_body_assertion(self) -> None:
        entered = asyncio.Event()
        release = asyncio.Event()

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            entered.set()
            await release.wait()
            await respond(writer, page())

        server = Server(handler)
        task: asyncio.Task[Any] | None = None
        with self.assertRaisesRegex(AssertionError, "intentional fixture body failure"):
            async with server:
                server.releases.append(release)
                async with client(server) as transport:
                    try:
                        task = asyncio.create_task(SlackOperations(transport).history_page("C1"))
                        await asyncio.wait_for(entered.wait(), 2)
                        raise AssertionError("intentional fixture body failure")
                    finally:
                        release.set()
                        if task is not None:
                            await asyncio.gather(task, return_exceptions=True)
        self.assertTrue(server.joined)
        self.assertEqual(len(server.tasks), 1)
        self.assertTrue(all(task.done() and task.exception() is None for task in server.tasks))

    async def test_repeated_caller_cancel_joins_before_borrowed_client_close(self) -> None:
        # Same default-transport timing as the independent five-case probe.
        for repeat in (False, True, True, True, True, True):
            with self.subTest(repeat=repeat):
                headers_seen = asyncio.Event()
                peer_eof = asyncio.Event()
                owners: list[asyncio.Task[Any]] = []

                async def response_hook(_response: httpx.Response) -> None:
                    owner = asyncio.current_task()
                    assert owner is not None
                    owners.append(owner)
                    headers_seen.set()

                async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
                    writer.write(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
                        b"Content-Length: 10000\r\n\r\n{"
                    )
                    await writer.drain()
                    self.assertTrue(await server.peer_closed(writer))
                    peer_eof.set()

                async with ReadableServer(handler) as server:
                    async with client(
                        server, event_hooks={"response": [response_hook]}
                    ) as transport:
                        task = asyncio.create_task(SlackOperations(transport).history_page("C1"))
                        try:
                            await asyncio.wait_for(headers_seen.wait(), 2)
                            await asyncio.sleep(0)
                            self.assertTrue(task.cancel(CANARY))
                            if repeat:
                                await asyncio.sleep(0)
                                self.assertTrue(task.cancel(CANARY))
                            with self.assertRaises(asyncio.CancelledError) as caught:
                                await asyncio.wait_for(task, 2)
                            self.safe(caught.exception)
                            await asyncio.wait_for(peer_eof.wait(), 0.25)
                            await asyncio.wait_for(asyncio.gather(*server.tasks), 2)
                            self.assertEqual(len(server.tasks), 1)
                            self.assertTrue(all(owner.done() for owner in owners))
                            self.assertTrue(all(owner is not task for owner in owners))
                            self.assertTrue(all(owner.done() for owner in server.tasks))
                            self.assertFalse(transport.is_closed)
                            self.assertEqual(len(server.requests), 1)
                        finally:
                            if not task.done():
                                task.cancel()
                            await asyncio.gather(task, return_exceptions=True)
                self.assertTrue(server.joined)

    async def test_caller_cancel_before_inner_scope_enters_is_joined_without_request(self) -> None:
        entered = asyncio.Event()
        release = asyncio.Event()
        owners: list[asyncio.Task[Any]] = []
        scopes: list[anyio.CancelScope] = []

        class DelayedStart(SlackOperations):
            async def _request(
                self, method: str, params: list[tuple[str, str]], scope: anyio.CancelScope
            ) -> dict[str, Any]:
                # A scheduling gate before the real implementation enters its
                # scope, not a substitute response, transport or cleanup path.
                owner = asyncio.current_task()
                assert owner is not None
                owners.append(owner)
                scopes.append(scope)
                entered.set()
                await release.wait()
                return await super()._request(method, params, scope)

        async def handler(_request: Request, _writer: asyncio.StreamWriter) -> None:
            self.fail("pre-entry cancellation sent a request")

        async with Server(handler) as server:
            async with client(server) as transport:
                task = asyncio.create_task(DelayedStart(transport).history_page("C1"))
                try:
                    await asyncio.wait_for(entered.wait(), 2)
                    self.assertTrue(task.cancel(CANARY))
                    await asyncio.sleep(0)
                    self.assertTrue(scopes[0].cancel_called)
                    self.assertTrue(task.cancel(CANARY))
                    await asyncio.sleep(0)
                    self.assertFalse(task.done())
                    self.assertFalse(owners[0].done())
                    release.set()
                    with self.assertRaises(asyncio.CancelledError) as caught:
                        await asyncio.wait_for(task, 2)
                    self.safe(caught.exception)
                    self.assertTrue(owners[0].done())
                    self.assertFalse(transport.is_closed)
                    self.assertEqual(server.requests, [])
                finally:
                    release.set()
                    if not task.done():
                        task.cancel()
                    await asyncio.gather(task, return_exceptions=True)
        self.assertTrue(server.joined)

    async def test_caller_cancel_during_ordinary_transport_error_cleanup(self) -> None:
        await self.held_close_case("error_then_caller")

    async def test_deadline_then_caller_cancel_cannot_interrupt_entered_cleanup(self) -> None:
        await self.held_close_case("deadline_then_caller")

    async def test_caller_cancel_then_original_deadline_cannot_interrupt_cleanup(self) -> None:
        await self.held_close_case("caller_then_deadline")

    async def test_ordinary_transport_error_without_cancellation_joins_owner(self) -> None:
        await self.held_close_case("error_control")

    async def test_deadline_without_caller_cancellation_joins_owner(self) -> None:
        await self.held_close_case("deadline_control")

    async def held_close_case(self, mode: str) -> None:
        close_entered = asyncio.Event()
        headers_seen = asyncio.Event()
        close_release = asyncio.Event()
        close_finished = asyncio.Event()
        peer_eof = asyncio.Event()
        owners: list[asyncio.Task[Any]] = []
        error = mode.startswith("error")

        async def trace(event: str, _info: dict[str, Any]) -> None:
            if event == "http11.response_closed.started":
                owner = asyncio.current_task()
                assert owner is not None
                owners.append(owner)
                close_entered.set()
                # Public HTTPX trace extension, a finite test-only scheduling
                # gate inside the actual default transport's entered close.
                await close_release.wait()
            elif event == "http11.response_closed.complete":
                close_finished.set()

        async def request_hook(request: httpx.Request) -> None:
            request.extensions["trace"] = trace

        async def response_hook(_response: httpx.Response) -> None:
            headers_seen.set()

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            if error:
                writer.write(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
                    b"Transfer-Encoding: chunked\r\n\r\ninvalid-chunk-size\r\n"
                )
            else:
                writer.write(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
                    b"Content-Length: 10000\r\n\r\n{"
                )
            await writer.drain()
            self.assertTrue(await server.peer_closed(writer))
            peer_eof.set()

        async with ReadableServer(handler) as server:
            server.releases.append(close_release)
            async with client(
                server, event_hooks={"request": [request_hook], "response": [response_hook]}
            ) as transport:
                task = asyncio.create_task(
                    SlackOperations(transport, timeout_seconds=0.2).replies_page("C1", "1.000001")
                )
                try:
                    await asyncio.wait_for(headers_seen.wait(), 2)
                    if mode == "caller_then_deadline":
                        self.assertTrue(task.cancel(CANARY))
                    await asyncio.wait_for(close_entered.wait(), 2)
                    self.assertFalse(peer_eof.is_set())
                    self.assertFalse(close_finished.is_set())
                    self.assertFalse(owners[0].done())
                    if mode.endswith("then_caller"):
                        self.assertTrue(task.cancel(CANARY))
                        await asyncio.sleep(0)
                        self.assertTrue(task.cancel(CANARY))
                    if mode in {"error_then_caller", "caller_then_deadline"}:
                        # Remain in the same entered close beyond the original
                        # deadline. Caller cancellation must still win after join.
                        await asyncio.sleep(0.25)
                    await asyncio.sleep(0)
                    self.assertFalse(task.done())
                    self.assertFalse(owners[0].done())
                    close_release.set()
                    if mode.endswith("control"):
                        with self.assertRaises(SlackError) as caught:
                            await asyncio.wait_for(task, 2)
                        self.assertEqual(
                            caught.exception.kind,
                            FailureKind.TRANSPORT if error else FailureKind.TIMEOUT,
                        )
                        self.safe(caught.exception)
                    else:
                        with self.assertRaises(asyncio.CancelledError) as cancelled:
                            await asyncio.wait_for(task, 2)
                        self.safe(cancelled.exception)
                    self.assertTrue(close_finished.is_set())
                    await asyncio.wait_for(peer_eof.wait(), 0.25)
                    await asyncio.wait_for(asyncio.gather(*server.tasks), 2)
                    self.assertEqual(len(server.tasks), 1)
                    self.assertTrue(all(owner.done() for owner in owners))
                    self.assertTrue(all(owner is not task for owner in owners))
                    self.assertTrue(all(owner.done() for owner in server.tasks))
                    self.assertFalse(transport.is_closed)
                    self.assertEqual(len(server.requests), 1)
                finally:
                    close_release.set()
                    if not task.done():
                        task.cancel()
                    await asyncio.gather(task, return_exceptions=True)
        self.assertTrue(server.joined)

    async def test_success_joins_operation_owner_and_preserves_result(self) -> None:
        owners: list[asyncio.Task[Any]] = []
        result = page("1700000000.123456789")

        async def response_hook(_response: httpx.Response) -> None:
            owner = asyncio.current_task()
            assert owner is not None
            owners.append(owner)

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, result)

        async with Server(handler) as server:
            async with client(server, event_hooks={"response": [response_hook]}) as transport:
                self.assertEqual(await SlackOperations(transport).history_page("C1"), result)
                await asyncio.wait_for(asyncio.gather(*server.tasks), 2)
                self.assertEqual(len(owners), 1)
                self.assertTrue(owners[0].done())
                self.assertIsNot(owners[0], asyncio.current_task())
                self.assertTrue(all(owner.done() for owner in server.tasks))
                self.assertFalse(transport.is_closed)
                self.assertEqual(len(server.requests), 1)
        self.assertTrue(server.joined)


if __name__ == "__main__":
    unittest.main()
