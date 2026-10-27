"""Explicit Slack read operations on a borrowed, host-authenticated client.

No credential lookup, OAuth, implicit discovery or retries. Explicit interval
retrieval is bounded; returned observations are not proof of workspace coverage.
"""

from __future__ import annotations

import asyncio
from collections import deque
from copy import deepcopy
from enum import Enum
import json
import math
import re
from typing import Any, Literal, NotRequired, TypedDict, cast

import anyio
import httpx

from ._httpx_stream import (
    HttpxClientNotPrepared,
    require_prepared_httpx_client,
    send_with_custody,
)


class FailureKind(str, Enum):
    AUTHENTICATION = "authentication"
    PERMISSION = "permission"
    NOT_FOUND = "not_found"
    RATE_LIMITED = "rate_limited"
    PROVIDER = "provider"
    REDIRECT = "redirect"
    HTTP_STATUS = "http_status"
    INVALID_RESPONSE = "invalid_response"
    RESPONSE_TOO_LARGE = "response_too_large"
    UNSUPPORTED_ENCODING = "unsupported_encoding"
    TIMEOUT = "timeout"
    TRANSPORT = "transport"
    CLIENT = "client"


class SlackError(Exception):
    """Safe metadata only. Retry-After is information, not an automatic retry.

    Unknown provider codes are omitted rather than reflected into diagnostics.
    No provider body, message, cursor, URL, token or host exception is retained.
    """

    def __init__(
        self,
        kind: FailureKind,
        *,
        status_code: int | None = None,
        provider_code: str | None = None,
        retry_after_seconds: int | None = None,
    ) -> None:
        self.kind = kind
        self.status_code = status_code
        self.provider_code = provider_code
        self.retry_after_seconds = retry_after_seconds
        status = "" if status_code is None else f" (HTTP {status_code})"
        super().__init__(f"Slack {kind.value}{status}")


SlackIntervalSource = Literal["history", "replies"]
SlackIntervalState = Literal["pending", "exhausted", "stopped", "not_requested"]
SlackIntervalStop = Literal[
    "cursor_cycle", "missing_cursor", "unusable_cursor", "message_budget",
    "thread_not_found", "request_failure", "continuation_budget", "result_budget",
]
SlackIntervalTermination = Literal[
    "finished", "request_budget", "message_budget", "request_failure", "continuation_budget", "result_budget",
]
SlackIntervalGapKind = Literal[
    "older_roots_unsearched", "history_outside_discovery", "invalid_thread_reference",
    "invalid_broadcast_reference", "reply_chain_mismatch", "pagination_contradiction",
    "missing_cursor", "unusable_cursor", "cursor_cycle", "retention_limited",
    "invalid_coverage_metadata", "thread_not_found", "request_failure",
    "request_budget", "message_budget", "continuation_budget", "result_budget",
]


class SlackIntervalError(TypedDict):
    kind: str
    status_code: int | None
    provider_code: str | None
    retry_after_seconds: int | None


class SlackIntervalChain(TypedDict):
    state: SlackIntervalState
    pages: int
    stop_reason: SlackIntervalStop | None
    next_cursor: NotRequired[str | None]
    has_more: NotRequired[bool]
    error: NotRequired[SlackIntervalError]


class SlackIntervalThread(SlackIntervalChain):
    root_ts: str


class SlackIntervalGap(TypedDict):
    kind: SlackIntervalGapKind
    source: SlackIntervalSource
    root_ts: NotRequired[str]
    message_ts: NotRequired[str]


class SlackIntervalMessage(TypedDict):
    source: SlackIntervalSource
    root_ts: str | None
    message: dict[str, Any]


class SlackIntervalLimits(TypedDict):
    max_requests: int
    max_messages: int
    page_limit: int
    max_response_bytes: int
    max_result_bytes: int


class SlackIntervalSelection(TypedDict):
    channel: str
    start: str
    end: str
    root_oldest: str
    page_limit: int
    include_all_metadata: bool


class SlackIntervalPending(TypedDict):
    root_ts: str | None
    cursor: str | None
    seen_cursors: list[str]


class SlackIntervalContinuation(TypedDict):
    schema_version: Literal[1]
    selection: SlackIntervalSelection
    pending: list[SlackIntervalPending]
    discovered_roots: list[str]


class SlackIntervalResult(TypedDict):
    channel: str
    start: str
    end: str
    root_oldest: str
    limits: SlackIntervalLimits
    messages: list[SlackIntervalMessage]
    history: SlackIntervalChain
    threads: list[SlackIntervalThread]
    requests_made: int
    skipped_non_threadable: int
    termination: SlackIntervalTermination
    coverage_gaps: list[SlackIntervalGap]
    observation_scope: NotRequired[Literal["current_call_only"]]
    continuation: NotRequired[SlackIntervalContinuation | None]


