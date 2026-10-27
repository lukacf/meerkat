"""Slack provider facts and single-attempt text replies, never host admission.

Signature verification proves request origin only. The host owns installation,
principal/audience policy, durable inbox/egress, credentials and lifecycle.
HTTPX/AnyIO are loaded only when constructing the optional reply transport.
"""

from __future__ import annotations

import asyncio
from collections.abc import Sequence
from dataclasses import dataclass
import hashlib
import hmac
import json
import math
import re
from typing import Any

_ID = re.compile(r"[A-Za-z0-9]{1,128}\Z")
_CHANNEL = re.compile(r"[CDG][A-Za-z0-9]{1,127}\Z")
_TS = re.compile(r"[0-9]{1,32}(?:\.[0-9]{1,32})?\Z")
_SIGNATURE = re.compile(r"v0=[0-9a-f]{64}\Z")


class SlackFrontendError(ValueError):
    """Bounded local refusal, with no raw body, secret or exception chain."""

    def __init__(self, code: str, *, write_outcome: str = "not_applicable") -> None:
        self.code = code
        self.write_outcome = write_outcome
        super().__init__(f"Slack frontend {code}")


class SlackReplyCancelled(asyncio.CancelledError):
    """Caller cancellation after all request/response work has been joined."""

    def __init__(self, *, request_attempted: bool) -> None:
        self.request_attempted = request_attempted
        self.write_outcome = "unknown" if request_attempted else "not_attempted"
        super().__init__("Slack reply cancelled")


def _id(value: object) -> bool:
    return isinstance(value, str) and _ID.fullmatch(value) is not None


def _timestamp(value: object) -> bool:
    return isinstance(value, str) and _TS.fullmatch(value) is not None


def _utf8(value: object, *, maximum: int, nonempty: bool = True) -> bool:
    if not isinstance(value, str) or (nonempty and not value):
        return False
    try:
        return len(value.encode("utf-8")) <= maximum
    except UnicodeError:
        return False


@dataclass(frozen=True)
class SlackInstallation:
    app_id: str
    team_id: str
    bot_user_id: str

    def __post_init__(self) -> None:
        if not all(_id(v) for v in (self.app_id, self.team_id, self.bot_user_id)):
            raise SlackFrontendError("invalid_installation")


@dataclass(frozen=True)
class SlackTextEvent:
    app_id: str
    team_id: str
    event_id: str
    event_time: int
    event_type: str
    user_id: str
    channel_id: str
    message_ts: str
    thread_ts: str | None
    event_ts: str
    text: str
    body_sha256: str


@dataclass(frozen=True)
class SlackChallenge:
    challenge: str
    body_sha256: str


@dataclass(frozen=True)
class SlackUnsupportedEvent:
    reason: str
    event_id: str | None
    app_id: str | None
    team_id: str | None
    body_sha256: str


def _object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON member")
        result[key] = value
    return result


def _constant(value: str) -> Any:
    raise ValueError("invalid JSON number")


def _number(value: str) -> float:
    result = float(value)
    if not math.isfinite(result):
        raise ValueError("invalid JSON number")
    return result


def _decode(body: bytes) -> dict[str, Any] | None:
    try:
        value = json.loads(
            body.decode("utf-8"),
            object_pairs_hook=_object,
            parse_float=_number,
            parse_constant=_constant,
        )
        if isinstance(value, dict):
            return value
    except (ValueError, RecursionError):
        pass
    return None


