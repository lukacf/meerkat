"""Portable Telegram provider interpretation and presentation.

The host selects credentials/transport, maps principals, authorizes recipients and
media, owns lifecycle, and commits inbox cursors. No returned DTO is admission.
This module imports no paired host or agent runtime. PTB is lazy and optional.
"""

from __future__ import annotations

import mimetypes
from collections.abc import Mapping, Sequence
from copy import deepcopy
from dataclasses import dataclass, replace
from typing import Any, Literal, cast

from .telegram import (
    ProviderResponse,
    TelegramMediaTooLarge,
    TelegramSendReceipt,
    TelegramTransport,
    TelegramTransportError,
    _validate_message_thread_id,
    classify_send_response,
)

JsonObject = dict[str, Any]
_PROVIDER_REF_LIMIT_BYTES = 4096
_FILENAME_LIMIT_BYTES = 1024
# Interpretation only; a host must separately authorize any edit notice.
TELEGRAM_EDIT_OUTCOME_VERSION = 1


class TelegramAdapterError(ValueError):
    """A bounded provider interpretation failure, never an admission decision."""

    def __init__(
        self,
        code: Literal["invalid_request", "provider_error", "runtime_not_ready"],
        message: str,
        *,
        retryable: bool = False,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.retryable = retryable


def _adapter_error(
    code: Literal["invalid_request", "provider_error", "runtime_not_ready"],
    message: str,
    *,
    retryable: bool = False,
) -> TelegramAdapterError:
    return TelegramAdapterError(code, message, retryable=retryable)


@dataclass(frozen=True)
class TelegramMediaRef:
    part_index: int
    provider_ref: str
    claimed_size_bytes: int | None = None
    claimed_mime_type: str | None = None
    filename: str | None = None


@dataclass(frozen=True)
class TelegramEvent:
    """Provider facts only. update_id is not a globally scoped host identity."""

    update_id: int
    envelope: JsonObject
    media: tuple[TelegramMediaRef, ...] = ()


@dataclass(frozen=True)
class TelegramUnsupportedEvent:
    """Explicitly unhandled provider content, never a conversational event.

    facts retains identity and any accompanying text for host-owned rejection
    and fallback. It deliberately has no ``envelope``/``media`` projection that
    a consumer written for TelegramEvent could accidentally admit unchanged.
    """

    update_id: int
    reason: Literal["service_message", "content_type", "unknown_fields", "sender_kind", "message_edit"]
    facts: JsonObject


@dataclass(frozen=True)
class TelegramPage:
    """Ordered outcomes; the host must handle all before committing next_cursor."""

    events: tuple[TelegramEvent | TelegramUnsupportedEvent, ...]
    next_cursor: str | None
    ignored_updates: int = 0


@dataclass(frozen=True)
class TelegramFetchedMedia:
    data: bytes
    mime_type: str


_ENTITY_TYPES = frozenset(
    {
        "bold",
        "italic",
        "underline",
        "strikethrough",
        "spoiler",
        "code",
        "pre",
        "blockquote",
        "expandable_blockquote",
    }
)
# OURS, not a quoted Telegram limit. Telegram publishes no entity-count cap we
# can rely on, so the ceiling is a number we own and can change alone.
_ENTITY_COUNT_CEILING = 100
# Any of these turns a formatting record into a tappable link or a mention,
# which is the one injection primitive that matters in a family chat. They are
# rejected structurally rather than filtered, so a core that starts emitting
# them degrades loudly in the counters instead of shipping them.
_FORBIDDEN_ENTITY_KEYS = frozenset({"url", "user", "custom_emoji_id"})
_ENTITY_KEYS = frozenset({"type", "offset", "length", "language"})


def _utf8_within(value: str, limit: int) -> bool:
    try:
        return len(value.encode("utf-8")) <= limit
    except UnicodeError:
        return False


def _utf16_len(value: str) -> int:
    return sum(2 if ord(char) > 0xFFFF else 1 for char in value)


def _valid_entities(entities: object, text: str) -> tuple[dict[str, Any], ...] | None:
    """Return sanitized entities, or ``None`` meaning "send this plain"."""

    if not isinstance(entities, (list, tuple)):
        return None
    if not entities:
        return ()
    if len(entities) > _ENTITY_COUNT_CEILING:
        return None
    limit = _utf16_len(text)
    checked: list[dict[str, Any]] = []
    previous_offset = -1
    for entity in entities:
        if not isinstance(entity, Mapping):
            return None
        keys = set(entity)
        if keys & _FORBIDDEN_ENTITY_KEYS or not keys <= _ENTITY_KEYS:
            return None
        kind = entity.get("type")
        offset = entity.get("offset")
        length = entity.get("length")
        if kind not in _ENTITY_TYPES:
            return None
        if not isinstance(offset, int) or isinstance(offset, bool) or offset < 0:
            return None
        if not isinstance(length, int) or isinstance(length, bool) or length <= 0:
            return None
        if offset + length > limit:
            return None
        if offset < previous_offset:
            return None  # unsorted
        previous_offset = offset
        row: dict[str, Any] = {"type": kind, "offset": offset, "length": length}
        language = entity.get("language")
        if language is not None:
            if kind != "pre" or not isinstance(language, str) or not language:
                return None
            row["language"] = language
        checked.append(row)
    for index, outer in enumerate(checked):
        outer_end = outer["offset"] + outer["length"]
        for inner in checked[index + 1 :]:
            if inner["offset"] >= outer_end:
                break
            if inner["offset"] + inner["length"] > outer_end:
                return None  # partial overlap: Telegram rejects the whole send
    return tuple(checked)


def _checked_text(value: object, *, field: str, max_bytes: int) -> str:
    if not isinstance(value, str) or not value or not _utf8_within(value, max_bytes):
        raise _adapter_error("invalid_request", f"Telegram {field} is invalid")
    return value


def _optional_size(value: object) -> int | None:
    if value is None:
        return None
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise _adapter_error("invalid_request", "Telegram media size is invalid")
    return value


def _optional_filename(value: object) -> str | None:
    if value is None or value == "":
        return None
    if not isinstance(value, str) or not _utf8_within(value, _FILENAME_LIMIT_BYTES):
        raise _adapter_error("invalid_request", "Telegram media filename is invalid")
    return value


def _normalize_mime(value: object, *, default: str | None = None) -> str | None:
    if value is None or value == "":
        return default
    if not isinstance(value, str):
        raise _adapter_error("invalid_request", "Telegram media MIME type is invalid")
    rendered = value.split(";", 1)[0].strip().lower()
    if not rendered or "/" not in rendered or any(character in rendered for character in "\r\n"):
        raise _adapter_error("invalid_request", "Telegram media MIME type is invalid")
    return rendered


def _provider_ref(value: object) -> str:
    return _checked_text(
        value,
        field="provider media reference",
        max_bytes=_PROVIDER_REF_LIMIT_BYTES,
    )


def _telegram_media(message: Any) -> tuple[TelegramMediaRef, ...]:
    photos = tuple(getattr(message, "photo", ()) or ())
    if photos:
        photo = max(
            photos,
            key=lambda row: (
                int(getattr(row, "width", 0) or 0) * int(getattr(row, "height", 0) or 0),
                int(getattr(row, "file_size", 0) or 0),
            ),
        )
        return (
            TelegramMediaRef(
                part_index=0,
                provider_ref=_provider_ref(getattr(photo, "file_id", None)),
                claimed_size_bytes=_optional_size(getattr(photo, "file_size", None)),
                claimed_mime_type="image/jpeg",
            ),
        )

    document = getattr(message, "document", None)
    if document is not None:
        return (
            TelegramMediaRef(
                part_index=0,
                provider_ref=_provider_ref(getattr(document, "file_id", None)),
                claimed_size_bytes=_optional_size(getattr(document, "file_size", None)),
                claimed_mime_type=_normalize_mime(getattr(document, "mime_type", None)),
                filename=_optional_filename(getattr(document, "file_name", None)),
            ),
        )

    voice = getattr(message, "voice", None)
    if voice is not None:
        return (
            TelegramMediaRef(
                part_index=0,
                provider_ref=_provider_ref(getattr(voice, "file_id", None)),
                claimed_size_bytes=_optional_size(getattr(voice, "file_size", None)),
                claimed_mime_type=_normalize_mime(
                    getattr(voice, "mime_type", None),
                    default="audio/ogg",
                ),
            ),
        )
    return ()


# This is a declared provider profile, not intent inference. Unknown fields
# produce an explicit unsupported outcome, including when PTB preserves them
# in api_kwargs. A new Bot API content field must never become plain text just
# because the pinned parser does not know it yet.
_CONTENT_FIELDS = frozenset({
    "text", "photo", "document", "voice", "animation", "audio", "video",
    "video_note", "sticker", "paid_media", "contact", "dice", "game", "poll",
    "venue", "location", "story", "checklist", "invoice", "passport_data",
    "giveaway", "giveaway_winners", "live_photo", "rich_message",
})
_SERVICE_FIELDS = frozenset({
    "new_chat_members", "left_chat_member", "new_chat_title", "new_chat_photo",
    "new_chat_participant", "left_chat_participant", "user_shared",
    "delete_chat_photo", "group_chat_created", "supergroup_chat_created",
    "channel_chat_created", "migrate_to_chat_id", "migrate_from_chat_id",
    "pinned_message", "successful_payment", "refunded_payment", "connected_website",
    "proximity_alert_triggered", "video_chat_started", "video_chat_ended",
    "video_chat_participants_invited", "message_auto_delete_timer_changed",
    "video_chat_scheduled", "web_app_data", "forum_topic_created", "forum_topic_closed",
    "forum_topic_reopened", "forum_topic_edited", "general_forum_topic_hidden",
    "general_forum_topic_unhidden", "write_access_allowed", "chat_shared", "users_shared",
    "giveaway_completed", "giveaway_created", "boost_added", "chat_background_set",
    "gift", "unique_gift", "gift_upgrade_sent", "paid_message_price_changed",
    "direct_message_price_changed", "checklist_tasks_done", "checklist_tasks_added",
    "suggested_post_declined", "suggested_post_paid", "suggested_post_refunded",
    "suggested_post_approved", "suggested_post_approval_failed", "chat_owner_changed",
    "chat_owner_left",
})
_METADATA_FIELDS = frozenset({
    "message_id", "date", "chat", "from", "reply_to_message", "edit_date",
    "entities", "caption", "caption_entities", "author_signature", "media_group_id",
    "reply_markup", "via_bot", "sender_chat", "is_automatic_forward",
    "has_protected_content", "is_topic_message", "message_thread_id", "has_media_spoiler",
    "link_preview_options", "external_reply", "quote", "forward_origin", "reply_to_story",
    # PTB 22.7 retains these legacy forward fields alongside forward_origin.
    # Provenance is not the current sender and never crosses the host envelope.
    "forward_from", "forward_from_chat", "forward_from_message_id",
    "forward_signature", "forward_sender_name", "forward_date",
    "sender_boost_count", "business_connection_id", "sender_business_bot", "is_from_offline",
    "effect_id", "show_caption_above_media", "paid_star_count", "is_paid_post",
    "direct_messages_topic", "reply_to_checklist_task_id", "suggested_post_info", "sender_tag",
})
_SERVICE_FLAGS = frozenset({
    "delete_chat_photo", "group_chat_created", "supergroup_chat_created", "channel_chat_created",
})


def _message_reason(
    payload: JsonObject, raw_message: Mapping[str, Any], message: Any, *, kind: str = "message",
) -> Literal["service_message", "content_type", "unknown_fields", "sender_kind"] | None:
    for key in ("is_automatic_forward", "is_from_offline"):
        if key in raw_message and type(raw_message[key]) is not bool:
            raise _adapter_error("invalid_request", "Telegram sender marker is malformed")
    raw_sender = raw_message.get("from")
    if isinstance(raw_sender, Mapping) and type(raw_sender.get("is_bot")) is not bool:
        raise _adapter_error("invalid_request", "Telegram sender marker is malformed")
    content = {key for key in _CONTENT_FIELDS if raw_message.get(key) is not None}
    services = {key for key in _SERVICE_FIELDS if raw_message.get(key) is not None}
    for key in content | services:
        value = raw_message[key]
        if key == "text":
            valid = isinstance(value, str)
        elif key in {"new_chat_title", "connected_website"}:
            valid = isinstance(value, str) and bool(value)
        elif key in {"photo", "new_chat_photo", "new_chat_members"}:
            valid = isinstance(value, list) and bool(value) and all(
                isinstance(item, dict) for item in value
            )
        elif key in _SERVICE_FLAGS:
            valid = value is True
        elif key in {"migrate_to_chat_id", "migrate_from_chat_id"}:
            valid = isinstance(value, int) and not isinstance(value, bool) and value != 0
        else:
            valid = isinstance(value, dict) and (bool(value) or key in services)
        if not valid:
            raise _adapter_error("invalid_request", "Telegram message content is malformed")
    # Telegram deliberately includes these backward-compatible aliases. They
    # describe one unsupported content type, not two contradictory messages.
    for primary, alias in (("animation", "document"), ("venue", "location"),
                           ("live_photo", "photo")):
        if primary in content:
            content.discard(alias)
    caption = raw_message.get("caption")
    text = raw_message.get("text")
    if (caption is not None and not isinstance(caption, str)) or (
        text is not None and not isinstance(text, str)
    ):
        raise _adapter_error("invalid_request", "Telegram message text is malformed")
    if len(content) > 1 or (services and (content or caption is not None)) or (
        "text" in content and caption is not None
    ):
        raise _adapter_error("invalid_request", "Telegram message content is contradictory")
    if (set(raw_message) - _CONTENT_FIELDS - _SERVICE_FIELDS - _METADATA_FIELDS
            or set(payload) - {"update_id", kind}):
        return "unknown_fields"
    if caption is not None and not content.intersection({
        "animation", "audio", "document", "paid_media", "photo", "video", "voice", "live_photo",
    }):
        raise _adapter_error("invalid_request", "Telegram caption has no supported media owner")
    if services:
        return "service_message"
    if not content:
        raise _adapter_error("invalid_request", "Telegram message content is absent")
    if text == "":
        return "content_type"
    if content - {"text", "photo", "document", "voice"}:
        return "content_type"
    if any(raw_message.get(key) is not None for key in (
        "sender_chat", "business_connection_id", "sender_business_bot", "direct_messages_topic",
    )) or raw_message.get("is_automatic_forward") or raw_message.get("is_from_offline"):
        return "sender_kind"
    user = getattr(message, "from_user", None)
    if user is None or getattr(user, "is_bot", False):
        return "sender_kind"
    return None


def normalize_update(payload: JsonObject) -> TelegramEvent | TelegramUnsupportedEvent:
    """Interpret a message without downloading media, granting access or ACKing it."""

    if not isinstance(payload, dict):
        raise _adapter_error("invalid_request", "Telegram update must be an object")
    parser_unavailable = False
    try:
        from telegram import Update
    except Exception:
        parser_unavailable = True
    if parser_unavailable:
        raise _adapter_error("runtime_not_ready", "Telegram optional parser is unavailable")
    if sum(payload.get(kind) is not None for kind in Update.ALL_TYPES) > 1:
        raise _adapter_error("invalid_request", "Telegram update variants are contradictory")
    kind = "edited_message" if payload.get("edited_message") is not None else "message"
    malformed = False
    try:
        provider_payload = deepcopy(payload)
        provider_message = provider_payload.get(kind)
        if isinstance(provider_message, dict):
            # Only the referenced message id crosses the host envelope.
            # Avoid recursively parsing a large reply body, and accept the
            # bounded provider recordings that retain only that stable id.
            provider_message.pop("reply_to_message", None)
        update = Update.de_json(provider_payload, None)
    except Exception:
        malformed = True
    if malformed:
        # Raise after leaving the handler so parser diagnostics are not retained
        # in either the explicit cause or the implicit exception context.
        raise _adapter_error("invalid_request", "Telegram update is malformed")

    update_id = getattr(update, "update_id", None)
    message = getattr(update, kind, None)
    user = getattr(update, "effective_user", None)
    chat = getattr(update, "effective_chat", None)
    if (
        not isinstance(update_id, int)
        or isinstance(update_id, bool)
        or update_id < 0
        or message is None
        or chat is None
    ):
        raise _adapter_error("invalid_request", "Telegram update is not a message update")

    message_id = getattr(message, "message_id", None)
    sender_id = getattr(user, "id", None)
    chat_id = getattr(chat, "id", None)
    date = getattr(message, "date", None)
    if (
        not isinstance(message_id, int)
        or isinstance(message_id, bool)
        or (user is not None and (not isinstance(sender_id, int) or isinstance(sender_id, bool)))
        or not isinstance(chat_id, int)
        or isinstance(chat_id, bool)
        or date is None
    ):
        raise _adapter_error("invalid_request", "Telegram message identity is malformed")

    chat_type = str(getattr(chat, "type", ""))
    is_group = chat_type in {"group", "supergroup"}
    raw_message = payload.get(kind)
    if not isinstance(raw_message, Mapping):
        raise _adapter_error("invalid_request", "Telegram message update is malformed")
    reason = _message_reason(payload, raw_message, message, kind=kind)
    raw_reply = raw_message.get("reply_to_message") if isinstance(raw_message, Mapping) else None
    reply_message_id = raw_reply.get("message_id") if isinstance(raw_reply, Mapping) else None
    thread_id = getattr(message, "message_thread_id", None)
    sender_name = str(getattr(user, "full_name", None) or getattr(user, "username", None) or "")
    text = getattr(message, "text", None) or getattr(message, "caption", None) or ""
    if not isinstance(text, str):
        raise _adapter_error("invalid_request", "Telegram message text is malformed")

    envelope: JsonObject = {
        "provider": "telegram",
        "provider_event_id": update_id,
        "provider_message_id": message_id,
        "sender_id": str(sender_id) if sender_id is not None else None,
        "sender_name": sender_name,
        "recipient_or_chat_id": str(chat_id),
        "group_id": str(chat_id) if is_group else None,
        "thread_id": str(thread_id) if thread_id is not None else None,
        "reply_to_message_id": (str(reply_message_id) if reply_message_id is not None else None),
        "text": text,
        "timestamp": int(date.timestamp()),
    }
    if kind == "edited_message":
        # Edits are never replacement turns. Retain only rejection/routing facts,
        # not the changed text, captions, reply body or downloadable media.
        envelope["text"] = ""
        envelope["chat_type"] = chat_type
        raw_sender = raw_message.get("from")
        envelope["sender_is_bot"] = (
            raw_sender.get("is_bot") if isinstance(raw_sender, Mapping) else None
        )
        if reason in (None, "content_type"):
            human = envelope["sender_is_bot"] is False and not any(
                raw_message.get(key) is not None for key in (
                    "sender_chat", "business_connection_id", "sender_business_bot",
                    "direct_messages_topic",
                )
            ) and not raw_message.get("is_automatic_forward") and not raw_message.get("is_from_offline")
            reason = "message_edit" if human else "sender_kind"
        return TelegramUnsupportedEvent(update_id, reason, envelope)
    if reason is not None:
        return TelegramUnsupportedEvent(update_id, reason, envelope)
    media = _telegram_media(message)
    if (not text or raw_message.get("text") is None) and not media:
        raise _adapter_error("invalid_request", "Telegram message content is absent")
    return TelegramEvent(
        update_id=update_id,
        envelope=envelope,
        media=media,
    )


def normalize_mime_type(value: object, *, default: str | None = None) -> str | None:
    """Preserve the donor media profile; this does not authorize a media type."""
    return _normalize_mime(value, default=default)


def poll_cursor_offset(cursor: str | None) -> int | None:
    if cursor is None:
        return None
    if not isinstance(cursor, str) or not cursor.isascii() or not cursor.isdecimal():
        raise _adapter_error("invalid_request", "Telegram polling cursor is invalid")
    return int(cursor)


def interpret_poll_page(updates: Sequence[JsonObject], cursor: str | None) -> TelegramPage:
    """Interpret one complete response without saving a cursor or admitting input.

    Other update kinds are counted as ignored. Valid unsupported messages/edits are
    explicit ordered outcomes, so they cannot starve later supported messages.
    A malformed message refuses the whole page, including any earlier good row.
    """
    poll_cursor_offset(cursor)
    if not isinstance(updates, (tuple, list)):
        raise _adapter_error("invalid_request", "Telegram updates page is invalid")
    if not updates:
        return TelegramPage((), cursor)
    for item in updates:
        if not isinstance(item, Mapping):
            raise _adapter_error("invalid_request", "Telegram update is invalid")
        value = item.get("update_id")
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            raise _adapter_error("invalid_request", "Telegram update id is invalid")
        for kind in ("message", "edited_message"):
            if kind in item and not isinstance(item[kind], Mapping):
                raise _adapter_error("invalid_request", "Telegram message update is malformed")
    events = tuple(
        normalize_update(item) for item in updates
        if "message" in item or "edited_message" in item
    )
    return TelegramPage(
        events, str(max(item["update_id"] for item in updates) + 1), len(updates) - len(events)
    )


def validate_media_request(ref: TelegramMediaRef, max_bytes: int) -> None:
    """Validate before a host resolves credentials or constructs a transport."""
    if not isinstance(ref, TelegramMediaRef) or ref.part_index < 0:
        raise _adapter_error("invalid_request", "Telegram media reference is invalid")
    _provider_ref(ref.provider_ref)
    _optional_filename(ref.filename)
    if not isinstance(max_bytes, int) or isinstance(max_bytes, bool) or max_bytes <= 0:
        raise _adapter_error("invalid_request", "Telegram media byte limit is invalid")


async def fetch_media(
    transport: TelegramTransport,
    ref: TelegramMediaRef,
    max_bytes: int,
    *,
    passthrough_errors: tuple[type[Exception], ...] = (),
) -> TelegramFetchedMedia:
    """Fetch one host-authorized provider reference through a borrowed transport.

    Only host-selected exception classes pass through unchanged. Their content
    is the host's declared error contract, never inferred from a diagnostic cause.
    All other provider errors become bounded failures without raw diagnostics.
    """
    validate_media_request(ref, max_bytes)
    if not isinstance(passthrough_errors, tuple) or any(
        not isinstance(kind, type) or not issubclass(kind, Exception) for kind in passthrough_errors
    ):
        raise _adapter_error("invalid_request", "Telegram host error contract is invalid")
    failure = None
    try:
        descriptor = await transport.get_file(ref.provider_ref)
        file_path = descriptor.get("file_path")
        if not isinstance(file_path, str) or not file_path:
            raise TelegramTransportError("Telegram file path is absent")
        data = await transport.download_file(file_path, max_bytes)
    except passthrough_errors:
        raise
    except TelegramMediaTooLarge:
        failure = _adapter_error(
            "invalid_request", "Telegram media exceeds the configured byte limit"
        )
    except Exception:
        failure = _adapter_error("provider_error", "Telegram media fetch failed", retryable=True)
    if failure is not None:
        raise failure
    mime_type = _normalize_mime(ref.claimed_mime_type)
    if mime_type is None:
        mime_type = _normalize_mime(
            mimetypes.guess_type(ref.filename or file_path)[0], default="application/octet-stream"
        )
    return TelegramFetchedMedia(data, cast(str, mime_type))


class TelegramPresenter:
    """Existing safe-formatting profile and diagnostics; no send authority/store.

    Hosts supply already-authorized recipients, topics, bytes and limits, and own the
    send lifetime. Each method makes one provider attempt. A complete explicit
    formatted-request refusal can advise a plain candidate for the delivery
    owner to authorize separately. This presenter never retries a send.
    """

    def __init__(self) -> None:
        self.entity_strip_fallbacks = 0
        self.entity_wire_rejections = 0

    def _presentation_receipt(
        self, response: ProviderResponse, *, had_formatting: bool,
        recipient: str, message_thread_id: int | None,
    ) -> TelegramSendReceipt:
        receipt = classify_send_response(
            response,
            expected_recipient=recipient,
            expected_message_thread_id=message_thread_id,
        )
        if had_formatting and receipt.outcome == "known_not_sent" and receipt.error_code == 400:
            self.entity_wire_rejections += 1
            return replace(receipt, retry_without_formatting=True)
        return receipt

    async def send_text(
        self,
        transport: TelegramTransport,
        *,
        recipient: str,
        text: str,
        entities: object = (),
        message_thread_id: int | None = None,
    ) -> TelegramSendReceipt:
        """Make one provider attempt; return plain-presentation advice on refusal."""
        _validate_message_thread_id(message_thread_id)
        # Omit the keyword entirely for legacy borrowed transports when the
        # host did not select a topic. Keep the same selection on every attempt.
        topic = {} if message_thread_id is None else {"message_thread_id": message_thread_id}
        checked = _valid_entities(entities, text)
        if checked is None:
            self.entity_strip_fallbacks += 1
            checked = ()
        try:
            response = await transport.send_message(
                recipient=recipient, text=text, entities=checked or None, **topic
            )
        except Exception:
            return TelegramSendReceipt("unknown", error="Telegram outcome unknown after dispatch")
        return self._presentation_receipt(
            response,
            had_formatting=bool(checked), recipient=recipient,
            message_thread_id=message_thread_id,
        )

    async def send_photo(
        self,
        transport: TelegramTransport,
        *,
        recipient: str,
        data: bytes,
        mime_type: str,
        caption: str | None = None,
        caption_entities: object = (),
        message_thread_id: int | None = None,
    ) -> TelegramSendReceipt:
        """Make one provider attempt; media custody remains with the host."""
        _validate_message_thread_id(message_thread_id)
        topic = {} if message_thread_id is None else {"message_thread_id": message_thread_id}
        checked: tuple[dict[str, Any], ...] = ()
        if caption:
            checked = _valid_entities(caption_entities, caption) or ()
            if not checked and caption_entities:
                self.entity_strip_fallbacks += 1
        try:
            response = await transport.send_photo(
                recipient=recipient,
                data=data,
                mime_type=mime_type,
                caption=caption,
                caption_entities=checked or None,
                **topic,
            )
        except Exception:
            return TelegramSendReceipt("unknown", error="Telegram outcome unknown after dispatch")
        return self._presentation_receipt(
            response,
            had_formatting=bool(checked), recipient=recipient,
            message_thread_id=message_thread_id,
        )
