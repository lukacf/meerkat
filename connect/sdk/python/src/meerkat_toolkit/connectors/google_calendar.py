"""Explicit Google Calendar Events operations on a borrowed HTTPX client.

The host owns credentials, selected account, authorization and reconciliation.
No OAuth, token lookup, automatic retries, page traversal or calendar policy.
"""

from __future__ import annotations

import asyncio
from enum import Enum
import json
import math
from typing import Any
from urllib.parse import quote

import anyio
import httpx

from ._google_errors import is_rate_limit
from ._httpx_stream import (
    HttpxClientNotPrepared,
    require_prepared_httpx_client,
    send_with_custody,
)


class FailureKind(str, Enum):
    AUTHENTICATION = "authentication"
    PERMISSION = "permission"
    NOT_FOUND = "not_found"
    CONFLICT = "conflict"
    PRECONDITION = "precondition"
    RATE_LIMITED = "rate_limited"
    REDIRECT = "redirect"
    HTTP_STATUS = "http_status"
    INVALID_RESPONSE = "invalid_response"
    RESPONSE_TOO_LARGE = "response_too_large"
    UNSUPPORTED_ENCODING = "unsupported_encoding"
    TIMEOUT = "timeout"
    TRANSPORT = "transport"
    CLIENT = "client"


class WriteOutcome(str, Enum):
    NOT_APPLICABLE = "not_applicable"
    UNKNOWN = "unknown"


class CalendarError(Exception):
    """Redacted metadata, never a response body, credential, URL or host exception.

    All attempted write errors are conservatively unknown, including provider
    refusals. Status codes inform reconciliation, never automatic replay.
    """

    def __init__(
        self,
        kind: FailureKind,
        *,
        write_outcome: WriteOutcome,
        status_code: int | None = None,
        retry_after_seconds: int | None = None,
    ) -> None:
        self.kind = kind
        self.write_outcome = write_outcome
        self.status_code = status_code
        self.retry_after_seconds = retry_after_seconds
        status = "" if status_code is None else f" (HTTP {status_code})"
        super().__init__(
            f"Google Calendar {kind.value}{status}; write outcome: {write_outcome.value}"
        )


class CalendarWriteCancelled(asyncio.CancelledError):
    """Cancellation joins entered HTTP work; it cannot undo a remote mutation."""

    write_outcome = WriteOutcome.UNKNOWN

    def __init__(self) -> None:
        super().__init__("Google Calendar write cancelled; write outcome: unknown")


def _text(value: str, maximum: int = 8192) -> str:
    if not isinstance(value, str) or not value or any(ord(c) < 32 or ord(c) == 127 for c in value):
        raise ValueError("expected nonempty text without control characters")
    try:
        if len(value.encode("utf-8")) <= maximum:
            return value
    except UnicodeError:
        pass
    raise ValueError("text exceeds the supported UTF-8 limit") from None


def _segment(value: str) -> str:
    value = _text(value)
    if value in {".", ".."}:
        raise ValueError("dot path segments are not identifiers")
    return quote(value, safe="")


def _etag(value: str) -> str:
    _text(value)
    if len(value) < 2 or value[0] != '"' or value[-1] != '"' or '"' in value[1:-1]:
        raise ValueError("expected one quoted provider ETag, not a wildcard or list")
    return value


def _object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON member")
        result[key] = value
    return result


def _number(value: str) -> float:
    number = float(value)
    if not math.isfinite(number):
        raise ValueError("non-finite JSON number")
    return number


def _constant(_value: str) -> Any:
    raise ValueError("non-JSON numeric constant")


def _event(value: Any, *, write: bool = False, expected_id: str | None = None) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError("expected an event object")
    _text(value.get("id"))
    if expected_id is not None and value["id"] != expected_id:
        raise ValueError("event identity differs from the requested event")
    # Deleted/cancelled events may contain only id. Keep their status and all
    # other provider fields intact for the host's absence/reconciliation owner.
    if write or value.get("status") != "cancelled":
        _etag(value.get("etag"))
        _text(value.get("updated"))
    return value