def verify_event_request(
    raw_body: bytes,
    headers: Sequence[tuple[str, str]],
    *,
    signing_secret: bytes,
    installation: SlackInstallation,
    now_seconds: int | float,
    max_age_seconds: int = 300,
    max_body_bytes: int = 1_048_576,
) -> SlackTextEvent | SlackChallenge | SlackUnsupportedEvent:
    """Interpret one signed request. Does not acknowledge, admit or deduplicate.

    Supply raw, uncollapsed header pairs and exact undecoded request bytes. The
    signing secret belongs to the host-selected app. The timestamp freshness
    allowance is configurable from 1 to 300 seconds, not an event-age limit.
    """
    if (
        type(raw_body) is not bytes
        or type(signing_secret) is not bytes
        or not 0 < len(signing_secret) <= 8192
        or not isinstance(installation, SlackInstallation)
        or type(max_body_bytes) is not int
        or max_body_bytes <= 0
        or len(raw_body) > max_body_bytes
        or type(max_age_seconds) is not int
        or not 1 <= max_age_seconds <= 300
        or isinstance(now_seconds, bool)
        or not isinstance(now_seconds, (int, float))
        or not math.isfinite(now_seconds)
    ):
        raise SlackFrontendError("invalid_request")
    if not isinstance(headers, (list, tuple)) or len(headers) > 100:
        raise SlackFrontendError("invalid_headers")
    selected: dict[str, str] = {}
    for pair in headers:
        if (
            not isinstance(pair, (list, tuple))
            or len(pair) != 2
            or not all(isinstance(item, str) for item in pair)
        ):
            raise SlackFrontendError("invalid_headers")
        name, value = pair
        if (
            len(name) > 256
            or len(value) > 8192
            or not name.isascii()
            or any(ord(c) <= 32 or ord(c) >= 127 for c in name)
            or "\r" in value
            or "\n" in value
        ):
            raise SlackFrontendError("invalid_headers")
        name = name.lower()
        if name in {
            "x-slack-signature",
            "x-slack-request-timestamp",
            "content-type",
            "content-encoding",
        }:
            if name in selected:
                raise SlackFrontendError("invalid_headers")
            selected[name] = value
    signature = selected.get("x-slack-signature", "")
    stamp = selected.get("x-slack-request-timestamp", "")
    if (
        not _SIGNATURE.fullmatch(signature)
        or not stamp.isascii()
        or not stamp.isdecimal()
        or len(stamp) > 20
    ):
        raise SlackFrontendError("invalid_signature")
    if abs(now_seconds - int(stamp)) > max_age_seconds:
        raise SlackFrontendError("stale_request")
    expected = (
        "v0="
        + hmac.new(
            signing_secret, b"v0:" + stamp.encode("ascii") + b":" + raw_body, hashlib.sha256
        ).hexdigest()
    )
    if not hmac.compare_digest(expected, signature):
        raise SlackFrontendError("invalid_signature")
    if (
        selected.get("content-type", "").split(";", 1)[0].strip().lower() != "application/json"
        or selected.get("content-encoding", "identity").lower() != "identity"
    ):
        raise SlackFrontendError("invalid_request")
    payload = _decode(raw_body)
    if payload is None:
        raise SlackFrontendError("invalid_event")
    digest = hashlib.sha256(raw_body).hexdigest()
    if payload.get("type") == "url_verification":
        if not _utf8(payload.get("challenge"), maximum=4096):
            raise SlackFrontendError("invalid_event")
        if "api_app_id" in payload and payload["api_app_id"] != installation.app_id:
            raise SlackFrontendError("wrong_installation")
        return SlackChallenge(payload["challenge"], digest)
    if payload.get("type") != "event_callback":
        return SlackUnsupportedEvent("callback_type", None, None, None, digest)
    if (
        payload.get("api_app_id") != installation.app_id
        or payload.get("team_id") != installation.team_id
    ):
        raise SlackFrontendError("wrong_installation")
    if "context_team_id" in payload and payload["context_team_id"] != installation.team_id:
        raise SlackFrontendError("wrong_installation")
    event_id = payload.get("event_id")
    if not _id(event_id) or type(payload.get("event_time")) is not int or payload["event_time"] < 0:
        raise SlackFrontendError("invalid_event")
    facts = (event_id, installation.app_id, installation.team_id, digest)
    if payload.get("context_enterprise_id") is not None or payload.get("enterprise_id") is not None:
        return SlackUnsupportedEvent("enterprise", *facts)
    if "is_ext_shared_channel" in payload:
        if type(payload["is_ext_shared_channel"]) is not bool:
            raise SlackFrontendError("invalid_event")
        if payload["is_ext_shared_channel"]:
            return SlackUnsupportedEvent("shared_channel", *facts)
    authorizations = payload.get("authorizations", [])
    if not isinstance(authorizations, list):
        raise SlackFrontendError("invalid_event")
    for row in authorizations:
        if not isinstance(row, dict):
            raise SlackFrontendError("invalid_event")
        if row.get("enterprise_id") is not None or row.get("is_enterprise_install") is True:
            return SlackUnsupportedEvent("enterprise", *facts)
        if "is_enterprise_install" in row and type(row["is_enterprise_install"]) is not bool:
            raise SlackFrontendError("invalid_event")
        if "team_id" in row and row["team_id"] != installation.team_id:
            raise SlackFrontendError("wrong_installation")
    event = payload.get("event")
    if not isinstance(event, dict) or not isinstance(event.get("type"), str):
        raise SlackFrontendError("invalid_event")
    if event["type"] not in {"message", "app_mention"}:
        return SlackUnsupportedEvent("event_type", *facts)
    if "subtype" in event or "edited" in event:
        return SlackUnsupportedEvent("message_variant", *facts)
    if (
        any(key in event for key in ("bot_id", "bot_profile", "app_id"))
        or event.get("user") == installation.bot_user_id
    ):
        return SlackUnsupportedEvent("bot_or_self", *facts)
    if any(key in event for key in ("files", "attachments")):
        return SlackUnsupportedEvent("media", *facts)
    for name in ("team", "user_team", "source_team"):
        if name in event and event[name] != installation.team_id:
            return SlackUnsupportedEvent("shared_channel", *facts)
    if event["type"] == "message" and event.get("channel_type") != "im":
        if "channel_type" not in event or not isinstance(event["channel_type"], str):
            raise SlackFrontendError("invalid_event")
        return SlackUnsupportedEvent("conversation_type", *facts)
    if (
        not _id(event.get("user"))
        or not isinstance(event.get("channel"), str)
        or not _CHANNEL.fullmatch(event["channel"])
        or not _timestamp(event.get("ts"))
        or not _timestamp(event.get("event_ts"))
        or not _utf8(event.get("text"), maximum=max_body_bytes)
        or ("thread_ts" in event and not _timestamp(event["thread_ts"]))
    ):
        raise SlackFrontendError("invalid_event")
    if event["type"] == "message" and not event["channel"].startswith("D"):
        raise SlackFrontendError("invalid_event")
    return SlackTextEvent(
        installation.app_id,
        installation.team_id,
        event_id,
        payload["event_time"],
        event["type"],
        event["user"],
        event["channel"],
        event["ts"],
        event.get("thread_ts"),
        event["event_ts"],
        event["text"],
        digest,
    )


