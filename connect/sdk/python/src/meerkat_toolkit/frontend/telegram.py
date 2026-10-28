"""Telegram frontend transport and provider observations, without host policy.

Extracted from user-owned example-ext-telegram at f187bf89. Source provenance
and the MIT OR Apache-2.0 grant for this copy are recorded in TELEGRAM.md. The
host still owns credentials, recipients, admission, durable rows and retries.
"""

from __future__ import annotations

import asyncio
import json
import math
import re
import secrets
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import PurePosixPath
from typing import Any, BinaryIO, Literal, Protocol, TypeVar
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

_JSON_RESPONSE_LIMIT = 64 * 1024 * 1024
_READ_CHUNK_BYTES = 64 * 1024
_TOKEN_LIMIT = 512
_Result = TypeVar("_Result")
# Explicit provider refusals supported by this sendMessage/sendPhoto profile.
# Status alone never proves non-delivery; unknown/new codes fail closed.
_REFUSAL_CODES = frozenset({400, 401, 403, 404, 429})


def _validate_message_thread_id(value: int | None) -> None:
    # The host selects the topic. Validate the bounded Bot API Integer shape
    # without inferring a topic, coercing identifiers, or admitting a send.
    if value is not None and (type(value) is not int or not 1 <= value <= 2147483647):
        raise ValueError("message_thread_id must be a positive 32-bit integer or None")


class _NoRedirect(HTTPRedirectHandler):
    def redirect_request(
        self, req: Any, fp: Any, code: int, msg: str, headers: Any, newurl: str
    ) -> None:
        return None


class TelegramTransportError(RuntimeError):
    """A provider response or transport failed without exposing credentials."""


class TelegramMediaTooLarge(TelegramTransportError):
    """The provider stream crossed the host-supplied byte ceiling."""


@dataclass(frozen=True)
class ProviderResponse:
    status_code: int
    body: dict[str, Any]


@dataclass(frozen=True)
class TelegramBotIdentity:
    """Observed bot account only, not a host identity, grant or readiness claim."""

    bot_id: str
    username: str | None
    can_join_groups: bool | None
    can_read_all_group_messages: bool | None


@dataclass(frozen=True)
class TelegramWebhookInfo:
    """Inbound configuration facts without the potentially secret callback URL.

    No configured webhook does not establish that no other poller is active.
    This observation never authorizes replacing a webhook or consuming updates.
    Error fields describe the last reported delivery error, not current health.
    """

    configured: bool
    has_custom_certificate: bool
    pending_update_count: int
    allowed_updates: tuple[str, ...] | None
    matches_expected_url: bool | None = None
    has_last_error: bool = False
    last_error_date: int | None = None


@dataclass(frozen=True)
class TelegramSendReceipt:
    """Provider observation only; never a grant, outbox row or retry command.

    provider_accepted means a usable message receipt, not recipient reading.
    provider_queued preserves Telegram's unusable message_id=0 observation.
    """

    outcome: Literal["provider_accepted", "provider_queued", "known_not_sent", "unknown"]
    provider_message_id: str | None = None
    error: str | None = None
    error_code: int | None = None
    retry_after_seconds: int | None = None
    # Presentation advice after a proved refusal, never authority to retry.
    # The durable delivery owner must admit/reserve a new plain attempt.
    retry_without_formatting: bool = False


def _description(value: str) -> str:
    # Preserve useful provider refusals, bounded and without control characters.
    return "".join(c for c in value if c.isprintable())[:512]


