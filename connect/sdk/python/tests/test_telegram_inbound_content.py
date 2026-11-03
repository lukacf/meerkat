"""Provider content must survive interpretation or have an explicit outcome."""

from copy import deepcopy

import pytest

from meerkat_toolkit.frontend.telegram_adapter import (
    TelegramAdapterError,
    TelegramEvent,
    TelegramUnsupportedEvent,
    interpret_poll_page,
    normalize_update,
)


def update(identifier=41, **content):
    return {"update_id": identifier, "message": {
        "message_id": 7, "date": 1700000000,
        "chat": {"id": -99, "type": "supergroup", "title": "fixture"},
        "from": {"id": 3, "is_bot": False, "first_name": "Alex"},
        "message_thread_id": 12, **content,
    }}


def file(**fields):
    return {"file_id": "opaque", "file_unique_id": "unique", **fields}


VIDEO = file(width=10, height=10, duration=2)
AUDIO = file(duration=2)
PHOTO = [file(width=10, height=10)]
STICKER = file(width=10, height=10, type="regular", is_animated=False, is_video=False)


_FORWARD_DATE = 1600000000
_FORWARD_USER = {"id": 90, "is_bot": False, "first_name": "Original sender"}
_FORWARD_CHAT = {"id": -900, "type": "supergroup", "title": "Original group"}
_FORWARD_CHANNEL = {"id": -901, "type": "channel", "title": "Original school"}
FORWARDS = [
    ({"type": "user", "date": _FORWARD_DATE, "sender_user": _FORWARD_USER},
     {"forward_from": _FORWARD_USER, "forward_date": _FORWARD_DATE}),
    ({"type": "hidden_user", "date": _FORWARD_DATE, "sender_user_name": "Original sender"},
     {"forward_sender_name": "Original sender", "forward_date": _FORWARD_DATE}),
    ({"type": "chat", "date": _FORWARD_DATE, "sender_chat": _FORWARD_CHAT,
      "author_signature": "Original author"},
     {"forward_from_chat": _FORWARD_CHAT, "forward_signature": "Original author",
      "forward_date": _FORWARD_DATE}),
    ({"type": "channel", "date": _FORWARD_DATE, "chat": _FORWARD_CHANNEL,
      "message_id": 87, "author_signature": "Original author"},
     {"forward_from_chat": _FORWARD_CHANNEL, "forward_from_message_id": 87,
      "forward_signature": "Original author", "forward_date": _FORWARD_DATE}),
]


@pytest.mark.parametrize("origin,legacy", FORWARDS,
                         ids=["user", "hidden-user", "chat", "channel"])
@pytest.mark.parametrize("content", [
    {"text": "School notice"}, {"photo": PHOTO, "caption": "School photo notice"},
], ids=["text", "photo-caption"])
def test_legacy_forward_metadata_preserves_content_and_current_sender(origin, legacy, content):
    from telegram import Update

    payload = update(forward_origin=origin, **legacy, **content)
    before = deepcopy(payload)
    parsed = Update.de_json(deepcopy(payload), None)
    assert parsed.message.forward_origin.type == origin["type"]
    assert parsed.message.from_user.id == 3
    event = normalize_update(payload)
    assert isinstance(event, TelegramEvent)
    assert event == normalize_update(update(**content))
    assert event.envelope["sender_id"] == "3"
    assert event.envelope["recipient_or_chat_id"] == "-99"
    assert event.envelope["provider_message_id"] == 7
    assert payload == before


def test_forward_metadata_does_not_tolerate_arbitrary_unknown_fields():
    origin, legacy = FORWARDS[0]
    payload = update(text="School notice", forward_origin=origin, **legacy,
                     future_provider_content={"untrusted": "not part of the supported envelope"})
    event = normalize_update(payload)
    assert isinstance(event, TelegramUnsupportedEvent)
    assert event.reason == "unknown_fields"
    assert "untrusted" not in repr(event)


@pytest.mark.parametrize("content", [
    {"video": VIDEO, "caption": "Forwarded video"},
    {"audio": AUDIO, "caption": "Forwarded audio"},
    {"animation": VIDEO, "document": file(mime_type="video/mp4"), "caption": "Forwarded GIF"},
])
def test_forward_metadata_does_not_promote_unsupported_media(content):
    origin, legacy = FORWARDS[0]
    event = normalize_update(update(forward_origin=origin, **legacy, **content))
    assert isinstance(event, TelegramUnsupportedEvent)
    assert event.reason == "content_type"
    assert not hasattr(event, "envelope") and not hasattr(event, "media")
    assert event.facts["text"] == content["caption"]