_CHANNEL = re.compile(r"[A-Za-z0-9]{1,128}\Z")
_TIMESTAMP = re.compile(r"[0-9]+(?:\.[0-9]+)?\Z")
_PROVIDER_ERRORS = {
    **dict.fromkeys(
        ("not_authed", "invalid_auth", "token_expired", "token_revoked", "account_inactive"),
        FailureKind.AUTHENTICATION,
    ),
    **dict.fromkeys(
        (
            "missing_scope",
            "no_permission",
            "access_denied",
            "accesslimited",
            "not_allowed_token_type",
            "team_access_not_granted",
            "not_in_channel",
        ),
        FailureKind.PERMISSION,
    ),
    "channel_not_found": FailureKind.NOT_FOUND,
    "message_not_found": FailureKind.NOT_FOUND,
    "thread_not_found": FailureKind.NOT_FOUND,
    "user_not_found": FailureKind.NOT_FOUND,
    "ratelimited": FailureKind.RATE_LIMITED,
    "invalid_cursor": FailureKind.PROVIDER,
    "invalid_arguments": FailureKind.PROVIDER,
    "invalid_ts_latest": FailureKind.PROVIDER,
    "invalid_ts_oldest": FailureKind.PROVIDER,
}


def _timestamp(value: str) -> str:
    # A bounded decimal string profile, never a float or a rewritten timestamp.
    if not isinstance(value, str) or len(value) > 64 or not _TIMESTAMP.fullmatch(value):
        raise ValueError("expected a decimal timestamp string of at most 64 characters")
    return value


def _cursor(value: str) -> str:
    if (
        not isinstance(value, str)
        or not value
        or any(ord(char) < 32 or ord(char) == 127 for char in value)
    ):
        raise ValueError("expected a nonempty cursor without control characters")
    try:
        if len(value.encode("utf-8")) <= 8192:
            return value
    except UnicodeError:
        pass
    raise ValueError("cursor exceeds the supported UTF-8 limit") from None


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


def _envelope(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict) or type(value.get("ok")) is not bool:
        raise ValueError("expected a Slack response object")
    if value["ok"] is False:
        if not isinstance(value.get("error"), str) or not value["error"]:
            raise ValueError("expected a Slack error code")
    return cast(dict[str, Any], value)


def _page(value: Any) -> dict[str, Any]:
    value = _envelope(value)
    if value["ok"] is False:
        return value
    if not isinstance(value.get("messages"), list):
        raise ValueError("expected a Slack message array")
    for message in value["messages"]:
        if not isinstance(message, dict):
            raise ValueError("expected a Slack message object")
        timestamp = message.get("ts")
        if not isinstance(timestamp, str):
            raise ValueError("expected a timestamp string")
        _timestamp(timestamp)
    if "has_more" in value and type(value["has_more"]) is not bool:
        raise ValueError("expected a boolean has_more")
    if "response_metadata" in value:
        metadata = value["response_metadata"]
        if not isinstance(metadata, dict):
            raise ValueError("expected a metadata object")
        cursor = metadata.get("next_cursor")
        # Slack documents null, empty and absent terminal cursors. Preserve all
        # three rather than manufacturing a single completion representation.
        if cursor is not None and not isinstance(cursor, str):
            raise ValueError("expected a string or null next_cursor")
    return cast(dict[str, Any], value)


def _identifier(value: Any) -> str:
    if not isinstance(value, str) or not _CHANNEL.fullmatch(value):
        raise ValueError("expected a bounded alphanumeric Slack identifier")
    return value


def _discovery_response(
    value: Any, method: str, channel: str | None, user: str | None = None
) -> dict[str, Any]:
    value = _envelope(value)
    if value["ok"] is False:
        return value
    if method == "auth.test":
        _identifier(value.get("team_id"))
        _identifier(value.get("user_id"))
    elif method == "users.info":
        row = value.get("user")
        if not isinstance(row, dict) or _identifier(row.get("id")) != user:
            raise ValueError("expected the exact selected user")
    elif method == "conversations.info":
        row = value.get("channel")
        if not isinstance(row, dict) or _identifier(row.get("id")) != channel:
            raise ValueError("expected the exact selected conversation")
    elif method == "chat.getPermalink":
        if _identifier(value.get("channel")) != channel:
            raise ValueError("expected the exact selected conversation")
        link = value.get("permalink")
        if (not isinstance(link, str) or not link or len(link.encode("utf-8")) > 8192
                or any(ord(char) <= 32 or ord(char) == 127 for char in link)):
            raise ValueError("expected a bounded permalink without whitespace or controls")
        url = httpx.URL(link)
        if url.scheme != "https" or not url.host or url.userinfo:
            raise ValueError("expected an absolute HTTPS permalink without credentials")
    else:
        rows = value.get("channels")
        if not isinstance(rows, list):
            raise ValueError("expected a conversation array")
        for row in rows:
            if not isinstance(row, dict):
                raise ValueError("expected a conversation object")
            _identifier(row.get("id"))
        if "response_metadata" in value:
            metadata = value["response_metadata"]
            if not isinstance(metadata, dict):
                raise ValueError("expected a metadata object")
            cursor = metadata.get("next_cursor")
            if cursor is not None and not isinstance(cursor, str):
                raise ValueError("expected a string or null next_cursor")
    return value


def _decimal_compare(left: str, right: str) -> int:
    """Compare validated nonnegative decimals without altering provider strings."""
    left_whole, _, left_fraction = left.partition(".")
    right_whole, _, right_fraction = right.partition(".")
    left_whole = left_whole.lstrip("0") or "0"
    right_whole = right_whole.lstrip("0") or "0"
    if len(left_whole) != len(right_whole):
        return 1 if len(left_whole) > len(right_whole) else -1
    if left_whole != right_whole:
        return 1 if left_whole > right_whole else -1
    width = max(len(left_fraction), len(right_fraction))
    left_fraction = left_fraction.ljust(width, "0")
    right_fraction = right_fraction.ljust(width, "0")
    return (left_fraction > right_fraction) - (left_fraction < right_fraction)


