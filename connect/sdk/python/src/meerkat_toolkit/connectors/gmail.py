"""Explicit Gmail REST operations on a borrowed, host-authenticated HTTP client.

No credential lookup, OAuth, automatic retries, recipient selection or MCP
fallback. A write failure after attempt entry has an unknown remote outcome.
"""

from __future__ import annotations

import asyncio
import base64
from enum import Enum
import json
import math
import re
from typing import Any, cast
from urllib.parse import quote

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
    QUOTA_EXCEEDED = "quota_exceeded"
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


class GmailError(Exception):
    """Safe error metadata; no provider body, query, MIME, credential or URL.

    Retry-After is informational. It is not permission to repeat a write.
    """

    def __init__(
        self,
        kind: FailureKind,
        *,
        write_outcome: WriteOutcome,
        status_code: int | None = None,
        retry_after_seconds: int | None = None,
        provider_reason: str | None = None,
    ) -> None:
        self.kind = kind
        self.write_outcome = write_outcome
        self.status_code = status_code
        self.retry_after_seconds = retry_after_seconds
        self.provider_reason = provider_reason
        status = "" if status_code is None else f" (HTTP {status_code})"
        super().__init__(f"Gmail {kind.value}{status}; write outcome: {write_outcome.value}")


class SendCancelled(asyncio.CancelledError):
    """Compatibility name for cancellation of any entered Gmail write."""

    write_outcome = WriteOutcome.UNKNOWN

    def __init__(self) -> None:
        super().__init__("Gmail write cancelled; write outcome: unknown")


_ACCOUNT = re.compile(r"[A-Za-z0-9._+-]+@[A-Za-z0-9.-]+\Z")
_ID = re.compile(r"[A-Za-z0-9_-]+\Z")
_HEADER = re.compile(r"[!#$%&'*+.^_`|~0-9A-Za-z-]+\Z")
_FORBIDDEN_REASONS = {
    "rateLimitExceeded": ("usageLimits", FailureKind.RATE_LIMITED),
    "userRateLimitExceeded": ("usageLimits", FailureKind.RATE_LIMITED),
    "dailyLimitExceeded": ("usageLimits", FailureKind.QUOTA_EXCEEDED),
    "domainPolicy": ("global", FailureKind.PERMISSION),
}


def _text(value: str, maximum: int = 8192) -> str:
    if (
        not isinstance(value, str)
        or not value
        or any(ord(char) < 32 or ord(char) == 127 for char in value)
    ):
        raise ValueError("expected nonempty text without control characters")
    try:
        if len(value.encode("utf-8")) <= maximum:
            return value
    except UnicodeError:
        pass
    raise ValueError("text exceeds the supported UTF-8 limit") from None


def _id(value: str) -> str:
    if not isinstance(value, str) or len(value) > 1024 or not _ID.fullmatch(value):
        raise ValueError("expected a Gmail message or label identifier")
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


def _message(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("id"), str)
        and bool(value["id"])
        and isinstance(value.get("threadId"), str)
        and bool(value["threadId"])
        and ("payload" not in value or isinstance(value["payload"], dict))
        and ("raw" not in value or isinstance(value["raw"], str))
        and (
            "labelIds" not in value
            or (
                isinstance(value["labelIds"], list)
                and all(isinstance(label, str) for label in value["labelIds"])
            )
        )
    )


