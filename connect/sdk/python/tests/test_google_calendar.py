"""Actual loopback Calendar wire contracts, without OAuth or a Google account."""

from __future__ import annotations

import asyncio
import json
import traceback
from urllib.parse import parse_qs, urlsplit

import anyio
import httpx
from meerkat_toolkit.httpx_client import prepare_httpx_client
import pytest

from meerkat_toolkit.connectors.google_calendar import (
    CalendarError,
    CalendarWriteCancelled,
    FailureKind,
    GoogleCalendarOperations,
    WriteOutcome,
)
from test_home_assistant import Server, client, respond

CANARY = "fixture-private-body-and-token"


def event(event_id="event-one", **values):
    return {
        "id": event_id,
        "etag": '"opaque-etag"',
        "updated": "2026-10-01T10:00:00.123456Z",
        **values,
    }


@pytest.mark.asyncio
async def test_five_operations_exact_wire_and_full_receipts():
    results = [
        {
            "kind": "calendar#events",
            "items": [event()],
            "nextPageToken": "opaque+/=",
            "extra": None,
        },
        event(),
        event("insert-id"),
        event(),
        None,
    ]

    async def handler(request, writer):
        i = len(server.requests) - 1
        await respond(
            writer, results[i], status=204 if i == 4 else 200, raw=b"" if i == 4 else None
        )

    async with Server(handler) as server:
        async with prepare_httpx_client(httpx.AsyncClient(
            base_url=server.url + "proxy/",
            trust_env=False,
            params={"fields": "id", "pageToken": "wrong"},
            headers={"Authorization": "Bearer selected", "If-Match": '"wrong"'},
        )) as transport:
            operations = GoogleCalendarOperations(
                transport, calendar_id="calendar/one@example.test"
            )
            transport.base_url = "http://unused.invalid/"
            assert (
                await operations.list_events_page(
                    time_min="2026-10-01T00:00:00+02:00",
                    time_max="2026-10-02T00:00:00+02:00",
                    q="two words",
                    page_token="opaque+/=",
                    single_events=True,
                    order_by="startTime",
                    max_results=2500,
                    show_deleted=False,
                )
                == results[0]
            )
            assert await operations.get_event("event-one") == results[1]
            body = {
                "id": "insert-id",
                "location": None,
                "description": "",
                "attendees": [],
                "guestsCanModify": False,
                "extendedProperties": {"private": {"x": "y"}},
            }
            assert await operations.insert_event(body) == results[2]
            assert (
                await operations.patch_event("event-one", body, expected_etag='"exact-etag"')
                == results[3]
            )
            assert await operations.delete_event("event-one", expected_etag='"exact-etag"') is None
            assert not transport.is_closed
        requests = server.requests
        assert [r.method for r in requests] == ["GET", "GET", "POST", "PATCH", "DELETE"]
        path = "/proxy/calendar/v3/calendars/calendar%2Fone%40example.test/events"
        assert [urlsplit(r.target).path for r in requests] == [
            path,
            path + "/event-one",
            path,
            path + "/event-one",
            path + "/event-one",
        ]
        assert parse_qs(urlsplit(requests[0].target).query) == {
            "timeMin": ["2026-10-01T00:00:00+02:00"],
            "timeMax": ["2026-10-02T00:00:00+02:00"],
            "q": ["two words"],
            "pageToken": ["opaque+/="],
            "singleEvents": ["true"],
            "orderBy": ["startTime"],
            "maxResults": ["2500"],
            "showDeleted": ["false"],
        }
        assert all(not urlsplit(r.target).query for r in requests[1:])
        assert all(r.headers["authorization"] == "Bearer selected" for r in requests)
        assert all("if-match" not in r.headers for r in requests[:3])
        assert requests[3].headers["if-match"] == requests[4].headers["if-match"] == '"exact-etag"'
        assert json.loads(requests[2].body) == json.loads(requests[3].body) == body
    assert server.joined