@dataclass(frozen=True)
class SlackReplyReceipt:
    outcome: str
    requested_channel_id: str
    requested_thread_ts: str
    observed_channel_id: str | None = None
    observed_thread_ts: str | None = None
    provider_message_id: str | None = None
    error: str | None = None
    status_code: int | None = None


_REFUSALS = frozenset(
    {
        "invalid_auth",
        "not_authed",
        "token_expired",
        "token_revoked",
        "account_inactive",
        "missing_scope",
        "not_in_channel",
        "channel_not_found",
        "is_archived",
        "cannot_reply_to_message",
        "msg_too_long",
        "no_text",
    }
)


class SlackReplyTransport:
    """Borrow a prepared host-authenticated client for one text-reply attempt.

    Host auth/hooks/transport must not retry, retarget or suppress cancellation.
    Client lifetime and current audience checks remain with the host. This
    transport neither opens a DM from a user ID nor falls back to channel posts.
    """

    def __init__(
        self, client: Any, *, timeout_seconds: float = 20.0, max_response_bytes: int = 2_097_152
    ) -> None:
        import httpx

        if not isinstance(client, httpx.AsyncClient):
            raise SlackFrontendError("invalid_client", write_outcome="not_attempted")
        base = client.base_url
        if (
            base.scheme not in {"http", "https"}
            or not base.host
            or base.username
            or base.password
            or base.query
            or base.fragment
            or isinstance(timeout_seconds, bool)
            or not isinstance(timeout_seconds, (int, float))
            or not math.isfinite(timeout_seconds)
            or timeout_seconds <= 0
            or type(max_response_bytes) is not int
            or max_response_bytes <= 0
        ):
            raise SlackFrontendError("invalid_client", write_outcome="not_attempted")
        self._client, self._base = client, base
        self._timeout, self._max_bytes = float(timeout_seconds), max_response_bytes

    async def send_text(self, channel_id: str, text: str, *, thread_ts: str) -> SlackReplyReceipt:
        import anyio
        from ..connectors._httpx_stream import HttpxClientNotPrepared, require_prepared_httpx_client

        if (
            not isinstance(channel_id, str)
            or not _CHANNEL.fullmatch(channel_id)
            or not _timestamp(thread_ts)
            or not _utf8(text, maximum=160_000)
            or len(text) > 40_000
        ):
            raise SlackFrontendError("invalid_request", write_outcome="not_attempted")
        prepared = True
        try:
            require_prepared_httpx_client(self._client)
        except HttpxClientNotPrepared:
            prepared = False
        if not prepared:
            raise SlackFrontendError("client_not_prepared", write_outcome="not_attempted")
        body = json.dumps(
            {
                "channel": channel_id,
                "text": text,
                "thread_ts": thread_ts,
                "reply_broadcast": False,
                "unfurl_links": False,
                "unfurl_media": False,
                "unfurl_app_links": False,
                "mrkdwn": False,
                "parse": "none",
                "link_names": False,
            },
            ensure_ascii=False,
            separators=(",", ":"),
        ).encode("utf-8")
        scope = anyio.CancelScope(deadline=anyio.current_time() + self._timeout)
        attempt = [False]
        owner = asyncio.create_task(self._request(body, channel_id, thread_ts, scope, attempt))
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
            raise SlackReplyCancelled(request_attempted=attempt[0]) from None
        return owner.result()

    async def _request(
        self, body: bytes, channel: str, thread: str, scope: Any, attempt: list[bool]
    ) -> SlackReplyReceipt:
        import anyio
        import httpx
        from ..connectors._httpx_stream import HttpxClientNotPrepared, send_with_custody

        response = None
        failure = None
        result = SlackReplyReceipt("unknown", channel, thread, error="client")
        try:
            with scope:
                await anyio.lowlevel.checkpoint()
                url = self._base.join("api/chat.postMessage").copy_with(query=b"")
                request = self._client.build_request(
                    "POST",
                    url,
                    content=body,
                    timeout=self._timeout,
                    headers={
                        "Content-Type": "application/json; charset=utf-8",
                        "Accept": "application/json",
                        "Accept-Encoding": "identity",
                    },
                )
                request.url = url
                for header in tuple(request.headers):
                    if header.startswith("if-") or header in {
                        "range",
                        "content-encoding",
                        "transfer-encoding",
                        "x-http-method",
                        "x-http-method-override",
                        "x-method-override",
                    }:
                        del request.headers[header]
                request.headers["content-length"] = str(len(body))
                attempt[0] = True
                response = await send_with_custody(self._client, request)
                try:
                    result = await self._receipt(response, channel, thread)
                finally:
                    with anyio.CancelScope(shield=True):
                        await response.aclose()
            if scope.cancel_called:
                result = SlackReplyReceipt("unknown", channel, thread, error="timeout")
        except HttpxClientNotPrepared:
            if response is None:
                attempt[0] = False
                failure = SlackFrontendError("client_not_prepared", write_outcome="not_attempted")
            else:
                result = SlackReplyReceipt("unknown", channel, thread, error="client")
        except asyncio.CancelledError:
            failure = SlackReplyCancelled(request_attempted=attempt[0])
        except (TimeoutError, httpx.TimeoutException):
            result = SlackReplyReceipt("unknown", channel, thread, error="timeout")
        except httpx.HTTPError:
            result = SlackReplyReceipt("unknown", channel, thread, error="transport")
        except Exception:
            result = SlackReplyReceipt("unknown", channel, thread, error="client")
        if failure is not None:
            raise failure from None
        return result

    async def _receipt(self, response: Any, channel: str, thread: str) -> SlackReplyReceipt:
        def unknown(kind: str) -> SlackReplyReceipt:
            return SlackReplyReceipt(
                "unknown", channel, thread, error=kind, status_code=response.status_code
            )

        if response.status_code != 200:
            return unknown("http_status")
        if (
            response.headers.get("content-encoding", "identity").lower() != "identity"
            or response.headers.get("content-type", "").split(";", 1)[0].strip().lower()
            != "application/json"
        ):
            return unknown("invalid_response")
        size = response.headers.get("content-length")
        if size is not None and (not size.isascii() or not size.isdecimal() or len(size) > 12):
            return unknown("invalid_response")
        if size is not None and int(size) > self._max_bytes:
            return unknown("response_too_large")
        body = bytearray()
        chunks = response.aiter_bytes() if response.is_stream_consumed else response.aiter_raw()
        async for chunk in chunks:
            if len(body) + len(chunk) > self._max_bytes:
                return unknown("response_too_large")
            body.extend(chunk)
        if size is not None and len(body) != int(size):
            return unknown("invalid_response")
        payload = _decode(bytes(body))
        if payload is None or type(payload.get("ok")) is not bool:
            return unknown("invalid_response")
        if payload["ok"] is False:
            if payload.get("error") in _REFUSALS and not any(
                key in payload for key in ("channel", "ts", "message")
            ):
                return SlackReplyReceipt(
                    "known_not_sent", channel, thread, error=payload["error"], status_code=200
                )
            return unknown("provider")
        if (
            "error" in payload
            or payload.get("channel") != channel
            or not _timestamp(payload.get("ts"))
        ):
            return unknown("invalid_receipt")
        message = payload.get("message")
        if not isinstance(message, dict) or message.get("ts") != payload["ts"]:
            return unknown("invalid_receipt")
        observed_thread = message.get("thread_ts")
        if "thread_ts" in message and (
            not _timestamp(observed_thread) or observed_thread != thread
        ):
            return unknown("invalid_receipt")
        return SlackReplyReceipt(
            "sent",
            channel,
            thread,
            observed_channel_id=channel,
            observed_thread_ts=observed_thread,
            provider_message_id=payload["ts"],
            status_code=200,
        )