@pytest.mark.parametrize("sender", ["bot", "automatic"])
def test_forward_provenance_never_authorizes_the_current_sender(sender):
    origin, legacy = FORWARDS[0]
    payload = update(text="School notice", forward_origin=origin, **legacy)
    if sender == "bot":
        payload["message"]["from"]["is_bot"] = True
    else:
        payload["message"]["is_automatic_forward"] = True
    event = normalize_update(payload)
    assert isinstance(event, TelegramUnsupportedEvent)
    assert event.reason == "sender_kind"


@pytest.mark.parametrize("content", [
    {"video": VIDEO}, {"audio": AUDIO}, {"sticker": STICKER},
    {"video": VIDEO, "caption": "Summarize this video"},
    {"audio": AUDIO, "caption": "Listen to this"},
    {"animation": VIDEO, "document": file(mime_type="video/mp4")},
    {"venue": {"location": {"latitude": 1, "longitude": 2}, "title": "a", "address": "b"},
     "location": {"latitude": 1, "longitude": 2}},
    {"live_photo": {"photo": PHOTO, "video": VIDEO}, "photo": PHOTO},
    {"text": ""},
])
def test_unsupported_content_is_never_a_partial_conversation(content):
    payload = update(**content)
    before = deepcopy(payload)
    event = normalize_update(payload)
    assert isinstance(event, TelegramUnsupportedEvent)
    assert event.reason == "content_type"
    assert not hasattr(event, "envelope") and not hasattr(event, "media")
    assert event.facts["text"] == content.get("caption", "")
    assert event.facts["thread_id"] == "12"
    assert payload == before


@pytest.mark.parametrize("content", [
    {"new_chat_title": "renamed"}, {"group_chat_created": True},
    {"forum_topic_closed": {}},
    {"web_app_data": {"data": "untrusted tool output", "button_text": "Run"}},
    {"user_shared": {"request_id": 5, "user_id": 90}},
    {"new_chat_participant": _FORWARD_USER},
    {"left_chat_participant": _FORWARD_USER},
])
@pytest.mark.parametrize("sender", [True, False])
def test_service_messages_are_explicit_even_without_a_sender(content, sender):
    payload = update(**content)
    if not sender:
        del payload["message"]["from"]
    event = normalize_update(payload)
    assert isinstance(event, TelegramUnsupportedEvent)
    assert event.reason == "service_message"
    assert event.facts["sender_id"] == ("3" if sender else None)
    assert event.facts["text"] == ""


@pytest.mark.parametrize("content", [
    {"photo": PHOTO, "video": VIDEO}, {"text": "hello", "sticker": STICKER},
    {"text": "hello", "new_chat_title": "renamed"},
    {"text": "hello", "caption": "different"},
    {"photo": [], "caption": "lost photo"}, {"document": {}, "caption": "lost document"},
    {"caption": "no attachment"}, {"sticker": STICKER, "caption": "no caption support"},
    {"video": {}}, {"group_chat_created": False},
    {"group_chat_created": 1}, {"new_chat_members": []}, {},
])
def test_malformed_or_contradictory_message_refuses_whole_page(content):
    with pytest.raises(TelegramAdapterError) as raised:
        interpret_poll_page([update(text="valid"), update(42, **content)], "40")
    assert raised.value.code == "invalid_request"
    assert raised.value.__cause__ is None and raised.value.__context__ is None


def test_multiple_known_update_variants_are_malformed():
    payload = update(text="hello")
    payload["edited_message"] = deepcopy(payload["message"])
    with pytest.raises(TelegramAdapterError, match="variants"):
        normalize_update(payload)


def test_nested_reply_service_content_does_not_change_the_outer_message():
    payload = update(text="what happened?")
    payload["message"]["reply_to_message"] = {
        "message_id": 4, "new_chat_title": "unparsed nested service message",
    }
    event = normalize_update(payload)
    assert isinstance(event, TelegramEvent)
    assert event.envelope["reply_to_message_id"] == "4"
    assert event.envelope["text"] == "what happened?"


@pytest.mark.parametrize("position", ["message", "update"])
def test_unknown_fields_remain_explicit_without_exposing_raw_values(position):
    payload = update(text="keep these words")
    target = payload["message"] if position == "message" else payload
    target["future_provider_content"] = {"secret": "not-a-real-credential"}
    event = normalize_update(payload)
    assert isinstance(event, TelegramUnsupportedEvent)
    assert event.reason == "unknown_fields" and event.facts["text"] == "keep these words"
    assert "future_provider_content" not in repr(event)
    assert "not-a-real-credential" not in repr(event)