@pytest.mark.asyncio
async def test_pagination_is_manual_and_tombstone_fields_are_preserved():
    values = [
        {"kind": "calendar#events", "nextPageToken": "next"},
        {"id": "gone", "status": "cancelled"},
    ]

    async def handler(request, writer):
        await respond(writer, values[len(server.requests) - 1])

    async with Server(handler) as server:
        async with client(server) as transport:
            op = GoogleCalendarOperations(transport, calendar_id="primary")
            assert await op.list_events_page() == values[0]
            assert len(server.requests) == 1
            assert await op.get_event("gone") == values[1]
    assert server.joined


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "status,kind",
    [
        (401, FailureKind.AUTHENTICATION),
        (403, FailureKind.PERMISSION),
        (404, FailureKind.NOT_FOUND),
        (409, FailureKind.CONFLICT),
        (412, FailureKind.PRECONDITION),
        (429, FailureKind.RATE_LIMITED),
        (500, FailureKind.HTTP_STATUS),
        (302, FailureKind.REDIRECT),
    ],
)
async def test_provider_errors_do_not_replay_or_expose_body(status, kind):
    async def handler(request, writer):
        await respond(
            writer,
            {"error": CANARY},
            status=status,
            headers={"Retry-After": "9", "Location": server.url + "credential-leak"},
        )

    async with Server(handler) as server:
        async with client(server, CANARY, follow_redirects=True) as transport:
            with pytest.raises(CalendarError) as caught:
                await GoogleCalendarOperations(transport, calendar_id="primary").patch_event(
                    "id", {}
                )
            exc = caught.value
            assert exc.kind == kind and exc.write_outcome == WriteOutcome.UNKNOWN
            assert exc.status_code == status and exc.retry_after_seconds == 9
            assert CANARY not in "".join(traceback.format_exception(exc))
            assert CANARY not in repr(vars(exc))
            assert exc.__context__ is None and exc.__cause__ is None
        assert len(server.requests) == 1
    assert server.joined


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "payload",
    [
        b"",
        b"{}",
        b"[]",
        b"not json",
        b'{"id":"id","etag":"\\"e\\""}',
        b'{"id":"id","updated":"v"}',
        b'{"id":"wrong","etag":"\\"e\\"","updated":"v"}',
        b'{"id":"id","etag":"\\"e\\"","updated":"v","x":1e999}',
        b'{"id":"id","etag":"\\"e\\"","updated":"v","x":NaN}',
        b'{"id":"id","etag":"\\"e\\"","updated":"v","updated":"other"}',
    ],
)
async def test_incomplete_or_invalid_write_receipt_is_unknown(payload):
    async def handler(request, writer):
        await respond(writer, None, raw=payload)

    async with Server(handler) as server:
        async with client(server) as transport:
            with pytest.raises(CalendarError) as caught:
                await GoogleCalendarOperations(transport, calendar_id="primary").patch_event(
                    "id", {}
                )
            assert caught.value.kind == FailureKind.INVALID_RESPONSE
            assert caught.value.write_outcome == WriteOutcome.UNKNOWN
        assert len(server.requests) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "headers,raw,kind",
    [
        ({"Content-Type": "text/html"}, b"{}", FailureKind.INVALID_RESPONSE),
        ({"Content-Encoding": "gzip"}, b"{}", FailureKind.UNSUPPORTED_ENCODING),
        ({}, b"x" * 101, FailureKind.RESPONSE_TOO_LARGE),
        ({"Content-Length": "bad"}, b"", FailureKind.TRANSPORT),
    ],
)
async def test_bad_bounded_representation(headers, raw, kind):
    async def handler(request, writer):
        await respond(writer, None, headers=headers, raw=raw)

    async with Server(handler) as server:
        async with client(server) as transport:
            with pytest.raises(CalendarError) as caught:
                await GoogleCalendarOperations(
                    transport, calendar_id="primary", max_response_bytes=100
                ).insert_event({})
            assert caught.value.kind == kind
            assert caught.value.write_outcome == WriteOutcome.UNKNOWN