def _page(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict) or value.get("kind") != "calendar#events":
        raise ValueError("expected a Calendar events collection")
    # Google may omit an empty items array. Never synthesize it or a terminal cursor.
    if "items" in value:
        if not isinstance(value["items"], list):
            raise ValueError("expected an event array")
        for event in value["items"]:
            _event(event)
    for field in ("nextPageToken", "nextSyncToken"):
        if field in value:
            _text(value[field])
    return value


class GoogleCalendarOperations:
    """One calendar on an explicitly selected API endpoint, with borrowed custody.

    Normally base_url is https://www.googleapis.com/. Otherwise the borrowed
    client's base URL is captured. Optional auth is an explicit host-supplied
    per-request HTTPX auth object; None preserves client auth. This selection
    routes a credential, it does not verify its account. Host auth/hooks/transport
    must not retarget, replay, swallow cancellation or mutate shared credentials.
    The client is never closed here. No partial-response fields are supported.
    """

    def __init__(
        self,
        client: httpx.AsyncClient,
        *,
        calendar_id: str,
        base_url: str | httpx.URL | None = None,
        auth: httpx.Auth | None = None,
        timeout_seconds: float = 20.0,
        max_response_bytes: int = 2_097_152,
        max_request_bytes: int = 2_097_152,
    ) -> None:
        base = client.base_url if base_url is None else httpx.URL(base_url)
        if (
            base.scheme not in {"http", "https"}
            or not base.host
            or base.username
            or base.password
            or base.query
            or base.fragment
        ):
            raise ValueError("select one HTTP(S) base URL without userinfo or query")
        if (
            isinstance(timeout_seconds, bool)
            or not isinstance(timeout_seconds, (int, float))
            or not math.isfinite(timeout_seconds)
            or timeout_seconds <= 0
        ):
            raise ValueError("timeout_seconds must be finite and positive")
        if any(type(n) is not int or n <= 0 for n in (max_response_bytes, max_request_bytes)):
            raise ValueError("byte limits must be positive integers")
        self._path = f"calendar/v3/calendars/{_segment(calendar_id)}/events"
        self._client = client
        self._base = base
        self._auth = auth
        self._timeout = float(timeout_seconds)
        self._max_bytes = max_response_bytes
        self._max_request_bytes = max_request_bytes

    def _failure(
        self, kind: FailureKind, *, write: bool, response: httpx.Response | None = None
    ) -> CalendarError:
        retry_after = None
        if response is not None:
            value = response.headers.get("retry-after", "")
            if value.isascii() and value.isdecimal() and len(value) <= 9:
                retry_after = int(value)
        return CalendarError(
            kind,
            write_outcome=WriteOutcome.UNKNOWN if write else WriteOutcome.NOT_APPLICABLE,
            status_code=None if response is None else response.status_code,
            retry_after_seconds=retry_after,
        )

    def _body(self, value: dict[str, Any]) -> bytes:
        if not isinstance(value, dict):
            raise ValueError("expected an event body object")
        try:
            # Reject Python values whose serialization silently rewrites keys or
            # values. A JSON roundtrip freezes the call body before any await.
            def validate(item: Any) -> None:
                if isinstance(item, dict):
                    if any(not isinstance(key, str) for key in item):
                        raise ValueError("expected string JSON keys")
                    for child in item.values():
                        validate(child)
                elif isinstance(item, list):
                    for child in item:
                        validate(child)
                elif item is not None and type(item) not in (str, int, float, bool):
                    raise ValueError("expected JSON values")

            validate(value)
            encoded = json.dumps(value, allow_nan=False, separators=(",", ":")).encode("utf-8")
        except (ValueError, TypeError, UnicodeError, RecursionError):
            raise ValueError("expected a finite JSON event body") from None
        if len(encoded) > self._max_request_bytes:
            raise ValueError("event body exceeds the request limit")
        return encoded

    async def _call(
        self,
        method: str,
        *,
        event_id: str | None = None,
        params: list[tuple[str, str]] | None = None,
        body: dict[str, Any] | None = None,
        expected_etag: str | None = None,
    ) -> Any:
        require_prepared_httpx_client(self._client)
        if method in {"POST", "PATCH"} and not isinstance(body, dict):
            raise ValueError("expected an event body object")
        path = self._path + ("" if event_id is None else f"/{_segment(event_id)}")
        content = None if body is None else self._body(body)
        etag = None if expected_etag is None else _etag(expected_etag)
        expected_id = event_id
        if method == "POST" and body is not None and "id" in body:
            expected_id = _text(body["id"])
        url = self._base.join(path)
        if params:
            url = url.copy_with(query=str(httpx.QueryParams(tuple(params))).encode("ascii"))
        write = method != "GET"
        scope = anyio.CancelScope(deadline=anyio.current_time() + self._timeout)
        owner = asyncio.create_task(self._request(method, url, content, etag, expected_id, scope))
        cancelled = False
        while not owner.done():
            try:
                await asyncio.shield(owner)
            except asyncio.CancelledError:
                cancelled = True
                scope.cancel()
            except Exception:
                break
        if cancelled:
            try:
                owner.result()
            except (Exception, asyncio.CancelledError):
                pass
            raise (CalendarWriteCancelled() if write else asyncio.CancelledError()) from None
        return owner.result()

    async def _request(
        self,
        method: str,
        url: httpx.URL,
        content: bytes | None,
        etag: str | None,
        expected_id: str | None,
        scope: anyio.CancelScope,
    ) -> Any:
        write = method != "GET"
        failure: BaseException | None = None
        response: httpx.Response | None = None
        payload: Any = None
        try:
            with scope:
                request = self._client.build_request(
                    method,
                    url,
                    content=content,
                    headers={
                        "Accept": "application/json",
                        "Accept-Encoding": "identity",
                        **({"Content-Type": "application/json"} if content is not None else {}),
                    },
                    timeout=self._timeout,
                )
                request.url = url  # Drop inherited query defaults, including fields.
                for header in ("If-Match", "If-None-Match", "X-HTTP-Method-Override"):
                    request.headers.pop(header, None)
                if etag is not None:
                    request.headers["If-Match"] = etag
                response = await send_with_custody(self._client, request, auth=self._auth)
                try:
                    expected_status = 204 if method == "DELETE" else 200
                    if response.status_code != expected_status:
                        kind = {
                            401: FailureKind.AUTHENTICATION,
                            403: FailureKind.PERMISSION,
                            404: FailureKind.NOT_FOUND,
                            409: FailureKind.CONFLICT,
                            412: FailureKind.PRECONDITION,
                            429: FailureKind.RATE_LIMITED,
                        }.get(response.status_code, FailureKind.HTTP_STATUS)
                        if 300 <= response.status_code < 400:
                            kind = FailureKind.REDIRECT
                        if response.status_code == 403 and await is_rate_limit(response, self._max_bytes):
                            kind = FailureKind.RATE_LIMITED
                        failure = self._failure(kind, write=write, response=response)
                    elif response.headers.get("content-encoding", "identity").lower() != "identity":
                        failure = self._failure(
                            FailureKind.UNSUPPORTED_ENCODING, write=write, response=response
                        )
                    else:
                        size = response.headers.get("content-length")
                        if size is not None and (not size.isascii() or not size.isdecimal()):
                            failure = self._failure(
                                FailureKind.INVALID_RESPONSE, write=write, response=response
                            )
                        elif size is not None and (len(size) > 12 or int(size) > self._max_bytes):
                            failure = self._failure(
                                FailureKind.RESPONSE_TOO_LARGE, write=write, response=response
                            )
                        elif method == "DELETE" and size not in (None, "0"):
                            failure = self._failure(
                                FailureKind.INVALID_RESPONSE, write=write, response=response
                            )
                        else:
                            data = bytearray()
                            # MockTransport may have already consumed a response.
                            chunks = (
                                response.aiter_bytes()
                                if response.is_stream_consumed
                                else response.aiter_raw()
                            )
                            async for chunk in chunks:
                                if len(data) + len(chunk) > self._max_bytes:
                                    failure = self._failure(
                                        FailureKind.RESPONSE_TOO_LARGE,
                                        write=write,
                                        response=response,
                                    )
                                    break
                                data.extend(chunk)
                            if failure is None:
                                try:
                                    if size is not None and len(data) != int(size):
                                        raise ValueError("response length does not match its header")
                                    if method == "DELETE":
                                        if data:
                                            raise ValueError("expected an empty deletion response")
                                    else:
                                        if (
                                            response.headers.get("content-type", "")
                                            .split(";", 1)[0]
                                            .strip()
                                            .lower()
                                            != "application/json"
                                        ):
                                            raise ValueError("expected JSON media type")
                                        value = json.loads(
                                            data.decode("utf-8"),
                                            object_pairs_hook=_object,
                                            parse_float=_number,
                                            parse_constant=_constant,
                                        )
                                        payload = (
                                            _page(value)
                                            if method == "GET" and expected_id is None
                                            else _event(value, write=write, expected_id=expected_id)
                                        )
                                except (ValueError, TypeError, RecursionError):
                                    failure = self._failure(
                                        FailureKind.INVALID_RESPONSE, write=write, response=response
                                    )
                finally:
                    with anyio.CancelScope(shield=True):
                        await response.aclose()
            if scope.cancel_called:
                failure = self._failure(FailureKind.TIMEOUT, write=write)
        except HttpxClientNotPrepared:
            failure = (
                HttpxClientNotPrepared()
                if response is None
                else self._failure(FailureKind.CLIENT, write=write)
            )
        except asyncio.CancelledError:
            failure = CalendarWriteCancelled() if write else asyncio.CancelledError()
        except (TimeoutError, httpx.TimeoutException):
            failure = self._failure(FailureKind.TIMEOUT, write=write)
        except httpx.HTTPError:
            failure = self._failure(FailureKind.TRANSPORT, write=write)
        except Exception:
            failure = self._failure(FailureKind.CLIENT, write=write)
        if failure is not None:
            raise failure from None
        return payload

    async def list_events_page(
        self,
        *,
        time_min: str | None = None,
        time_max: str | None = None,
        q: str | None = None,
        page_token: str | None = None,
        max_results: int | None = None,
        single_events: bool | None = None,
        order_by: str | None = None,
        show_deleted: bool | None = None,
    ) -> dict[str, Any]:
        """Return one untouched page. The caller owns time ranges and traversal."""
        params: list[tuple[str, str]] = []
        for name, value in (
            ("timeMin", time_min),
            ("timeMax", time_max),
            ("q", q),
            ("pageToken", page_token),
        ):
            if value is not None:
                params.append((name, _text(value)))
        if max_results is not None:
            if type(max_results) is not int or not 1 <= max_results <= 2500:
                raise ValueError("max_results must be an integer from 1 to 2500")
            params.append(("maxResults", str(max_results)))
        for name, enabled in (("singleEvents", single_events), ("showDeleted", show_deleted)):
            if enabled is not None:
                if type(enabled) is not bool:
                    raise ValueError("event flags must be booleans")
                params.append((name, str(enabled).lower()))
        if order_by is not None:
            if order_by not in {"startTime", "updated"}:
                raise ValueError("unsupported order_by")
            if order_by == "startTime" and single_events is not True:
                raise ValueError("startTime ordering requires single_events=True")
            params.append(("orderBy", order_by))
        return await self._call("GET", params=params)

    async def get_event(self, event_id: str) -> dict[str, Any]:
        return await self._call("GET", event_id=event_id)

    async def insert_event(self, event: dict[str, Any]) -> dict[str, Any]:
        return await self._call("POST", body=event)

    async def patch_event(
        self, event_id: str, patch: dict[str, Any], *, expected_etag: str | None = None
    ) -> dict[str, Any]:
        return await self._call("PATCH", event_id=event_id, body=patch, expected_etag=expected_etag)

    async def delete_event(self, event_id: str, *, expected_etag: str | None = None) -> None:
        return await self._call("DELETE", event_id=event_id, expected_etag=expected_etag)