def _matches_destination(
    result: dict[str, Any], recipient: str | None, message_thread_id: int | None
) -> bool:
    # A bound receipt needs an actual Message destination. This compares
    # observations only; it does not resolve aliases or authorize a send.
    chat = result.get("chat")
    chat_id = chat.get("id") if isinstance(chat, dict) else None
    if type(chat_id) is not int or not 0 < abs(chat_id) < 2**52:
        return False
    if not isinstance(recipient, str):
        return False
    if re.fullmatch(r"-?[0-9]{1,16}", recipient):
        if int(recipient) != chat_id:
            return False
    else:
        username = chat.get("username")
        if (
            not isinstance(username, str)
            or re.fullmatch(r"[A-Za-z0-9_]{1,32}", username) is None
            or recipient != "@" + username
        ):
            return False
    if message_thread_id is not None and (
        type(message_thread_id) is not int or not 1 <= message_thread_id <= 2147483647
    ):
        return False
    if "message_thread_id" not in result:
        return message_thread_id is None
    observed_topic = result["message_thread_id"]
    return (
        type(observed_topic) is int
        and 1 <= observed_topic <= 2147483647
        and (message_thread_id is None or observed_topic == message_thread_id)
    )


def classify_send_response(
    response: ProviderResponse,
    *,
    expected_recipient: str | None = None,
    expected_message_thread_id: int | None = None,
) -> TelegramSendReceipt:
    """Classify sendMessage/sendPhoto only, from an authenticated direct response.

    Do not use this with sendChatAction, webhook HTTP replies or arbitrary MCP
    responses. Missing/malformed receipts, contradictory fields, redirects and
    server errors cannot justify another send.

    With neither expected field this preserves the legacy envelope-only check.
    A bound success requires a Message chat ID and an expected recipient: a
    signed decimal ID, or an alias matching the returned username exactly
    (including case). Missing aliases are not resolved from numeric IDs.
    An explicit expected topic requires the same returned topic. With no topic
    selected, an absent or valid provider-assigned topic is allowed. Missing,
    malformed or contradictory bound facts yield unknown, including for a
    queued message. Explicit refusals retain their existing classification.
    """
    unknown = TelegramSendReceipt("unknown", error="Telegram outcome unknown after dispatch")
    status, body = response.status_code, response.body
    if type(status) is not int or not isinstance(body, dict):
        return unknown
    if status == 200 and body.get("ok") is True:
        if "error_code" in body or "parameters" in body:
            return unknown
        result = body.get("result")
        message_id = result.get("message_id") if isinstance(result, dict) else None
        if type(message_id) is not int or message_id < 0:
            return unknown
        if (
            expected_recipient is not None or expected_message_thread_id is not None
        ) and not _matches_destination(result, expected_recipient, expected_message_thread_id):
            return unknown
        if message_id == 0:
            return TelegramSendReceipt(
                "provider_queued",
                error="Telegram accepted scheduled output but returned an unusable message receipt",
            )
        return TelegramSendReceipt("provider_accepted", provider_message_id=str(message_id))
    code, description = body.get("error_code"), body.get("description")
    if (
        status not in _REFUSAL_CODES
        or body.get("ok") is not False
        or type(code) is not int
        or code != status
        or "result" in body
        or not isinstance(description, str)
        or not description.strip()
    ):
        return unknown
    parameters = body.get("parameters", {})
    if not isinstance(parameters, dict):
        return unknown
    retry_after = parameters.get("retry_after")
    if retry_after is not None and (type(retry_after) is not int or retry_after < 0):
        return unknown
    return TelegramSendReceipt(
        "known_not_sent",
        error=_description(description),
        error_code=code,
        retry_after_seconds=retry_after,
    )


class TelegramTransport(Protocol):
    async def get_updates(self, offset: int | None) -> tuple[dict[str, Any], ...]: ...

    async def set_webhook(
        self,
        *,
        callback_url: str,
        secret_token: str,
        allowed_updates: tuple[str, ...],
        drop_pending_updates: bool,
    ) -> None: ...

    async def delete_webhook(self, *, drop_pending_updates: bool) -> None: ...

    async def get_file(self, file_id: str) -> dict[str, Any]: ...

    async def download_file(self, file_path: str, max_bytes: int) -> bytes: ...

    async def send_message(
        self,
        *,
        recipient: str,
        text: str,
        entities: Sequence[Mapping[str, Any]] | None = None,
        message_thread_id: int | None = None,
    ) -> ProviderResponse: ...

    async def send_chat_action(self, *, recipient: str, action: str) -> ProviderResponse: ...

    async def send_photo(
        self,
        *,
        recipient: str,
        data: bytes,
        mime_type: str,
        caption: str | None,
        caption_entities: Sequence[Mapping[str, Any]] | None = None,
        message_thread_id: int | None = None,
    ) -> ProviderResponse: ...

    async def close(self) -> None: ...