def _interval_reference(
    message: dict[str, Any],
) -> tuple[str | None, SlackIntervalGapKind | None]:
    root = message["ts"]
    if "thread_ts" in message:
        candidate = message["thread_ts"]
        try:
            _timestamp(candidate)
        except ValueError:
            return None, "invalid_thread_reference"
        if _decimal_compare(candidate, root) > 0:
            return None, "invalid_thread_reference"
        root = candidate
    if message.get("subtype") in ("thread_broadcast", "reply_broadcast"):
        if "thread_ts" not in message:
            return None, "invalid_broadcast_reference"
        if "root" in message:
            nested = message["root"]
            if not isinstance(nested, dict) or nested.get("ts") != root:
                return None, "invalid_broadcast_reference"
    return root, None


# Checkpoints are caller-held query data, not evidence or account authority.
_INTERVAL_ROOT_LIMIT = 1024
_INTERVAL_PENDING_LIMIT = 1025
_INTERVAL_SEEN_LIMIT = 2048
DEFAULT_INTERVAL_RESULT_BYTES = 262_144
MIN_INTERVAL_RESULT_BYTES = 4096
MAX_INTERVAL_RESULT_BYTES = 16_777_216
_INTERVAL_BYTE_LIMIT = 262144


def _interval_fits(value: SlackIntervalContinuation) -> bool:
    if (len(value["discovered_roots"]) > _INTERVAL_ROOT_LIMIT
            or len(value["pending"]) > _INTERVAL_PENDING_LIMIT
            or sum(len(step["seen_cursors"]) for step in value["pending"]) > _INTERVAL_SEEN_LIMIT):
        return False
    try:
        return len(json.dumps(value, ensure_ascii=False, separators=(",", ":"),
                              allow_nan=False).encode("utf-8")) <= _INTERVAL_BYTE_LIMIT
    except (ValueError, UnicodeError):
        return False


def _interval_resume(value: Any, selection: SlackIntervalSelection) -> SlackIntervalContinuation:
    """Validate and copy bounded primitives before any provider request or await."""
    def exact(item: Any, keys: tuple[str, ...]) -> None:
        if type(item) is not dict or set(item) != set(keys):
            raise ValueError("invalid interval continuation shape")

    exact(value, ("schema_version", "selection", "pending", "discovered_roots"))
    if type(value["schema_version"]) is not int or value["schema_version"] != 1:
        raise ValueError("unsupported interval continuation version")
    supplied = value["selection"]
    exact(supplied, tuple(selection))
    if any(type(supplied[key]) is not type(expected) or supplied[key] != expected
           for key, expected in selection.items()):
        raise ValueError("interval continuation selection mismatch")
    raw_roots, raw_pending = value["discovered_roots"], value["pending"]
    if (type(raw_roots) is not list or len(raw_roots) > _INTERVAL_ROOT_LIMIT
            or type(raw_pending) is not list or not 0 < len(raw_pending) <= _INTERVAL_PENDING_LIMIT):
        raise ValueError("interval continuation exceeds structural limits")
    roots: list[str] = []
    for root in raw_roots:
        if type(root) is not str:
            raise ValueError("invalid interval root")
        _timestamp(root)
        if root in roots or _decimal_compare(root, selection["end"]) >= 0:
            raise ValueError("invalid or repeated interval root")
        roots.append(root)
    pending: list[SlackIntervalPending] = []
    queued: set[str | None] = set()
    count = 0
    for step in raw_pending:
        exact(step, ("root_ts", "cursor", "seen_cursors"))
        root, cursor, raw_seen = step["root_ts"], step["cursor"], step["seen_cursors"]
        if root is not None and (type(root) is not str or root not in roots):
            raise ValueError("unknown pending interval root")
        if root in queued:
            raise ValueError("repeated pending interval root")
        queued.add(root)
        if type(raw_seen) is not list or len(raw_seen) > _INTERVAL_SEEN_LIMIT - count:
            raise ValueError("too many interval cursors")
        seen: list[str] = []
        for item in raw_seen:
            if type(item) is not str or item in seen:
                raise ValueError("invalid or repeated interval cursor")
            seen.append(_cursor(item))
        count += len(seen)
        if cursor is None:
            if seen:
                raise ValueError("initial interval cursor has history")
        elif type(cursor) is not str or not seen or cursor != seen[-1]:
            raise ValueError("pending interval cursor does not match history")
        pending.append({"root_ts": root, "cursor": cursor, "seen_cursors": seen})
    result: SlackIntervalContinuation = {
        "schema_version": 1, "selection": dict(selection),
        "pending": pending, "discovered_roots": roots,
    }
    if not _interval_fits(result):
        raise ValueError("interval continuation exceeds byte limit")
    return result


