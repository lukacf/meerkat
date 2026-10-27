"""Real HTTP/1.1 loopback protocol tests; no live Home Assistant or credentials.

The selected HA source also has a real provider test for service continuation
after disconnect. These fixtures reproduce its wire consequences, not the full
HA implementation, entity policy, device outcome or credential lifecycle.
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
import json
import socket
import traceback
from typing import Any
import unittest

import httpx
from meerkat_toolkit.httpx_client import prepare_httpx_client

from meerkat_toolkit.connectors.home_assistant import (
    FailureKind,
    HomeAssistantError,
    HomeAssistantOperations,
    ServiceCallCancelled,
    WriteOutcome,
)


@dataclass(frozen=True)
class Request:
    method: str
    target: str
    headers: dict[str, str]
    body: bytes


Handler = Callable[[Request, asyncio.StreamWriter], Awaitable[None]]


class Server:
    """Own the listener and every accepted handler, including failed bodies."""

    def __init__(self, handler: Handler) -> None:
        self.handler = handler
        self.requests: list[Request] = []
        self.tasks: list[asyncio.Task[None]] = []
        self.releases: list[asyncio.Event] = []
        self.joined = False
        self.accepted = asyncio.Event()

    async def __aenter__(self) -> Server:
        # Accept and register the socket in one synchronous loop callback. The
        # start_server stream callback can arrive after close/wait_closed on
        # Python 3.11, too late for a snapshot of its handler tasks to own it.
        self.loop = asyncio.get_running_loop()
        self.listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.listener.setblocking(False)
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(64)
        self.loop.add_reader(self.listener.fileno(), self._accept)
        port = self.listener.getsockname()[1]
        self.url = f"http://127.0.0.1:{port}/"
        return self

    def _accept(self) -> None:
        while True:
            try:
                connection, _address = self.listener.accept()
            except (BlockingIOError, InterruptedError):
                return
            connection.setblocking(False)
            self.tasks.append(asyncio.create_task(self._serve_socket(connection)))
            self.accepted.set()

    async def _serve_socket(self, connection: socket.socket) -> None:
        try:
            reader, writer = await asyncio.open_connection(sock=connection)
            await self._serve(reader, writer)
        finally:
            connection.close()

    async def _serve(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        try:
            try:
                head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 5)
            except asyncio.IncompleteReadError as error:
                if not error.partial:
                    return  # An accepted connection closed before any HTTP request.
                raise
            if len(head) > 16_384:
                raise AssertionError("unexpected oversized fixture request headers")
            lines = head.decode("ascii").split("\r\n")
            method, target, version = lines[0].split(" ")
            if version != "HTTP/1.1":
                raise AssertionError("fixture expects actual HTTP/1.1")
            headers = {
                name.lower(): value
                for name, value in (line.split(": ", 1) for line in lines[1:] if line)
            }
            length = int(headers.get("content-length", "0"))
            if not 0 <= length <= 65_536:
                raise AssertionError("unexpected fixture request body size")
            body = await asyncio.wait_for(reader.readexactly(length), 5)
            request = Request(method, target, headers, body)
            self.requests.append(request)
            await self.handler(request, writer)
        finally:
            writer.close()
            try:
                await writer.wait_closed()
            except (BrokenPipeError, ConnectionResetError):
                pass  # Client disconnect is an intentional test input.

    async def __aexit__(self, _type: Any, body_error: BaseException | None, _tb: Any) -> bool:
        # No callback can interleave with this synchronous retirement. Removing
        # the reader also cancels a queued readiness callback. Every accepted
        # socket already has its retained task before the first await below.
        self.loop.remove_reader(self.listener.fileno())
        self.listener.close()
        for event in self.releases:
            event.set()
        errors: list[BaseException] = []
        if self.tasks:
            done, pending = await asyncio.wait(self.tasks, timeout=5)
            if pending:
                errors.append(AssertionError("fixture handlers needed forced cancellation"))
                for task in pending:
                    task.cancel()
            # Await every actual owner even when another failed or needed abort.
            outcomes = await asyncio.gather(*self.tasks, return_exceptions=True)
            errors.extend(outcome for outcome in outcomes if isinstance(outcome, BaseException))
            self.joined = not pending and len(done) == len(self.tasks)
        else:
            self.joined = True
        if errors:
            if body_error is not None:
                errors.insert(0, body_error)
            raise BaseExceptionGroup("loopback body/cleanup failures", errors)
        return False


async def respond(
    writer: asyncio.StreamWriter,
    value: Any,
    *,
    status: int = 200,
    headers: dict[str, str] | None = None,
    raw: bytes | None = None,
) -> None:
    body = json.dumps(value).encode() if raw is None else raw
    response_headers = {
        "Content-Type": "application/json",
        "Content-Length": str(len(body)),
        "Connection": "close",
        **(headers or {}),
    }
    head = (
        f"HTTP/1.1 {status} Fixture\r\n"
        + "".join(f"{name}: {value}\r\n" for name, value in response_headers.items())
        + "\r\n"
    )
    writer.write(head.encode("ascii") + body)
    await writer.drain()


def state(entity: str, value: str = "on") -> dict[str, Any]:
    return {
        "entity_id": entity,
        "state": value,
        "attributes": {"nested": {"unicode": "kök", "items": [1, True, None]}},
        "last_changed": "2026-09-30T12:00:00+00:00",
        "context": {"id": "provider-context", "parent_id": None},
    }


def client(server: Server, token: str = "fixture-token", **kwargs: Any) -> httpx.AsyncClient:
    return prepare_httpx_client(httpx.AsyncClient(
        base_url=server.url,
        headers={"Authorization": f"Bearer {token}"},
        trust_env=False,
        **kwargs,
    ))


class HomeAssistantTests(unittest.IsolatedAsyncioTestCase):
    async def test_fixture_retires_accepted_socket_before_http_request(self) -> None:
        async def handler(_request: Request, _writer: asyncio.StreamWriter) -> None:
            self.fail("no HTTP request was sent")

        async with Server(handler) as server:
            _reader, writer = await asyncio.open_connection(
                "127.0.0.1", server.listener.getsockname()[1]
            )
            await asyncio.wait_for(server.accepted.wait(), 2)
            writer.close()
            await writer.wait_closed()
        self.assertTrue(server.joined)
        self.assertEqual(server.requests, [])
        self.assertGreater(len(server.tasks), 0)
        self.assertTrue(all(task.done() and task.exception() is None for task in server.tasks))

    async def test_reads_preserve_provider_data_exact_prefix_and_borrowed_client(self) -> None:
        observed_state = state("light.kitchen")
        services = [
            {
                "domain": "light",
                "services": {
                    "turn_on": {
                        "name": "Switch on",
                        "target": {"entity": [{"domain": "light"}]},
                        "fields": {"brightness": {"selector": {"number": {"min": 0, "max": 255}}}},
                    }
                },
            },
            {"domain": "sensor", "services": {}, "provider_extra": ["preserved"]},
        ]

        async def handler(request: Request, writer: asyncio.StreamWriter) -> None:
            result = services if request.target.endswith("/services") else observed_state
            await respond(writer, result)

        async with Server(handler) as server:
            async with prepare_httpx_client(httpx.AsyncClient(
                base_url=server.url + "proxy/",
                headers={"Authorization": "Bearer selected"},
                params={"return_response": "should-not-leak", "unexpected": "default"},
                trust_env=False,
            )) as transport:
                operations = HomeAssistantOperations(transport)
                # Changing the borrowed client's default URL cannot retarget an existing selection.
                transport.base_url = "http://unused.invalid/"
                self.assertEqual(await operations.get_state("light.kitchen"), observed_state)
                self.assertEqual(await operations.list_services(), services)
                self.assertFalse(transport.is_closed)
            self.assertEqual(
                [r.target for r in server.requests],
                [
                    "/proxy/api/states/light.kitchen",
                    "/proxy/api/services",
                ],
            )
            self.assertTrue(all(r.method == "GET" for r in server.requests))
            self.assertTrue(
                all(r.headers["authorization"] == "Bearer selected" for r in server.requests)
            )
            self.assertTrue(
                all(r.headers["accept-encoding"] == "identity" for r in server.requests)
            )
        self.assertTrue(server.joined)

    async def test_both_service_receipt_profiles_preserve_exact_json(self) -> None:
        changes = [state("light.second"), state("light.first", "off")]
        provider_response = {
            "weather.home": {"forecast": [{"temperature": 4.5, "condition": None}]}
        }
        bodies: list[Any] = []

        async def handler(request: Request, writer: asyncio.StreamWriter) -> None:
            bodies.append(json.loads(request.body))
            value = (
                {"changed_states": changes, "service_response": provider_response}
                if "?" in request.target
                else changes
            )
            await respond(writer, value)

        data = {
            "entity_id": ["light.first", "light.second"],
            "transition": 1.25,
            "extra": {"keep": True},
        }
        async with Server(handler) as server:
            async with client(server) as transport:
                operations = HomeAssistantOperations(transport)
                ordinary = await operations.call_service("light", "turn_on", data=data)
                response = await operations.call_service(
                    "weather", "get_forecasts", data=data, return_response=True
                )
                self.assertEqual(ordinary.changed_states, changes)
                self.assertFalse(ordinary.response_requested)
                self.assertIsNone(ordinary.service_response)
                self.assertEqual(response.changed_states, changes)
                self.assertTrue(response.response_requested)
                self.assertEqual(response.service_response, provider_response)
            self.assertEqual(bodies, [data, data])
            self.assertEqual(
                [r.target for r in server.requests],
                [
                    "/api/services/light/turn_on",
                    "/api/services/weather/get_forecasts?return_response",
                ],
            )
            self.assertEqual([r.method for r in server.requests], ["POST", "POST"])
        self.assertTrue(server.joined)

    async def test_requested_null_response_differs_from_absent_response(self) -> None:
        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, {"changed_states": [], "service_response": None})

        async with Server(handler) as server:
            async with client(server) as transport:
                receipt = await HomeAssistantOperations(transport).call_service(
                    "test", "respond", return_response=True
                )
                self.assertTrue(receipt.response_requested)
                self.assertIsNone(receipt.service_response)
                self.assertEqual(receipt.changed_states, [])

    async def test_two_accounts_remain_separately_selected(self) -> None:
        async def a(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, [state("light.same", "a")])

        async def b(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, [state("light.same", "b")])

        async with Server(a) as server_a, Server(b) as server_b:
            async with (
                client(server_a, "account-a") as transport_a,
                client(server_b, "account-b") as transport_b,
            ):
                result_a, result_b = await asyncio.gather(
                    HomeAssistantOperations(transport_a).call_service("light", "turn_on"),
                    HomeAssistantOperations(transport_b).call_service("light", "turn_on"),
                )
                self.assertEqual(result_a.changed_states[0]["state"], "a")
                self.assertEqual(result_b.changed_states[0]["state"], "b")
            self.assertEqual(
                [r.headers["authorization"] for r in server_a.requests], ["Bearer account-a"]
            )
            self.assertEqual(
                [r.headers["authorization"] for r in server_b.requests], ["Bearer account-b"]
            )
        self.assertTrue(server_a.joined and server_b.joined)

    async def test_invalid_targets_and_json_refuse_before_any_request(self) -> None:
        async def unexpected(_request: Request, _writer: asyncio.StreamWriter) -> None:
            raise AssertionError("invalid local input reached the provider")

        async with Server(unexpected) as server:
            async with client(server) as transport:
                operations = HomeAssistantOperations(transport)
                for entity in [
                    "../services",
                    "light.a?target=b",
                    "light.a/b",
                    "light.A",
                    "http://elsewhere",
                ]:
                    with self.subTest(entity=entity), self.assertRaises(ValueError):
                        await operations.get_state(entity)
                for domain, service, data, response in [
                    ("../api", "turn_on", {}, False),
                    ("light", "turn_on?return_response", {}, False),
                    ("light", "turn_on", {"bad": float("nan")}, False),
                    ("light", "turn_on", {"bad": object()}, False),
                    ("light", "turn_on", {"nested": {1: "not a JSON key"}}, False),
                    ("light", "turn_on", {}, "yes"),
                ]:
                    with (
                        self.subTest(domain=domain, service=service),
                        self.assertRaises(ValueError),
                    ):
                        await operations.call_service(
                            domain, service, data=data, return_response=response
                        )
                self.assertEqual(server.requests, [])

    async def test_constructor_refuses_ambiguous_base_and_invalid_limits(self) -> None:
        async def unexpected(_request: Request, _writer: asyncio.StreamWriter) -> None:
            raise AssertionError("constructor validation must not enter transport")

        async with Server(unexpected) as server:
            for url in [
                server.url + "?selected=other",
                server.url + "#fragment",
                server.url.replace("http://", "http://fixture-user:fixture-password@"),
            ]:
                async with prepare_httpx_client(httpx.AsyncClient(base_url=url, trust_env=False)) as transport:
                    with self.subTest(url=url), self.assertRaises(ValueError) as caught:
                        HomeAssistantOperations(transport)
                    self.assertNotIn("fixture-password", str(caught.exception))
            async with client(server) as transport:
                for options in [
                    {"timeout_seconds": 0},
                    {"timeout_seconds": float("inf")},
                    {"timeout_seconds": True},
                    {"max_response_bytes": 0},
                    {"max_response_bytes": True},
                ]:
                    with self.subTest(options=options), self.assertRaises(ValueError):
                        HomeAssistantOperations(transport, **options)
            self.assertEqual(server.requests, [])

    async def test_redirect_is_not_followed_even_when_client_default_allows_it(self) -> None:
        async def sink(_request: Request, _writer: asyncio.StreamWriter) -> None:
            raise AssertionError("redirect target must receive no request or credential")

        async with Server(sink) as second:

            async def redirect(_request: Request, writer: asyncio.StreamWriter) -> None:
                await respond(
                    writer,
                    {"message": "redirect"},
                    status=307,
                    headers={"Location": second.url + "api/services/light/turn_on"},
                )

            async with Server(redirect) as first:
                async with client(first, follow_redirects=True) as transport:
                    with self.assertRaises(HomeAssistantError) as caught:
                        await HomeAssistantOperations(transport).call_service("light", "turn_on")
                    self.assertEqual(caught.exception.kind, FailureKind.REDIRECT)
                    self.assertEqual(caught.exception.status_code, 307)
                    self.assertEqual(caught.exception.write_outcome, WriteOutcome.UNKNOWN)
                self.assertEqual(len(first.requests), 1)
                self.assertEqual(second.requests, [])

    async def test_http_errors_keep_status_and_uncertainty_without_body_or_secret(self) -> None:
        secret = "fixture-sensitive-provider-body-and-token"
        expected = {
            400: FailureKind.HTTP_STATUS,
            401: FailureKind.AUTHENTICATION,
            403: FailureKind.PERMISSION,
            404: FailureKind.NOT_FOUND,
            429: FailureKind.RATE_LIMITED,
            500: FailureKind.HTTP_STATUS,
        }
        entered: list[int] = []
        replies = iter(expected)

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            status = next(replies)
            # An entered handler can have effects before a 400/401/500 response.
            entered.append(status)
            await respond(writer, {"message": secret}, status=status, headers={"Retry-After": "17"})

        async with Server(handler) as server:
            async with client(server, secret) as transport:
                operations = HomeAssistantOperations(transport)
                for status, kind in expected.items():
                    with (
                        self.subTest(status=status),
                        self.assertRaises(HomeAssistantError) as caught,
                    ):
                        await operations.call_service("light", "turn_on")
                    error = caught.exception
                    self.assertEqual(
                        (error.kind, error.status_code, error.write_outcome),
                        (kind, status, WriteOutcome.UNKNOWN),
                    )
                    self.assertEqual(error.retry_after_seconds, 17)
                    self.assertNotIn(secret, str(error))
                    self.assertNotIn(secret, repr(error))
                    self.assertNotIn(secret, "".join(traceback.format_exception(error)))
                    self.assertIsNone(error.__context__)
                    self.assertIsNone(error.__cause__)
            self.assertEqual(entered, list(expected))
            self.assertEqual(len(server.requests), len(expected))

    async def test_read_refusal_has_no_write_outcome(self) -> None:
        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, {"message": "missing"}, status=404)

        async with Server(handler) as server:
            async with client(server) as transport:
                with self.assertRaises(HomeAssistantError) as caught:
                    await HomeAssistantOperations(transport).get_state("light.missing")
                self.assertEqual(caught.exception.kind, FailureKind.NOT_FOUND)
                self.assertEqual(caught.exception.write_outcome, WriteOutcome.NOT_APPLICABLE)

    async def test_malformed_and_wrong_receipts_refuse_without_partial_data(self) -> None:
        replies = iter(
            [
                b'{"secret":"bad-json"',
                b'{"changed_states":[],"changed_states":[]}',
                b"[NaN]",
                b'[{"entity_id":"light.a","state":"on","attributes":null}]',
                b'{"changed_states":[],"service_response":["wrong-profile"]}',
                b'{"changed_states":[]}',
            ]
        )

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, None, raw=next(replies))

        async with Server(handler) as server:
            async with client(server) as transport:
                operations = HomeAssistantOperations(transport)
                for requested in [False, True, False, False, True, True]:
                    with (
                        self.subTest(return_response=requested),
                        self.assertRaises(HomeAssistantError) as caught,
                    ):
                        await operations.call_service("light", "turn_on", return_response=requested)
                    self.assertEqual(caught.exception.kind, FailureKind.INVALID_RESPONSE)
                    self.assertEqual(caught.exception.write_outcome, WriteOutcome.UNKNOWN)
            self.assertEqual(len(server.requests), 6)

    async def test_wrong_entity_and_wrong_service_metadata_are_not_accepted(self) -> None:
        replies = iter([state("light.other"), [{"domain": "light", "services": ["not-an-object"]}]])

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            await respond(writer, next(replies))

        async with Server(handler) as server:
            async with client(server) as transport:
                operations = HomeAssistantOperations(transport)
                with self.assertRaises(HomeAssistantError) as wrong_entity:
                    await operations.get_state("light.selected")
                with self.assertRaises(HomeAssistantError) as wrong_services:
                    await operations.list_services()
                self.assertEqual(wrong_entity.exception.kind, FailureKind.INVALID_RESPONSE)
                self.assertEqual(wrong_services.exception.kind, FailureKind.INVALID_RESPONSE)

    async def test_known_and_chunked_body_limits_and_encoding_are_explicit(self) -> None:
        count = 0

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            nonlocal count
            count += 1
            if count == 1:
                # Refuse from headers even though the announced body never arrives.
                writer.write(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 999999\r\n\r\n"
                )
            elif count == 2:
                writer.write(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n"
                )
                writer.write(b"30\r\n" + b"x" * 48 + b"\r\n0\r\n\r\n")
            else:
                await respond(writer, [], headers={"Content-Encoding": "gzip"})
            await writer.drain()

        async with Server(handler) as server:
            async with client(server) as transport:
                operations = HomeAssistantOperations(transport, max_response_bytes=32)
                for kind in [
                    FailureKind.RESPONSE_TOO_LARGE,
                    FailureKind.RESPONSE_TOO_LARGE,
                    FailureKind.UNSUPPORTED_ENCODING,
                ]:
                    with self.subTest(kind=kind), self.assertRaises(HomeAssistantError) as caught:
                        await operations.call_service("light", "turn_on")
                    self.assertEqual(caught.exception.kind, kind)
                    self.assertEqual(caught.exception.write_outcome, WriteOutcome.UNKNOWN)

    async def test_timeout_does_not_retry_or_claim_remote_service_stopped(self) -> None:
        entered, release, completed = asyncio.Event(), asyncio.Event(), asyncio.Event()

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            entered.set()
            await release.wait()
            completed.set()  # Provider work completed after the client stopped waiting.
            try:
                await respond(writer, [])
            except (BrokenPipeError, ConnectionResetError):
                pass

        async with Server(handler) as server:
            server.releases.append(release)
            async with client(server) as transport:
                with self.assertRaises(HomeAssistantError) as caught:
                    await HomeAssistantOperations(transport, timeout_seconds=0.2).call_service(
                        "light", "turn_on"
                    )
                self.assertTrue(
                    entered.is_set(), "timeout before fixture entry is not this counterexample"
                )
                self.assertEqual(caught.exception.kind, FailureKind.TIMEOUT)
                self.assertEqual(caught.exception.write_outcome, WriteOutcome.UNKNOWN)
                self.assertFalse(completed.is_set())
                release.set()
                await asyncio.wait_for(completed.wait(), 2)
                self.assertFalse(transport.is_closed)
            self.assertEqual(len(server.requests), 1)
        self.assertTrue(server.joined)

    async def test_streaming_chunks_do_not_reset_the_overall_read_deadline(self) -> None:
        entered = asyncio.Event()

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            writer.write(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n"
            )
            entered.set()
            try:
                # Every chunk arrives inside httpx's read timeout; the complete
                # body cannot arrive inside the operation's one second deadline.
                # Leave setup headroom under concurrent compiler load.
                for chunk in [b"[", b" ", b" ", b" ", b" ", b" ", b"]"]:
                    writer.write(b"1\r\n" + chunk + b"\r\n")
                    await writer.drain()
                    await asyncio.sleep(0.25)
                writer.write(b"0\r\n\r\n")
                await writer.drain()
            except (BrokenPipeError, ConnectionResetError):
                pass

        async with Server(handler) as server:
            async with client(server) as transport:
                with self.assertRaises(HomeAssistantError) as caught:
                    await HomeAssistantOperations(transport, timeout_seconds=1.0).call_service(
                        "light", "turn_on"
                    )
                self.assertTrue(entered.is_set())
                self.assertEqual(caught.exception.kind, FailureKind.TIMEOUT)
                self.assertEqual(caught.exception.write_outcome, WriteOutcome.UNKNOWN)
            self.assertEqual(len(server.requests), 1)
        self.assertTrue(server.joined)

    async def test_connection_loss_after_entry_is_unknown_and_not_retried(self) -> None:
        entered = asyncio.Event()

        async def handler(_request: Request, _writer: asyncio.StreamWriter) -> None:
            entered.set()  # Return without an HTTP response: actual TCP EOF.

        async with Server(handler) as server:
            async with client(server) as transport:
                with self.assertRaises(HomeAssistantError) as caught:
                    await HomeAssistantOperations(transport).call_service("light", "turn_on")
                self.assertTrue(entered.is_set())
                self.assertEqual(caught.exception.kind, FailureKind.TRANSPORT)
                self.assertEqual(caught.exception.write_outcome, WriteOutcome.UNKNOWN)
                self.assertIsNone(caught.exception.__context__)
                self.assertIsNone(caught.exception.__cause__)
                self.assertNotIn(server.url, str(caught.exception))
            self.assertEqual(len(server.requests), 1)
        self.assertTrue(server.joined)

    async def test_host_hook_failure_does_not_expose_the_underlying_exception(self) -> None:
        secret = "fixture-secret-in-host-hook-error"

        async def hook(_request: httpx.Request) -> None:
            raise RuntimeError(secret)

        async def unexpected(_request: Request, _writer: asyncio.StreamWriter) -> None:
            raise AssertionError("the failing host hook ran before transport")

        async with Server(unexpected) as server:
            async with client(server, event_hooks={"request": [hook]}) as transport:
                with self.assertRaises(HomeAssistantError) as caught:
                    await HomeAssistantOperations(transport).call_service("light", "turn_on")
                error = caught.exception
                self.assertEqual(error.kind, FailureKind.CLIENT)
                self.assertEqual(error.write_outcome, WriteOutcome.UNKNOWN)
                self.assertNotIn(secret, str(error))
                self.assertNotIn(secret, repr(error))
                self.assertNotIn(secret, "".join(traceback.format_exception(error)))
                self.assertIsNone(error.__context__)
                self.assertIsNone(error.__cause__)
            self.assertEqual(server.requests, [])

    async def test_cancellation_preserves_cancel_and_remote_uncertainty(self) -> None:
        entered, release, completed = asyncio.Event(), asyncio.Event(), asyncio.Event()

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            entered.set()
            await release.wait()
            completed.set()
            try:
                await respond(writer, [])
            except (BrokenPipeError, ConnectionResetError):
                pass

        async with Server(handler) as server:
            server.releases.append(release)
            async with client(server) as transport:
                operation = asyncio.create_task(
                    HomeAssistantOperations(transport).call_service("light", "turn_on")
                )
                try:
                    await asyncio.wait_for(entered.wait(), 2)
                    operation.cancel()
                    with self.assertRaises(ServiceCallCancelled) as caught:
                        await operation
                    self.assertIsInstance(caught.exception, asyncio.CancelledError)
                    self.assertEqual(caught.exception.write_outcome, WriteOutcome.UNKNOWN)
                    self.assertFalse(completed.is_set())
                finally:
                    release.set()
                    if not operation.done():
                        operation.cancel()
                    # Preserve the same operation owner even on prerequisite failure.
                    await asyncio.gather(operation, return_exceptions=True)
                await asyncio.wait_for(completed.wait(), 2)
                self.assertFalse(transport.is_closed)
            self.assertEqual(len(server.requests), 1)
        self.assertTrue(server.joined)

    async def test_fixture_body_failure_still_joins_every_entered_handler(self) -> None:
        entered, release, completed = asyncio.Event(), asyncio.Event(), asyncio.Event()

        async def handler(_request: Request, writer: asyncio.StreamWriter) -> None:
            entered.set()
            await release.wait()
            completed.set()
            try:
                await respond(writer, [])
            except (BrokenPipeError, ConnectionResetError):
                pass

        server = Server(handler)
        operation = None
        with self.assertRaisesRegex(AssertionError, "original fixture body"):
            async with server:
                server.releases.append(release)
                async with client(server) as transport:
                    operation = asyncio.create_task(
                        HomeAssistantOperations(transport).call_service("light", "turn_on")
                    )
                    try:
                        await asyncio.wait_for(entered.wait(), 2)
                        raise AssertionError("original fixture body")
                    finally:
                        release.set()
                        await asyncio.gather(operation, return_exceptions=True)
        self.assertTrue(completed.is_set())
        self.assertTrue(server.joined)
        self.assertTrue(operation is not None and operation.done())


if __name__ == "__main__":
    unittest.main()