class TelegramBotApiTransport:
    """One-token client with bounded reads and joined thread custody.

    Calls have a 30-second socket timeout, not a hard total wall-clock limit.
    Cancellation waits for entered blocking work, then propagates cancellation;
    uncooperative threads cannot be preempted. Use on one asyncio event loop.
    No redirects, ambient proxies, transport retries or detached send work.
    """

    def __init__(self, token: str) -> None:
        if (
            not isinstance(token, str)
            or not token
            or len(token.encode("utf-8")) > _TOKEN_LIMIT
            or any(character.isspace() for character in token)
            or "/" in token
        ):
            raise ValueError("telegram bot token has an invalid shape")
        encoded_token = quote(token, safe=":")
        self._api_base = f"https://api.telegram.org/bot{encoded_token}/"
        self._file_base = f"https://api.telegram.org/file/bot{encoded_token}/"
        self._redactions = (token, encoded_token)
        self._closed = False
        self._operations: set[asyncio.Task[Any]] = set()

    async def get_me(self) -> TelegramBotIdentity:
        """Read the selected token's bot identity without activating a frontend."""
        result = self._require_ok(await self._api_call("getMe", {}))
        if not isinstance(result, dict):
            raise TelegramTransportError("Telegram getMe returned an invalid result")
        identifier, username = result.get("id"), result.get("username")
        first_name = result.get("first_name")
        flags = (result.get("can_join_groups"), result.get("can_read_all_group_messages"))
        if (
            type(identifier) is not int or not 0 < identifier < 2**52
            or result.get("is_bot") is not True
            or not isinstance(first_name, str) or not first_name
            or ("username" in result and (
                not isinstance(username, str)
                or re.fullmatch(r"[A-Za-z0-9_]{1,32}", username) is None
            ))
            or any(key in result and type(result[key]) is not bool
                for key in ("can_join_groups", "can_read_all_group_messages"))
        ):
            raise TelegramTransportError("Telegram getMe returned an invalid result")
        return TelegramBotIdentity(str(identifier), username, *flags)

    async def get_webhook_info(self, *, expected_url: str | None = None) -> TelegramWebhookInfo:
        """Read webhook facts and optionally compare the host's exact callback URL.

        The comparison is local, without normalization or returning either URL.
        None requests no comparison; an empty configured URL never matches a
        supplied nonempty expected URL. Last-error presence records either
        delivery-error field, even an empty message or zero timestamp. It does
        not establish a current failure or cover datacenter synchronization.
        """
        if expected_url is not None and (not isinstance(expected_url, str) or not expected_url):
            raise ValueError("expected_url must be a nonempty string or None")
        result = self._require_ok(await self._api_call("getWebhookInfo", {}))
        if not isinstance(result, dict):
            raise TelegramTransportError("Telegram getWebhookInfo returned an invalid result")
        url, certificate = result.get("url"), result.get("has_custom_certificate")
        pending, updates = result.get("pending_update_count"), result.get("allowed_updates")
        last_error_date = result.get("last_error_date")
        if (
            not isinstance(url, str)
            or type(certificate) is not bool
            or type(pending) is not int or not 0 <= pending <= 2147483647
            or ("allowed_updates" in result and (
                not isinstance(updates, list) or len(updates) > 128
                or any(not isinstance(value, str)
                    or re.fullmatch(r"[a-z][a-z0-9_]{0,63}", value) is None
                    for value in updates)
            ))
            or ("last_error_date" in result and (
                type(last_error_date) is not int or not 0 <= last_error_date <= 2147483647
            ))
            or ("last_error_message" in result and not isinstance(result["last_error_message"], str))
        ):
            raise TelegramTransportError("Telegram getWebhookInfo returned an invalid result")
        return TelegramWebhookInfo(
            bool(url), certificate, pending, None if updates is None else tuple(updates),
            None if expected_url is None else url == expected_url,
            "last_error_date" in result or "last_error_message" in result,
            last_error_date,
        )

    async def get_updates(self, offset: int | None) -> tuple[dict[str, Any], ...]:
        payload: dict[str, Any] = {"allowed_updates": ["message"]}
        if offset is not None:
            payload["offset"] = offset
        result = self._require_ok(await self._api_call("getUpdates", payload))
        if not isinstance(result, list) or any(not isinstance(item, dict) for item in result):
            raise TelegramTransportError("Telegram getUpdates returned an invalid result")
        return tuple(result)

    async def set_webhook(
        self,
        *,
        callback_url: str,
        secret_token: str,
        allowed_updates: tuple[str, ...],
        drop_pending_updates: bool,
    ) -> None:
        self._require_ok(
            await self._api_call(
                "setWebhook",
                {
                    "url": callback_url,
                    "secret_token": secret_token,
                    "allowed_updates": list(allowed_updates),
                    "drop_pending_updates": drop_pending_updates,
                },
            )
        )

    async def delete_webhook(self, *, drop_pending_updates: bool) -> None:
        self._require_ok(
            await self._api_call(
                "deleteWebhook",
                {"drop_pending_updates": drop_pending_updates},
            )
        )

    async def get_file(self, file_id: str) -> dict[str, Any]:
        result = self._require_ok(await self._api_call("getFile", {"file_id": file_id}))
        if not isinstance(result, dict):
            raise TelegramTransportError("Telegram getFile returned an invalid result")
        return result

    async def download_file(self, file_path: str, max_bytes: int) -> bytes:
        if not isinstance(max_bytes, int) or isinstance(max_bytes, bool) or max_bytes <= 0:
            raise ValueError("max_bytes must be a positive integer")
        normalized = self._provider_file_path(file_path)
        return await self._run_thread(self._download_file_sync, normalized, max_bytes)

    async def send_message(
        self,
        *,
        recipient: str,
        text: str,
        entities: Sequence[Mapping[str, Any]] | None = None,
        message_thread_id: int | None = None,
    ) -> ProviderResponse:
        _validate_message_thread_id(message_thread_id)
        payload: dict[str, Any] = {"chat_id": recipient, "text": text}
        if message_thread_id is not None:
            payload["message_thread_id"] = message_thread_id
        if entities:
            # Entities carry the formatting, so the text stays markup-free and
            # ``parse_mode`` is never set on ANY path in this transport. Absent
            # when empty: a plain send must produce the byte-identical body it
            # produced before rich text existed.
            payload["entities"] = [dict(entity) for entity in entities]
        return await self._api_call("sendMessage", payload)

    async def send_chat_action(self, *, recipient: str, action: str) -> ProviderResponse:
        return await self._api_call(
            "sendChatAction",
            {"chat_id": recipient, "action": action},
        )

    async def send_photo(
        self,
        *,
        recipient: str,
        data: bytes,
        mime_type: str,
        caption: str | None,
        caption_entities: Sequence[Mapping[str, Any]] | None = None,
        message_thread_id: int | None = None,
    ) -> ProviderResponse:
        body, content_type = self._multipart_photo(
            recipient=recipient,
            data=data,
            mime_type=mime_type,
            caption=caption,
            caption_entities=caption_entities,
            message_thread_id=message_thread_id,
        )
        return await self._run_thread(
            self._request_sync,
            self._api_base + "sendPhoto",
            body,
            content_type,
        )

    async def close(self) -> None:
        """Stop admission and join entered operations, even under cancellation."""
        self._closed = True
        if self._operations:
            await self._join(asyncio.gather(*self._operations, return_exceptions=True))

    @staticmethod
    async def _join(operation: asyncio.Future[_Result]) -> _Result:
        cancelled: asyncio.CancelledError | None = None
        while not operation.done():
            try:
                await asyncio.shield(operation)
            except asyncio.CancelledError as error:
                if cancelled is None:
                    cancelled = error
            except Exception:
                # Retrieve the original exception below after the operation ends.
                break
        if cancelled is not None:
            # Retrieve late failure without replacing the caller's cancellation.
            if not operation.cancelled():
                operation.exception()
            raise cancelled
        return operation.result()

    async def _run_thread(self, function: Callable[..., _Result], *args: Any) -> _Result:
        if self._closed:
            raise TelegramTransportError("Telegram transport is closed")
        caller = asyncio.current_task()
        if caller is not None and caller.cancelling():
            raise asyncio.CancelledError
        operation = asyncio.create_task(asyncio.to_thread(function, *args))
        self._operations.add(operation)
        try:
            return await self._join(operation)
        finally:
            self._operations.discard(operation)

    async def _api_call(self, method: str, payload: dict[str, Any]) -> ProviderResponse:
        body = json.dumps(
            payload,
            allow_nan=False,
            ensure_ascii=False,
            separators=(",", ":"),
            sort_keys=True,
        ).encode("utf-8")
        return await self._run_thread(
            self._request_sync,
            self._api_base + method,
            body,
            "application/json",
        )

    @staticmethod
    def _require_ok(response: ProviderResponse) -> Any:
        if (
            response.status_code == 200
            and response.body.get("ok") is True
            and "result" in response.body
            and "error_code" not in response.body
        ):
            return response.body["result"]
        raise TelegramTransportError("Telegram rejected the provider request")

    @staticmethod
    def _read_limited(stream: BinaryIO, limit: int) -> bytes:
        chunks: list[bytes] = []
        remaining = limit + 1
        while remaining > 0:
            chunk = stream.read(min(_READ_CHUNK_BYTES, remaining))
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        data = b"".join(chunks)
        if len(data) > limit:
            raise TelegramMediaTooLarge("Telegram media exceeds the configured byte limit")
        return data

    @classmethod
    def _read_json_response(cls, stream: BinaryIO) -> dict[str, Any]:
        raw = cls._read_limited(stream, _JSON_RESPONSE_LIMIT)
        try:
            value = json.loads(
                raw,
                object_pairs_hook=cls._unique_object,
                parse_float=cls._finite_float,
                parse_constant=cls._finite_float,
            )
        except (UnicodeDecodeError, ValueError):
            raise TelegramTransportError("Telegram returned invalid JSON") from None
        if not isinstance(value, dict):
            raise TelegramTransportError("Telegram returned a non-object response")
        return value

    @staticmethod
    def _finite_float(value: str) -> float:
        result = float(value)
        if not math.isfinite(result):
            raise ValueError("non-finite JSON number")
        return result

    @staticmethod
    def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        value: dict[str, Any] = {}
        for key, item in pairs:
            if key in value:
                raise ValueError("duplicate JSON member")
            value[key] = item
        return value

    def _open(self, request: Request) -> Any:
        # The token appears in the path. Never hand it to another origin or to
        # an ambient proxy. A host needing a custom network path supplies its
        # own transport under the same receipt/custody contract.
        return build_opener(ProxyHandler({}), _NoRedirect()).open(request, timeout=30)

    def _request_sync(self, url: str, body: bytes, content_type: str) -> ProviderResponse:
        request = Request(
            url,
            data=body,
            headers={"Content-Type": content_type, "User-Agent": "meerkat-toolkit/0.1.0"},
            method="POST",
        )
        try:
            try:
                response = self._open(request)
            except HTTPError as error:
                response = error
            with response:
                body_object = self._read_json_response(response)
                if isinstance(body_object.get("description"), str):
                    for secret in self._redactions:
                        body_object["description"] = body_object["description"].replace(
                            secret, "[redacted]"
                        )
                return ProviderResponse(int(response.status), body_object)
        except TelegramTransportError:
            raise
        except Exception:
            # URL-bearing urllib errors must not expose the bot token, including
            # through ordinary traceback chaining. The original effect is unknown.
            raise TelegramTransportError("Telegram request outcome unknown") from None

    def _download_file_sync(self, file_path: str, max_bytes: int) -> bytes:
        request = Request(
            self._file_base + quote(file_path, safe="/"),
            headers={"User-Agent": "meerkat-toolkit/0.1.0"},
            method="GET",
        )
        try:
            with self._open(request) as response:
                return self._read_limited(response, max_bytes)
        except HTTPError as error:
            error.close()
            raise TelegramTransportError("Telegram media download failed") from None
        except TelegramTransportError:
            raise
        except Exception:
            raise TelegramTransportError("Telegram media download failed") from None

    @staticmethod
    def _provider_file_path(value: str) -> str:
        if not isinstance(value, str) or not value or "\\" in value:
            raise TelegramTransportError("Telegram returned an invalid file path")
        path = PurePosixPath(value)
        if path.is_absolute() or any(part in {"", ".", ".."} for part in path.parts):
            raise TelegramTransportError("Telegram returned an invalid file path")
        if path.as_posix() != value:
            raise TelegramTransportError("Telegram returned an invalid file path")
        return value

    @staticmethod
    def _multipart_photo(
        *,
        recipient: str,
        data: bytes,
        mime_type: str,
        caption: str | None,
        caption_entities: Sequence[Mapping[str, Any]] | None = None,
        message_thread_id: int | None = None,
    ) -> tuple[bytes, str]:
        _validate_message_thread_id(message_thread_id)
        if not re.fullmatch(r"[A-Za-z0-9!#$&^_.+-]+/[A-Za-z0-9!#$&^_.+-]+", mime_type):
            raise ValueError("Telegram media MIME has an invalid shape")
        boundary = f"paired-host-{secrets.token_hex(16)}"
        chunks: list[bytes] = []

        def field(name: str, value: str) -> None:
            chunks.extend(
                [
                    f"--{boundary}\r\n".encode(),
                    f'Content-Disposition: form-data; name="{name}"\r\n\r\n'.encode(),
                    value.encode("utf-8"),
                    b"\r\n",
                ]
            )

        field("chat_id", recipient)
        if message_thread_id is not None:
            field("message_thread_id", str(message_thread_id))
        if caption is not None:
            field("caption", caption)
        if caption_entities:
            # Telegram takes caption_entities as a JSON-serialised array even in
            # multipart. The entities carry the formatting, so the caption text
            # stays markup-free and ``parse_mode`` is never set on ANY path in
            # this transport - the same contract as send_message.
            field(
                "caption_entities",
                json.dumps(
                    [dict(entity) for entity in caption_entities],
                    allow_nan=False,
                    ensure_ascii=False,
                    separators=(",", ":"),
                    sort_keys=True,
                ),
            )
        chunks.extend(
            [
                f"--{boundary}\r\n".encode(),
                b'Content-Disposition: form-data; name="photo"; filename="media"\r\n',
                f"Content-Type: {mime_type}\r\n\r\n".encode("ascii"),
                data,
                b"\r\n",
                f"--{boundary}--\r\n".encode(),
            ]
        )
        return b"".join(chunks), f"multipart/form-data; boundary={boundary}"


__all__ = [
    "ProviderResponse",
    "TelegramBotApiTransport",
    "TelegramBotIdentity",
    "TelegramMediaTooLarge",
    "TelegramTransport",
    "TelegramTransportError",
    "TelegramSendReceipt",
    "TelegramWebhookInfo",
    "classify_send_response",
]