@pytest.mark.asyncio
async def test_committed_write_disconnect_is_unknown_and_never_replaced():
    commits = []

    async def handler(request, writer):
        commits.append(json.loads(request.body))
        writer.close()

    async with Server(handler) as server:
        async with client(server) as transport:
            with pytest.raises(CalendarError) as caught:
                await GoogleCalendarOperations(transport, calendar_id="primary").insert_event(
                    {"id": "stable"}
                )
            assert caught.value.kind == FailureKind.TRANSPORT
            assert caught.value.write_outcome == WriteOutcome.UNKNOWN
        assert commits == [{"id": "stable"}] and len(server.requests) == 1


@pytest.mark.asyncio
async def test_repeat_cancel_joins_response_cleanup_and_preserves_unknown():
    entered = asyncio.Event()
    closing = asyncio.Event()
    release = asyncio.Event()

    class Stream(httpx.AsyncByteStream):
        async def __aiter__(self):
            entered.set()
            await anyio.sleep_forever()
            yield b""

        async def aclose(self):
            closing.set()
            await release.wait()

    async def handler(request):
        return httpx.Response(200, headers={"Content-Type": "application/json"}, stream=Stream())

    async with prepare_httpx_client(httpx.AsyncClient(
        base_url="http://fixture.invalid/", transport=httpx.MockTransport(handler)
    )) as transport:
        task = asyncio.create_task(
            GoogleCalendarOperations(transport, calendar_id="primary").insert_event({})
        )
        await asyncio.wait_for(entered.wait(), 2)
        task.cancel(CANARY)
        await asyncio.wait_for(closing.wait(), 2)
        task.cancel(CANARY)
        await asyncio.sleep(0)
        assert not task.done()
        release.set()
        with pytest.raises(CalendarWriteCancelled) as caught:
            await asyncio.wait_for(task, 2)
        assert caught.value.write_outcome == WriteOutcome.UNKNOWN and CANARY not in str(
            caught.value
        )
        assert not transport.is_closed


@pytest.mark.asyncio
async def test_real_body_deadline_disconnects_and_does_not_replay():
    entered = asyncio.Event()

    async def handler(request, writer):
        entered.set()
        writer.write(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 999\r\n\r\n{"
        )
        await writer.drain()
        # Owned fixture joins after the client deadline; no provider retry.
        await asyncio.sleep(0.1)

    async with Server(handler) as server:
        async with client(server) as transport:
            with pytest.raises(CalendarError) as caught:
                await GoogleCalendarOperations(
                    transport, calendar_id="primary", timeout_seconds=0.02
                ).insert_event({})
            assert caught.value.kind == FailureKind.TIMEOUT
            assert caught.value.write_outcome == WriteOutcome.UNKNOWN
        assert len(server.requests) == 1
    assert server.joined


@pytest.mark.asyncio
async def test_local_invalid_inputs_never_enter_http():
    async def handler(request):
        raise AssertionError("local invalid input reached transport")

    async with prepare_httpx_client(httpx.AsyncClient(
        base_url="https://example.test/", transport=httpx.MockTransport(handler)
    )) as transport:
        op = GoogleCalendarOperations(transport, calendar_id="primary")
        for body in [None, [], {"x": float("inf")}, {1: "value"}, {"x": (1, 2)}]:
            with pytest.raises(ValueError):
                await op.insert_event(body)
        for etag in ["", "*", '"a", "b"', '"x\r\ny"']:
            with pytest.raises(ValueError):
                await op.patch_event("id", {}, expected_etag=etag)
        for ident in [".", "..", "", "bad\nvalue"]:
            with pytest.raises(ValueError):
                await op.get_event(ident)
        with pytest.raises(ValueError):
            await op.list_events_page(max_results=True)
        with pytest.raises(ValueError):
            await op.list_events_page(order_by="startTime", single_events=False)