class GmailOperations:
    """One selected mailbox and base URL, with no ownership of the borrowed client.

    The host supplies the Gmail API origin (normally https://gmail.googleapis.com/)
    and explicitly selects ``user_id`` as ``me`` or a simple ASCII email address.
    This captures routing, not proof of credential/account correspondence. The
    host owns auth, refresh, grants, TLS, client mutation and eventual closure.
    Auth, hooks and transports must not retarget or replay requests.
    """

    def __init__(
        self,
        client: httpx.AsyncClient,
        *,
        user_id: str,
        base_url: str | httpx.URL | None = None,
        auth: httpx.Auth | None = None,
        timeout_seconds: float = 20.0,
        max_response_bytes: int = 2_097_152,
        max_message_bytes: int = 4_194_304,
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
            raise ValueError("client must select one HTTP(S) base URL without userinfo or query")
        if (
            not isinstance(user_id, str)
            or len(user_id) > 320
            or (user_id != "me" and not _ACCOUNT.fullmatch(user_id))
        ):
            raise ValueError("user_id must be me or a supported ASCII email address")
        if (
            isinstance(timeout_seconds, bool)
            or not isinstance(timeout_seconds, (int, float))
            or not math.isfinite(timeout_seconds)
            or timeout_seconds <= 0
        ):
            raise ValueError("timeout_seconds must be finite and positive")
        for limit in (max_response_bytes, max_message_bytes):
            if type(limit) is not int or limit <= 0:
                raise ValueError("byte limits must be positive integers")
        self._client = client
        self._base = base
        self._auth = auth
        self._user_path = quote(user_id, safe="")
        self._timeout = float(timeout_seconds)
        self._max_bytes = max_response_bytes
        self._max_message_bytes = max_message_bytes

    def _failure(
        self,
        kind: FailureKind,
        *,
        write: bool,
        response: httpx.Response | None = None,
        provider_reason: str | None = None,
    ) -> GmailError:
        retry_after = None
        if response is not None:
            value = response.headers.get("retry-after", "")
            if value.isascii() and value.isdecimal() and len(value) <= 9:
                retry_after = int(value)
        return GmailError(
            kind,
            write_outcome=WriteOutcome.UNKNOWN if write else WriteOutcome.NOT_APPLICABLE,
            status_code=None if response is None else response.status_code,
            retry_after_seconds=retry_after,
            provider_reason=provider_reason,
        )

    async def _forbidden_reason(self, response: httpx.Response) -> str | None:
        """Read optional 403 diagnostics under the existing request/close owner.

        Failed diagnostics never replace the known HTTP refusal. Cancellation
        still reaches the request owner; no provider text escapes this reader.
        """
        try:
            if (response.headers.get("content-encoding", "identity").lower() != "identity"
                    or response.headers.get("content-type", "").split(";", 1)[0].strip().lower()
                    != "application/json"):
                return None
            limit = min(self._max_bytes, 65_536)
            size = response.headers.get("content-length")
            if size is not None and (not size.isascii() or not size.isdecimal()
                                     or len(size) > 12 or int(size) > limit):
                return None
            body = bytearray()
            chunks = response.aiter_bytes() if response.is_stream_consumed else response.aiter_raw()
            async for chunk in chunks:
                if len(body) + len(chunk) > limit:
                    return None
                body.extend(chunk)
            if size is not None and len(body) != int(size):
                return None
            payload = json.loads(body.decode("utf-8"), object_pairs_hook=_object,
                                 parse_float=_number, parse_constant=_constant)
            error = payload.get("error") if type(payload) is dict else None
            if (type(error) is not dict or type(error.get("code")) is not int
                    or error["code"] != 403):
                return None
            entries = error.get("errors")
            if type(entries) is not list or not 1 <= len(entries) <= 32:
                return None
            reason = None
            for entry in entries:
                if type(entry) is not dict:
                    return None
                current = entry.get("reason")
                if (type(current) is not str or current not in _FORBIDDEN_REASONS
                        or entry.get("domain") != _FORBIDDEN_REASONS[current][0]
                        or (reason is not None and current != reason)):
                    return None
                reason = current
            return reason
        except Exception:
            # Body I/O, JSON and metadata errors are optional diagnostic failure.
            # CancelledError is a BaseException and retains the outer custody.
            return None

    async def _json(
        self,
        method: str,
        path: str,
        *,
        write: bool = False,
        content: bytes | None = None,
        params: list[tuple[str, str]] | None = None,
    ) -> Any:
        require_prepared_httpx_client(self._client)
        url = self._base.join(f"gmail/v1/users/{self._user_path}/{path}")
        if params:
            url = url.copy_with(query=str(httpx.QueryParams(tuple(params))).encode("ascii"))
        scope = anyio.CancelScope(deadline=anyio.current_time() + self._timeout)
        owner = asyncio.create_task(self._request(method, url, content, write, scope))
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
            raise (SendCancelled() if write else asyncio.CancelledError()) from None
        return owner.result()

    async def _request(
        self,
        method: str,
        url: httpx.URL,
        content: bytes | None,
        write: bool,
        scope: anyio.CancelScope,
    ) -> Any:
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
                # build_request merges client.params; that must not change the
                # selected account, representation or message query.
                request.url = url
                request.headers.pop("X-HTTP-Method-Override", None)
                response = await send_with_custody(self._client, request, auth=self._auth)
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
                        reason = await self._forbidden_reason(response) if response.status_code == 403 else None
                        if reason is not None:
                            kind = _FORBIDDEN_REASONS[reason][1]
                        failure = self._failure(kind, write=write, response=response, provider_reason=reason)
                    elif response.headers.get("content-encoding", "identity").lower() != "identity":
                        failure = self._failure(
                            FailureKind.UNSUPPORTED_ENCODING, write=write, response=response
                        )
                    elif (
                        response.headers.get("content-type", "").split(";", 1)[0].strip().lower()
                        != "application/json"
                    ):
                        failure = self._failure(
                            FailureKind.INVALID_RESPONSE, write=write, response=response
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
                                        FailureKind.RESPONSE_TOO_LARGE,
                                        write=write,
                                        response=response,
                                    )
                                    break
                                body.extend(chunk)
                            if failure is None:
                                try:
                                    if size is not None and len(body) != int(size):
                                        raise ValueError("response length does not match its header")
                                    payload = json.loads(
                                        body.decode("utf-8"),
                                        object_pairs_hook=_object,
                                        parse_float=_number,
                                        parse_constant=_constant,
                                    )
                                except (ValueError, RecursionError):
                                    failure = self._failure(
                                        FailureKind.INVALID_RESPONSE,
                                        write=write,
                                        response=response,
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
            failure = SendCancelled() if write else asyncio.CancelledError()
        except (TimeoutError, httpx.TimeoutException):
            failure = self._failure(FailureKind.TIMEOUT, write=write)
        except httpx.HTTPError:
            failure = self._failure(FailureKind.TRANSPORT, write=write)
        except Exception:
            # Host auth/hook errors may contain secrets too. Do not retain their
            # exception object or raise inside its handler (implicit context).
            failure = self._failure(FailureKind.CLIENT, write=write)
        if failure is not None:
            raise failure from None
        return payload

    async def get_profile(self) -> dict[str, Any]:
        """Observe the host-selected mailbox profile without normalizing its email.

        The host compares this observation with its intended mailbox. It is not
        a stable issuer/subject identity, a scope inventory, send-as verification
        or an authorization decision. gmail.send alone cannot call this API.
        """
        result = await self._json("GET", "profile")
        valid = isinstance(result, dict)
        if valid:
            try:
                for field in ("emailAddress", "historyId"):
                    if field == "historyId" and field not in result:
                        continue
                    value = _text(result.get(field), maximum=1024)
                    # _text already refuses C0 and DEL; profile identity text
                    # also excludes C1 controls while preserving Unicode.
                    if any(128 <= ord(char) <= 159 for char in value):
                        raise ValueError("profile text contains a control character")
                for field in ("messagesTotal", "threadsTotal"):
                    if field in result and (type(result[field]) is not int or result[field] < 0):
                        raise ValueError("profile count must be a nonnegative integer")
            except ValueError:
                valid = False
        if not valid:
            raise self._failure(FailureKind.INVALID_RESPONSE, write=False)
        return cast(dict[str, Any], result)

    async def list_messages_page(
        self,
        *,
        query: str | None = None,
        label_ids: list[str] | tuple[str, ...] = (),
        page_token: str | None = None,
        max_results: int = 100,
        include_spam_trash: bool = False,
    ) -> dict[str, Any]:
        """Read one page and preserve the whole response, including absent fields.

        A nextPageToken is opaque continuation data, not a complete result or
        permission for another request. This method never loops or deduplicates.
        Query support depends on the host's selected scope (not gmail.metadata).
        """
        if type(max_results) is not int or not 1 <= max_results <= 500:
            raise ValueError("max_results must be an integer from 1 to 500")
        if type(include_spam_trash) is not bool:
            raise ValueError("include_spam_trash must be boolean")
        if not isinstance(label_ids, (list, tuple)) or len(label_ids) > 100:
            raise ValueError("label_ids must be a bounded list or tuple")
        params = [
            ("maxResults", str(max_results)),
            ("includeSpamTrash", str(include_spam_trash).lower()),
        ]
        if query is not None:
            params.append(("q", _text(query)))
        if page_token is not None:
            params.append(("pageToken", _text(page_token)))
        params.extend(("labelIds", _id(label)) for label in label_ids)
        result = await self._json("GET", "messages", params=params)
        if (
            not isinstance(result, dict)
            or (
                "messages" in result
                and (
                    not isinstance(result["messages"], list)
                    or not all(_message(row) for row in result["messages"])
                )
            )
            or (
                "nextPageToken" in result
                and (not isinstance(result["nextPageToken"], str) or not result["nextPageToken"])
            )
            or (
                "resultSizeEstimate" in result
                and (
                    type(result["resultSizeEstimate"]) is not int
                    or result["resultSizeEstimate"] < 0
                )
            )
        ):
            raise self._failure(FailureKind.INVALID_RESPONSE, write=False)
        return result

    async def get_message(
        self,
        message_id: str,
        *,
        format: str = "full",
        metadata_headers: list[str] | tuple[str, ...] = (),
    ) -> dict[str, Any]:
        """Preserve the provider's selected representation and nested MIME data."""
        message_id = _id(message_id)
        if format not in ("full", "metadata", "minimal", "raw"):
            raise ValueError("unsupported Gmail message format")
        if (
            not isinstance(metadata_headers, (list, tuple))
            or len(metadata_headers) > 100
            or (metadata_headers and format != "metadata")
        ):
            raise ValueError(
                "metadata_headers requires metadata format and a bounded list or tuple"
            )
        params = [("format", format)]
        for header in metadata_headers:
            if not isinstance(header, str) or len(header) > 998 or not _HEADER.fullmatch(header):
                raise ValueError("invalid metadata header name")
            params.append(("metadataHeaders", header))
        result = await self._json("GET", f"messages/{message_id}", params=params)
        if (
            not _message(result)
            or result["id"] != message_id
            or (format == "raw" and "raw" not in result)
        ):
            raise self._failure(FailureKind.INVALID_RESPONSE, write=False)
        return cast(dict[str, Any], result)

    def _raw_message(self, mime: bytes, thread_id: str | None) -> dict[str, str]:
        if not isinstance(mime, bytes) or not mime or len(mime) > self._max_message_bytes:
            raise ValueError("mime must be nonempty immutable bytes within max_message_bytes")
        message = {"raw": base64.urlsafe_b64encode(mime).decode("ascii")}
        if thread_id is not None:
            message["threadId"] = _id(thread_id)
        return message

    async def send_raw(self, mime: bytes, *, thread_id: str | None = None) -> dict[str, Any]:
        """Send exact immutable RFC 2822 MIME bytes in one JSON POST.

        The host authorizes every recipient (including Cc/Bcc) and the complete
        bytes before entry. This method bounds/encodes bytes; it does not build,
        parse, repair or reinterpret MIME or add a Message-ID/idempotency key.
        Optional thread routing does not supply required reply headers/subject.
        A successful Message is a provider receipt, not recipient delivery proof.
        """
        content = json.dumps(self._raw_message(mime, thread_id)).encode("ascii")
        result = await self._json("POST", "messages/send", write=True, content=content)
        if not _message(result):
            raise self._failure(FailureKind.INVALID_RESPONSE, write=True)
        return cast(dict[str, Any], result)

    async def create_draft_raw(
        self, mime: bytes, *, thread_id: str | None = None
    ) -> dict[str, Any]:
        """Create one draft from exact bytes, without sending or automatic retry."""
        content = json.dumps({"message": self._raw_message(mime, thread_id)}).encode("ascii")
        result = await self._json("POST", "drafts", write=True, content=content)
        if (
            not isinstance(result, dict)
            or not isinstance(result.get("id"), str)
            or not result["id"]
            or not _message(result.get("message"))
        ):
            raise self._failure(FailureKind.INVALID_RESPONSE, write=True)
        return result

    async def modify_message(
        self,
        message_id: str,
        *,
        add_label_ids: list[str] | tuple[str, ...] = (),
        remove_label_ids: list[str] | tuple[str, ...] = (),
    ) -> dict[str, Any]:
        """Add/remove ordinary labels on one message; no policy or retry owner."""
        message_id = _id(message_id)
        body: dict[str, list[str]] = {}
        for name, labels in (("addLabelIds", add_label_ids), ("removeLabelIds", remove_label_ids)):
            if not isinstance(labels, (list, tuple)) or len(labels) > 100:
                raise ValueError("label IDs must be a list or tuple of at most 100 identifiers")
            if labels:
                body[name] = [_id(label) for label in labels]
        if not body:
            raise ValueError("at least one label addition or removal is required")
        content = json.dumps(body).encode("ascii")
        result = await self._json(
            "POST", f"messages/{message_id}/modify", write=True, content=content
        )
        if not _message(result) or result["id"] != message_id:
            raise self._failure(FailureKind.INVALID_RESPONSE, write=True)
        return cast(dict[str, Any], result)

    async def get_attachment(self, message_id: str, attachment_id: str) -> dict[str, Any]:
        """Read one bounded MessagePartBody; caching/conversion remain host-owned."""
        message_id, attachment_id = _id(message_id), _id(attachment_id)
        result = await self._json("GET", f"messages/{message_id}/attachments/{attachment_id}")
        if (
            not isinstance(result, dict)
            or type(result.get("size")) is not int
            or result["size"] < 0
            or not isinstance(result.get("data"), str)
        ):
            raise self._failure(FailureKind.INVALID_RESPONSE, write=False)
        data = result["data"]
        try:
            if not re.fullmatch(r"[A-Za-z0-9_-]*={0,2}", data):
                raise ValueError("invalid base64url")
            decoded = base64.b64decode(data + "=" * (-len(data) % 4), altchars=b"-_", validate=True)
            if len(decoded) != result["size"]:
                raise ValueError("attachment size mismatch")
        except (ValueError, UnicodeError):
            raise self._failure(FailureKind.INVALID_RESPONSE, write=False) from None
        return result