class SlackOperations:
    """A selected Slack client/base URL; the host owns workspace/account identity.

    The normal base is https://slack.com/; an explicit proxy prefix is preserved.
    The selected credential's workspace/account is not inferred or verified here.
    The host owns auth, refresh, scopes, TLS, client mutation and closure. Its
    auth, hooks and transport must not retarget, retry or suppress cancellation.
    Separate account selections need separate host-bound clients/instances.
    allowed_channel_ids is an optional fixed host routing ceiling: None leaves
    routing unrestricted, while an empty list denies all channel calls. A
    restricted instance refuses broad conversation listing. This does not narrow
    provider credentials, authenticate a grant or filter quoted message content.
    """

    def __init__(
        self,
        client: httpx.AsyncClient,
        *,
        timeout_seconds: float = 20.0,
        max_response_bytes: int = 2_097_152,
        allowed_channel_ids: list[str] | tuple[str, ...] | None = None,
    ) -> None:
        base = client.base_url
        if (
            base.scheme not in {"http", "https"}
            or not base.host
            or base.username
            or base.password
            or base.query
            or base.fragment
        ):
            raise ValueError("client must select one HTTP(S) base URL without userinfo or query")
        if (
            isinstance(timeout_seconds, bool)
            or not isinstance(timeout_seconds, (int, float))
            or not math.isfinite(timeout_seconds)
            or timeout_seconds <= 0
        ):
            raise ValueError("timeout_seconds must be finite and positive")
        if type(max_response_bytes) is not int or max_response_bytes <= 0:
            raise ValueError("max_response_bytes must be a positive integer")
        if allowed_channel_ids is not None:
            if (type(allowed_channel_ids) not in (list, tuple)
                    or len(allowed_channel_ids) > 256
                    or any(type(item) is not str or not _CHANNEL.fullmatch(item)
                           for item in allowed_channel_ids)
                    or len(set(allowed_channel_ids)) != len(allowed_channel_ids)):
                raise ValueError("allowed_channel_ids must contain at most 256 unique Slack identifiers")
        # This fixed host ceiling narrows routing, not credentials or native grants.
        self.__allowed_channel_ids = (None if allowed_channel_ids is None
                                      else frozenset(allowed_channel_ids))
        self._client = client
        self._base = base
        self._timeout = float(timeout_seconds)
        self._max_bytes = max_response_bytes

    def _channel(self, value: str) -> str:
        channel = _identifier(value)
        if self.__allowed_channel_ids is not None and channel not in self.__allowed_channel_ids:
            raise SlackError(FailureKind.PERMISSION)
        return channel

    def _failure(
        self,
        kind: FailureKind,
        response: httpx.Response | None = None,
        provider_code: str | None = None,
    ) -> SlackError:
        retry_after = None
        if response is not None:
            value = response.headers.get("retry-after", "")
            if value.isascii() and value.isdecimal() and len(value) <= 9:
                retry_after = int(value)
        return SlackError(
            kind,
            status_code=None if response is None else response.status_code,
            provider_code=provider_code,
            retry_after_seconds=retry_after,
        )

    async def _json(self, method: str, params: list[tuple[str, str]]) -> dict[str, Any]:
        require_prepared_httpx_client(self._client)
        # Already pending cancellation must not start auth or the request owner.
        await anyio.lowlevel.checkpoint()
        # Raw asyncio cancellation can interrupt HTTPX's already-entered close
        # path. Keep the complete request/unwind in one retained task and use
        # AnyIO cancellation, which respects the transport's cleanup shields.
        scope = anyio.CancelScope(deadline=anyio.current_time() + self._timeout)
        owner = asyncio.create_task(self._request(method, params, scope))
        cancelled = False
        while not owner.done():
            try:
                await asyncio.shield(owner)
            except asyncio.CancelledError:
                cancelled = True
                # Also valid before _request enters the scope. Repeated caller
                # cancellation never sends Task.cancel into the HTTPX owner.
                scope.cancel()
            except Exception:
                # The actual completed owner's result is consumed below.
                break
        if cancelled:
            try:
                owner.result()
            except (Exception, asyncio.CancelledError):
                pass
            # Cancellation wins a race with an error/success, after joining and
            # observing the owner. Do not retain the caller's cancellation text.
            raise asyncio.CancelledError() from None
        return owner.result()

    async def _request(
        self, method: str, params: list[tuple[str, str]], scope: anyio.CancelScope
    ) -> dict[str, Any]:
        url = self._base.join(f"api/{method}").copy_with(
            query=str(httpx.QueryParams(tuple(params))).encode("ascii")
        )
        failure: BaseException | None = None
        response: httpx.Response | None = None
        payload: dict[str, Any] = {}
        try:
            # One scope covers send and the complete streamed body. The same
            # cancellation model protects HTTPX's earlier iterator cleanup too.
            with scope:
                headers = {"Accept": "application/json", "Accept-Encoding": "identity"}
                if method == "auth.test":
                    headers["Content-Type"] = "application/x-www-form-urlencoded"
                request = self._client.build_request(
                    "POST" if method == "auth.test" else "GET",
                    url,
                    headers=headers,
                    timeout=self._timeout,
                )
                # Remove merged client defaults, including token/channel/cursor
                # query parameters. Credential headers remain host-controlled.
                request.url = url
                response = await send_with_custody(self._client, request)
                try:
                    if response.status_code != 200:
                        kind = {
                            401: FailureKind.AUTHENTICATION,
                            403: FailureKind.PERMISSION,
                            404: FailureKind.NOT_FOUND,
                            429: FailureKind.RATE_LIMITED,
                        }.get(response.status_code, FailureKind.HTTP_STATUS)
                        if 300 <= response.status_code < 400:
                            kind = FailureKind.REDIRECT
                        failure = self._failure(kind, response)
                    elif response.headers.get("content-encoding", "identity").lower() != "identity":
                        failure = self._failure(FailureKind.UNSUPPORTED_ENCODING, response)
                    elif (
                        response.headers.get("content-type", "").split(";", 1)[0].strip().lower()
                        != "application/json"
                    ):
                        failure = self._failure(FailureKind.INVALID_RESPONSE, response)
                    else:
                        size = response.headers.get("content-length")
                        if size is not None and (not size.isascii() or not size.isdecimal()):
                            failure = self._failure(FailureKind.INVALID_RESPONSE, response)
                        elif size is not None and (len(size) > 12 or int(size) > self._max_bytes):
                            failure = self._failure(FailureKind.RESPONSE_TOO_LARGE, response)
                        else:
                            body = bytearray()
                            chunks = (
                                response.aiter_bytes()
                                if response.is_stream_consumed
                                else response.aiter_raw()
                            )
                            async for chunk in chunks:
                                if len(body) + len(chunk) > self._max_bytes:
                                    failure = self._failure(
                                        FailureKind.RESPONSE_TOO_LARGE, response
                                    )
                                    break
                                body.extend(chunk)
                            if failure is None:
                                try:
                                    if size is not None and len(body) != int(size):
                                        raise ValueError(
                                            "response length does not match its header"
                                        )
                                    decoded = json.loads(
                                        body.decode("utf-8"),
                                        object_pairs_hook=_object,
                                        parse_float=_number,
                                        parse_constant=_constant,
                                    )
                                    payload = (
                                        _discovery_response(
                                            decoded, method, dict(params).get("channel"),
                                            dict(params).get("user"),
                                        )
                                        if method
                                        in {"auth.test", "users.info", "conversations.list", "conversations.info", "chat.getPermalink"}
                                        else _page(decoded)
                                    )
                                except (ValueError, RecursionError, httpx.InvalidURL):
                                    failure = self._failure(FailureKind.INVALID_RESPONSE, response)
                                if failure is None and payload["ok"] is False:
                                    code = payload["error"]
                                    failure = self._failure(
                                        _PROVIDER_ERRORS.get(code, FailureKind.PROVIDER),
                                        response,
                                        code if code in _PROVIDER_ERRORS else None,
                                    )
                finally:
                    with anyio.CancelScope(shield=True):
                        await response.aclose()
            if scope.cancel_called:
                failure = self._failure(FailureKind.TIMEOUT)
        except HttpxClientNotPrepared:
            failure = (
                HttpxClientNotPrepared() if response is None else self._failure(FailureKind.CLIENT)
            )
        except asyncio.CancelledError:
            failure = asyncio.CancelledError()
        except (TimeoutError, httpx.TimeoutException):
            failure = self._failure(FailureKind.TIMEOUT)
        except httpx.HTTPError:
            failure = self._failure(FailureKind.TRANSPORT)
        except Exception:
            # Never retain a possibly secret-bearing host hook/auth exception.
            failure = self._failure(FailureKind.CLIENT)
        if failure is not None:
            raise failure from None
        return payload

    async def auth_test(self) -> dict[str, Any]:
        """Observe provider identity; the host owns selection, comparison and grants."""
        return await self._json("auth.test", [])

    async def get_user(self, user: str, *, include_locale: bool = False) -> dict[str, Any]:
        """Observe one exact user profile, without inferring channel membership."""
        user = _identifier(user)
        if type(include_locale) is not bool:
            raise ValueError("include_locale must be boolean")
        return await self._json(
            "users.info", [("user", user), ("include_locale", str(include_locale).lower())]
        )

    async def get_permalink(self, channel: str, message_ts: str) -> dict[str, Any]:
        """Read the provider's exact message link without constructing or opening it.

        A returned link does not attest the recipient's access to the message.
        """
        return await self._json(
            "chat.getPermalink",
            [("channel", self._channel(channel)), ("message_ts", _timestamp(message_ts))],
        )

    async def list_conversations_page(
        self,
        *,
        types: list[str] | tuple[str, ...] = ("public_channel",),
        exclude_archived: bool = False,
        cursor: str | None = None,
        limit: int = 100,
        team_id: str | None = None,
    ) -> dict[str, Any]:
        """One explicitly filtered page; even an empty page can have a next cursor."""
        if self.__allowed_channel_ids is not None:
            # Broad metadata and cursors cannot be safely narrowed by row filtering.
            raise SlackError(FailureKind.PERMISSION)
        if not isinstance(types, (list, tuple)) or not 1 <= len(types) <= 4:
            raise ValueError("types must be a nonempty list or tuple of at most four values")
        selected = tuple(types)
        allowed = {"public_channel", "private_channel", "mpim", "im"}
        if any(not isinstance(item, str) or item not in allowed for item in selected):
            raise ValueError("unsupported conversation type")
        if len(set(selected)) != len(selected):
            raise ValueError("conversation types must be unique")
        if type(exclude_archived) is not bool:
            raise ValueError("exclude_archived must be boolean")
        if type(limit) is not int or not 1 <= limit <= 999:
            raise ValueError("limit must be an integer from 1 to 999")
        params = [
            ("types", ",".join(selected)),
            ("exclude_archived", str(exclude_archived).lower()),
            ("limit", str(limit)),
        ]
        if cursor is not None:
            params.append(("cursor", _cursor(cursor)))
        if team_id is not None:
            params.append(("team_id", _identifier(team_id)))
        return await self._json("conversations.list", params)

    async def get_conversation(
        self, channel: str, *, include_locale: bool = False, include_num_members: bool = False
    ) -> dict[str, Any]:
        """Read exact conversation metadata, not a membership or audience attestation."""
        channel = self._channel(channel)
        if type(include_locale) is not bool or type(include_num_members) is not bool:
            raise ValueError("conversation detail flags must be boolean")
        return await self._json(
            "conversations.info",
            [
                ("channel", channel),
                ("include_locale", str(include_locale).lower()),
                ("include_num_members", str(include_num_members).lower()),
            ],
        )

    def _params(
        self,
        channel: str,
        oldest: str | None,
        latest: str | None,
        inclusive: bool,
        cursor: str | None,
        limit: int,
        include_all_metadata: bool,
    ) -> list[tuple[str, str]]:
        if not isinstance(channel, str) or not _CHANNEL.fullmatch(channel):
            raise ValueError("expected an alphanumeric conversation ID of at most 128 characters")
        self._channel(channel)
        if type(limit) is not int or not 1 <= limit <= 999:
            raise ValueError("limit must be an integer from 1 to 999")
        if type(inclusive) is not bool or type(include_all_metadata) is not bool:
            raise ValueError("inclusive and include_all_metadata must be boolean")
        params = [
            ("channel", channel),
            ("limit", str(limit)),
            ("inclusive", str(inclusive).lower()),
            ("include_all_metadata", str(include_all_metadata).lower()),
        ]
        for name, value in (("oldest", oldest), ("latest", latest)):
            if value is not None:
                params.append((name, _timestamp(value)))
        if cursor is not None:
            params.append(("cursor", _cursor(cursor)))
        return params

    async def history_page(
        self,
        channel: str,
        *,
        oldest: str | None = None,
        latest: str | None = None,
        inclusive: bool = False,
        cursor: str | None = None,
        limit: int = 15,
        include_all_metadata: bool = False,
    ) -> dict[str, Any]:
        """Read one conversations.history page. Never infer total coverage."""
        params = self._params(
            channel, oldest, latest, inclusive, cursor, limit, include_all_metadata
        )
        return await self._json("conversations.history", params)

    async def replies_page(
        self,
        channel: str,
        ts: str,
        *,
        oldest: str | None = None,
        latest: str | None = None,
        inclusive: bool = False,
        cursor: str | None = None,
        limit: int = 15,
        include_all_metadata: bool = False,
    ) -> dict[str, Any]:
        """Read one conversations.replies page for an explicit message timestamp."""
        params = self._params(
            channel, oldest, latest, inclusive, cursor, limit, include_all_metadata
        )
        params.append(("ts", _timestamp(ts)))
        return await self._json("conversations.replies", params)

    async def read_interval(
        self,
        channel: str,
        *,
        start: str,
        end: str,
        root_oldest: str,
        max_requests: int,
        max_messages: int,
        page_limit: int = 15,
        include_all_metadata: bool = False,
        resumable: bool = False,
        continuation: SlackIntervalContinuation | None = None,
        max_result_bytes: int = DEFAULT_INTERVAL_RESULT_BYTES,
    ) -> SlackIntervalResult:
        """Read an explicitly permitted discovery interval and selected creation window.

        Results retain occurrences, not deduplicated messages. Exhausted chains
        describe visible cursor traversal, never a complete historical snapshot.
        The existing borrowed-client page owner retains request and cleanup custody.
        max_result_bytes bounds the complete compact ASCII JSON result, including
        metadata and the optional checkpoint. No individual message is clipped.
        """
        start, end, root_oldest = _timestamp(start), _timestamp(end), _timestamp(root_oldest)
        if _decimal_compare(root_oldest, start) > 0 or _decimal_compare(start, end) >= 0:
            raise ValueError("expected root_oldest <= start < end")
        for budget in (max_requests, max_messages):
            if type(budget) is not int or not 0 < budget <= 9_007_199_254_740_991:
                raise ValueError("interval budgets must be positive safe integers")
        if (type(max_result_bytes) is not int
                or not MIN_INTERVAL_RESULT_BYTES <= max_result_bytes <= MAX_INTERVAL_RESULT_BYTES):
            raise ValueError("max_result_bytes must be an integer from 4096 through 16777216")
        # Validate the entire selection before any credential or page owner is entered.
        self._params(channel, root_oldest, end, True, None, page_limit, include_all_metadata)
        if type(resumable) is not bool or (continuation is not None and not resumable):
            raise ValueError("continuation requires resumable=True")
        selection: SlackIntervalSelection = {
            "channel": channel, "start": start, "end": end, "root_oldest": root_oldest,
            "page_limit": page_limit, "include_all_metadata": include_all_metadata,
        }
        restored = None if continuation is None else _interval_resume(continuation, selection)
        history: SlackIntervalChain = {"state": "pending", "pages": 0, "stop_reason": None}
        result: SlackIntervalResult = {
            "channel": channel, "start": start, "end": end, "root_oldest": root_oldest,
            "limits": {"max_requests": max_requests, "max_messages": max_messages,
                       "page_limit": page_limit, "max_response_bytes": self._max_bytes,
                       "max_result_bytes": max_result_bytes},
            "messages": [], "history": history, "threads": [], "requests_made": 0,
            "skipped_non_threadable": 0, "termination": "finished", "coverage_gaps": [],
        }
        queue: deque[tuple[str | None, SlackIntervalChain, str | None, list[str]]] = deque()
        roots: dict[str, None] = {}
        if restored is None:
            queue.append((None, history, None, []))
        else:
            history["state"] = "not_requested"
            roots = dict.fromkeys(restored["discovered_roots"])
            for step in restored["pending"]:
                root = step["root_ts"]
                if root is None:
                    history["state"] = "pending"
                    chain = history
                else:
                    thread: SlackIntervalThread = {
                        "root_ts": root, "state": "pending", "pages": 0, "stop_reason": None,
                    }
                    result["threads"].append(thread)
                    chain = thread
                queue.append((root, chain, step["cursor"], step["seen_cursors"]))
        if resumable:
            result["observation_scope"] = "current_call_only"

        def checkpoint(steps, discovered) -> SlackIntervalContinuation | None:
            if not steps:
                return None
            return {"schema_version": 1, "selection": dict(selection),
                    "pending": [{"root_ts": root, "cursor": cursor, "seen_cursors": list(seen)}
                                for root, _, cursor, seen in steps],
                    "discovered_roots": list(discovered)}

        def gap(kind: SlackIntervalGapKind, root: str | None, message_ts: str | None = None):
            item: SlackIntervalGap = {
                "kind": kind, "source": "history" if root is None else "replies",
            }
            if root is not None:
                item["root_ts"] = root
            if message_ts is not None:
                item["message_ts"] = message_ts
            result["coverage_gaps"].append(item)

        def fits(value, steps, discovered) -> bool:
            projected = dict(value)
            if resumable:
                projected["continuation"] = checkpoint(steps, discovered)
            return len(json.dumps(projected, ensure_ascii=True, allow_nan=False,
                                  separators=(",", ":")).encode("utf-8")) <= max_result_bytes

        def terminal(value, steps, reason, *, attempted=False, error=None):
            # Copy the joined graph so a queued chain still refers to the same
            # history/thread record in the projected result.
            stopped, pending = deepcopy((value, steps))
            root, chain, _, _ = pending[0]
            stopped["requests_made"] += int(attempted)
            stopped["termination"] = reason
            if reason != "request_budget":
                chain.update(state="stopped", stop_reason=reason)
            if error is not None:
                chain["error"] = error
            item = {"kind": reason, "source": "history" if root is None else "replies"}
            if root is not None:
                item["root_ts"] = root
            stopped["coverage_gaps"].append(item)
            return stopped, pending

        # These are the largest safe fields emitted by the ordinary page owner.
        # None occupies more JSON bytes than a standard three-digit HTTP status.
        # An unusual host transport's larger error is checked after observation.
        largest_error = {
            "kind": max((kind.value for kind in FailureKind), key=len),
            "status_code": None,
            "provider_code": max(_PROVIDER_ERRORS, key=len),
            "retry_after_seconds": 999_999_999,
        }

        def capacity_ok(value, steps, discovered) -> bool:
            if not fits(value, steps, discovered):
                return False
            if not steps:
                return True
            if value["requests_made"] >= max_requests:
                stopped, pending = terminal(value, steps, "request_budget")
                return fits(stopped, pending, discovered)
            for reason in ("result_budget", "message_budget", "continuation_budget", "request_failure"):
                stopped, pending = terminal(value, steps, reason, attempted=True,
                    error=largest_error if reason == "request_failure" else None)
                if not fits(stopped, pending, discovered):
                    return False
            return True

        def reject_page(before):
            old_result, old_queue, old_roots = before
            stopped, pending = terminal(old_result, old_queue, "result_budget", attempted=True)
            return stopped, pending, old_roots

        if _decimal_compare(root_oldest, "0") > 0:
            gap("older_roots_unsearched", None)
        if not capacity_ok(result, queue, roots):
            raise ValueError("max_result_bytes cannot fit the interval state and a bounded stop")
        while queue:
            if result["requests_made"] >= max_requests:
                result["termination"] = "request_budget"
                gap("request_budget", queue[0][0])
                break
            before = deepcopy((result, queue, roots))
            root, chain, selected_cursor, seen = queue.popleft()
            source: SlackIntervalSource = "history" if root is None else "replies"
            result["requests_made"] += 1
            try:
                if root is None:
                    page = await self.history_page(
                        channel, oldest=root_oldest, latest=end, inclusive=True,
                        cursor=selected_cursor, limit=page_limit,
                        include_all_metadata=include_all_metadata,
                    )
                else:
                    page = await self.replies_page(
                        channel, root, oldest=start, latest=end, inclusive=True,
                        cursor=selected_cursor, limit=page_limit,
                        include_all_metadata=include_all_metadata,
                    )
            except SlackError as error:
                unavailable = root is not None and error.provider_code == "thread_not_found"
                reason: SlackIntervalStop = "thread_not_found" if unavailable else "request_failure"
                chain.update(state="stopped", stop_reason=reason)
                chain["error"] = {
                    "kind": error.kind.value, "status_code": error.status_code,
                    "provider_code": error.provider_code,
                    "retry_after_seconds": error.retry_after_seconds,
                }
                gap(reason, root)
                if unavailable:
                    if not capacity_ok(result, queue, roots):
                        result, queue, roots = reject_page(before)
                        break
                    continue
                result["termination"] = "request_failure"
                queue.appendleft((root, chain, selected_cursor, seen))
                if not fits(result, queue, roots):
                    result, queue, roots = reject_page(before)
                break

            selected = [row for row in page["messages"]
                        if _decimal_compare(row["ts"], start) >= 0
                        and _decimal_compare(row["ts"], end) < 0]
            if len(selected) > max_messages - len(result["messages"]):
                chain.update(state="stopped", stop_reason="message_budget")
                result["termination"] = "message_budget"
                gap("message_budget", root)
                queue.appendleft((root, chain, selected_cursor, seen))
                break

            # Plan the entire page's next traversal before admitting any projection.
            planned_queue = deque(queue)
            planned_roots = dict(roots)
            new_threads: list[SlackIntervalThread] = []
            if root is None:
                for row in page["messages"]:
                    if (_decimal_compare(row["ts"], root_oldest) < 0
                            or _decimal_compare(row["ts"], end) > 0
                            or row.get("subtype") in ("channel_join", "channel_leave")):
                        continue
                    observed_root, reference_gap = _interval_reference(row)
                    if (reference_gap is None and observed_root is not None
                            and _decimal_compare(observed_root, end) < 0
                            and observed_root not in planned_roots):
                        planned_roots[observed_root] = None
                        thread = {"root_ts": observed_root, "state": "pending",
                                  "pages": 0, "stop_reason": None}
                        new_threads.append(thread)
                        planned_queue.append((observed_root, thread, None, []))
            metadata = page.get("response_metadata", {})
            next_cursor = metadata.get("next_cursor")
            stop: SlackIntervalStop | None = None
            follows = False
            if isinstance(next_cursor, str) and next_cursor:
                try:
                    _cursor(next_cursor)
                except ValueError:
                    stop = "unusable_cursor"
                if stop is None and next_cursor in seen:
                    stop = "cursor_cycle"
                if stop is None:
                    follows = True
                    planned_queue.append((root, chain, next_cursor, [*seen, next_cursor]))
            elif page.get("has_more") is True:
                stop = "unusable_cursor" if "next_cursor" in metadata else "missing_cursor"
            proposed = checkpoint(planned_queue, planned_roots) if resumable else None
            if proposed is not None and not _interval_fits(proposed):
                chain.update(state="stopped", stop_reason="continuation_budget")
                result["termination"] = "continuation_budget"
                gap("continuation_budget", root)
                queue.appendleft((root, chain, selected_cursor, seen))
                break
            queue, roots = planned_queue, planned_roots
            result["threads"].extend(new_threads)

            # Commit this page's projection as a unit before considering its continuation.
            chain["pages"] += 1
            chain.pop("next_cursor", None)
            chain.pop("has_more", None)
            metadata = page.get("response_metadata", {})
            if "next_cursor" in metadata:
                chain["next_cursor"] = metadata["next_cursor"]
            if "has_more" in page:
                chain["has_more"] = page["has_more"]
            for row in page["messages"]:
                observed_root: str | None = None
                if root is None and (
                    _decimal_compare(row["ts"], root_oldest) < 0
                    or _decimal_compare(row["ts"], end) > 0
                ):
                    gap("history_outside_discovery", None, row["ts"])
                elif root is None and row.get("subtype") in ("channel_join", "channel_leave"):
                    result["skipped_non_threadable"] += 1
                else:
                    observed_root, reference_gap = _interval_reference(row)
                    if reference_gap is not None:
                        gap(reference_gap, root, row["ts"])
                    elif root is not None and observed_root != root:
                        gap("reply_chain_mismatch", root, row["ts"])
                if _decimal_compare(row["ts"], start) >= 0 and _decimal_compare(row["ts"], end) < 0:
                    result["messages"].append({
                        "source": source, "root_ts": root if root is not None else observed_root,
                        "message": row,
                    })
            if "is_limited" in page:
                if type(page["is_limited"]) is not bool:
                    gap("invalid_coverage_metadata", root)
                elif page["is_limited"]:
                    gap("retention_limited", root)

            if follows:
                if page.get("has_more") is False:
                    gap("pagination_contradiction", root)
            elif stop is not None:
                chain.update(state="stopped", stop_reason=stop)
                gap(stop, root)
            else:
                chain["state"] = "exhausted"
            if not capacity_ok(result, queue, roots):
                result, queue, roots = reject_page(before)
                break
        if resumable:
            result["continuation"] = checkpoint(queue, roots)
        if not fits(result, queue, roots):
            raise ValueError("interval result cannot fit max_result_bytes")
        return result