@pytest.mark.parametrize("sender", ["absent", "bot", "channel", "business"])
def test_unrepresented_sender_never_uses_a_fake_human_identity(sender):
    payload = update(text="hello")
    if sender == "absent":
        del payload["message"]["from"]
    elif sender == "bot":
        payload["message"]["from"]["is_bot"] = True
    elif sender == "channel":
        payload["message"]["sender_chat"] = {"id": -100, "type": "channel", "title": "c"}
    else:
        payload["message"]["business_connection_id"] = "business"
    event = normalize_update(payload)
    assert isinstance(event, TelegramUnsupportedEvent) and event.reason == "sender_kind"


@pytest.mark.parametrize("field", ["is_automatic_forward", "is_from_offline", "is_bot"])
@pytest.mark.parametrize("invalid", ["", "false", 0, 1, [], {}, None])
def test_sender_flags_are_exact_booleans(field, invalid):
    payload = update(text="hello")
    target = payload["message"]["from"] if field == "is_bot" else payload["message"]
    target[field] = invalid
    with pytest.raises(TelegramAdapterError) as raised:
        normalize_update(payload)
    assert raised.value.code == "invalid_request"


@pytest.mark.parametrize("field", ["is_automatic_forward", "is_from_offline", "is_bot"])
@pytest.mark.parametrize("value", [False, True])
def test_sender_flag_values_are_not_inferred_from_truthiness(field, value):
    payload = update(text="hello")
    target = payload["message"]["from"] if field == "is_bot" else payload["message"]
    target[field] = value
    event = normalize_update(payload)
    assert isinstance(event, TelegramUnsupportedEvent if value else TelegramEvent)


def test_page_preserves_order_and_exposes_all_outcomes_before_cursor_commit():
    page = interpret_poll_page([
        update(text="first"), update(42, video=VIDEO, caption="watch"),
        update(43, new_chat_title="renamed"), update(44, text="last"),
        {"update_id": 45, "callback_query": {}},
    ], "40")
    assert [type(row) for row in page.events] == [
        TelegramEvent, TelegramUnsupportedEvent, TelegramUnsupportedEvent, TelegramEvent,
    ]
    assert [row.update_id for row in page.events] == [41, 42, 43, 44]
    assert page.next_cursor == "46" and page.ignored_updates == 1


def edited_update(**content):
    payload = update(**content)
    message = payload.pop("message")
    message["chat"] = {"id": 3, "type": "private"}
    message.pop("message_thread_id")
    message["edit_date"] = 1700000010
    payload["edited_message"] = message
    return payload


def test_private_edit_is_content_free_unsupported_outcome_before_cursor():
    payload = edited_update(text="private correction must not be dispatched")
    before = deepcopy(payload)
    page = interpret_poll_page([payload], "40")
    assert len(page.events) == 1
    event = page.events[0]
    assert isinstance(event, TelegramUnsupportedEvent)
    assert event.reason == "message_edit"
    assert event.facts["chat_type"] == "private"
    assert event.facts["sender_is_bot"] is False
    assert event.facts["provider_event_id"] == 41
    assert event.facts["provider_message_id"] == 7
    assert event.facts["sender_id"] == event.facts["recipient_or_chat_id"] == "3"
    assert event.facts["text"] == ""
    assert not hasattr(event, "media")
    assert page.next_cursor == "42" and page.ignored_updates == 0
    assert payload == before


@pytest.mark.parametrize("change,reason", [
    ({"from": {"id": 3, "is_bot": True, "first_name": "Bot"}}, "sender_kind"),
    ({"sender_chat": {"id": -99, "type": "channel", "title": "Channel"}}, "sender_kind"),
    ({"future_field": "private"}, "unknown_fields"),
])
def test_edit_does_not_launder_foreign_sender_or_unknown_content(change, reason):
    event = normalize_update(edited_update(text="private", **change))
    assert isinstance(event, TelegramUnsupportedEvent)
    assert event.reason == reason
    assert event.facts["text"] == ""


@pytest.mark.parametrize("content, mime", [
    ({"photo": PHOTO, "caption": ""}, "image/jpeg"),
    ({"document": file(mime_type="video/mp4"), "caption": "as a document"}, "video/mp4"),
    ({"voice": AUDIO, "caption": "voice"}, "audio/ogg"),
])
def test_existing_media_profile_keeps_caption_and_reference(content, mime):
    event = normalize_update(update(**content))
    assert isinstance(event, TelegramEvent)
    assert event.envelope["text"] == content["caption"]
    assert event.media[0].provider_ref == "opaque"
    assert event.media[0].claimed_mime_type == mime
