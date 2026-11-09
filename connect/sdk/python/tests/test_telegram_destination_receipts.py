"""Destination-bound provider observations, without provider access."""

from __future__ import annotations

import asyncio
from copy import deepcopy

import pytest

from meerkat_toolkit.frontend.telegram import ProviderResponse, classify_send_response
from meerkat_toolkit.frontend.telegram_adapter import TelegramPresenter


def success(*, chat=-99, topic=12, message=7):
    return ProviderResponse(200, {"ok": True, "result": {
        "message_id": message, "chat": {"id": chat}, "message_thread_id": topic,
    }})


def bound(response, recipient="-99", topic=12):
    return classify_send_response(
        response, expected_recipient=recipient, expected_message_thread_id=topic,
    )


def test_unbound_classifier_retains_legacy_envelope_only_behavior():
    for response in [success(chat=-100, topic=13), ProviderResponse(200, {
        "ok": True, "result": {"message_id": 7},
    })]:
        assert classify_send_response(response).outcome == "provider_accepted"
        assert bound(response).outcome == "unknown"


def test_numeric_chat_and_selected_topic_match_actual_message():
    response = success()
    original = deepcopy(response.body)
    for recipient in ["-99", "-099"]:
        receipt = bound(response, recipient)
        assert (receipt.outcome, receipt.provider_message_id) == ("provider_accepted", "7")
    assert response.body == original
    assert bound(success(chat=4503599627370495), "4503599627370495").outcome == "provider_accepted"


@pytest.mark.parametrize("change", [
    {}, {"chat": None}, {"chat": {}}, {"chat": {"id": True}},
    {"chat": {"id": "-99"}}, {"chat": {"id": -99.0}}, {"chat": {"id": 0}},
    {"chat": {"id": -100}}, {"chat": {"id": 2**52}},
    {"chat": {"id": -99}, "message_thread_id": None},
    {"chat": {"id": -99}, "message_thread_id": True},
    {"chat": {"id": -99}, "message_thread_id": "12"},
    {"chat": {"id": -99}, "message_thread_id": 0},
    {"chat": {"id": -99}, "message_thread_id": 13},
    {"chat": {"id": -99}, "message_thread_id": 2**31},
])
def test_missing_malformed_or_conflicting_destination_is_unknown(change):
    receipt = bound(ProviderResponse(200, {
        "ok": True, "result": {"message_id": 7, **change},
    }))
    assert receipt.outcome == "unknown"
    assert receipt.provider_message_id is None
    assert receipt.error_code is None


def test_no_selected_topic_allows_absence_or_a_valid_provider_default():
    response = success(topic=19)
    assert bound(response, topic=None).outcome == "provider_accepted"
    del response.body["result"]["message_thread_id"]
    assert bound(response, topic=None).outcome == "provider_accepted"
    assert bound(response).outcome == "unknown"
    response.body["result"]["message_thread_id"] = None
    assert bound(response, topic=None).outcome == "unknown"


def test_alias_needs_exact_observed_username_and_never_guesses_from_id():
    response = success()
    assert bound(response, "@fixture_chat").outcome == "unknown"
    response.body["result"]["chat"]["username"] = "fixture_chat"
    assert bound(response, "@fixture_chat").outcome == "provider_accepted"
    for recipient in ["@Fixture_chat", "@another_chat", "fixture_chat", "-99x", "-99\n"]:
        assert bound(response, recipient).outcome == "unknown"
    assert bound(response).outcome == "provider_accepted"
    response.body["result"]["chat"]["id"] = False
    assert bound(response, "@fixture_chat").outcome == "unknown"


def test_queued_result_still_requires_destination_evidence_and_does_not_refuse():
    assert bound(success(message=0)).outcome == "provider_queued"
    assert bound(success(message=0, topic=13)).outcome == "unknown"
    assert bound(success(message=0, chat=-100)).outcome == "unknown"
    assert bound(success(), topic=True).outcome == "unknown"


class Transport:
    def __init__(self, responses):
        self.responses = list(responses)
        self.calls = []

    async def send_message(self, **kwargs):
        self.calls.append(kwargs)
        return self.responses.pop(0)

    async def send_photo(self, **kwargs):
        return await self.send_message(**kwargs)


async def send(presenter, transport, photo=False, *, formatted=True):
    arguments = {"recipient": "-99", "message_thread_id": 12}
    entities = ({"type": "bold", "offset": 0, "length": 5},) if formatted else ()
    if photo:
        return await presenter.send_photo(
            transport, **arguments, data=b"photo", mime_type="image/png",
            caption="hello", caption_entities=entities,
        )
    return await presenter.send_text(transport, **arguments, text="hello", entities=entities)


@pytest.mark.asyncio
@pytest.mark.parametrize("photo", [False, True])
@pytest.mark.parametrize("fallback", [False, True])
async def test_conflicting_destination_never_causes_an_extra_send(photo, fallback):
    responses = [success(chat=-100)]
    if fallback:
        responses.insert(0, ProviderResponse(400, {
            "ok": False, "error_code": 400, "description": "bad entities",
        }))
    transport = Transport(responses)
    presenter = TelegramPresenter()
    receipt = await send(presenter, transport, photo)
    if fallback:
        # Advice is not a second send. An explicit caller-selected next
        # attempt still has to validate its own destination receipt.
        assert receipt.outcome == "known_not_sent"
        assert receipt.retry_without_formatting is True
        assert len(transport.calls) == 1
        receipt = await send(presenter, transport, photo, formatted=False)
    assert receipt.outcome == "unknown"
    assert receipt.retry_without_formatting is False
    assert len(transport.calls) == 1 + int(fallback)
    assert presenter.entity_wire_rejections == int(fallback)
    assert all(call["recipient"] == "-99" and call["message_thread_id"] == 12
               for call in transport.calls)


@pytest.mark.asyncio
@pytest.mark.parametrize("photo", [False, True])
async def test_refusal_with_a_result_cannot_start_formatting_fallback(photo):
    transport = Transport([ProviderResponse(400, {
        "ok": False, "error_code": 400, "description": "bad entities",
        "result": success(chat=-100).body["result"],
    })])
    presenter = TelegramPresenter()
    assert (await send(presenter, transport, photo)).outcome == "unknown"
    assert len(transport.calls) == 1
    assert presenter.entity_wire_rejections == 0


@pytest.mark.asyncio
@pytest.mark.parametrize("photo", [False, True])
async def test_missing_bound_receipt_does_not_weaken_for_legacy_transport(photo):
    class LegacyTransport:
        async def send_message(self, *, recipient, text, entities=None):
            return ProviderResponse(200, {"ok": True, "result": {"message_id": 7}})

        async def send_photo(self, *, recipient, data, mime_type, caption, caption_entities=None):
            return ProviderResponse(200, {"ok": True, "result": {"message_id": 7}})

    presenter = TelegramPresenter()
    if photo:
        receipt = await presenter.send_photo(
            LegacyTransport(), recipient="-99", data=b"photo", mime_type="image/png",
        )
    else:
        receipt = await presenter.send_text(LegacyTransport(), recipient="-99", text="hello")
    assert receipt.outcome == "unknown"


@pytest.mark.asyncio
async def test_interleaved_replies_keep_each_calls_expected_destination():
    entered = asyncio.Event()
    release = asyncio.Event()

    class Interleaved:
        async def send_message(self, **kwargs):
            entered.set()
            await release.wait()
            return success()

        async def send_photo(self, **kwargs):
            return success(chat=-100, topic=13)

    presenter = TelegramPresenter()
    transport = Interleaved()
    first = asyncio.create_task(send(presenter, transport))
    await entered.wait()
    second = await presenter.send_photo(
        transport, recipient="-100", message_thread_id=13, data=b"photo", mime_type="image/png",
    )
    release.set()
    assert (await first).outcome == second.outcome == "provider_accepted"


@pytest.mark.asyncio
@pytest.mark.parametrize("photo", [False, True])
async def test_cancellation_during_entered_send_never_becomes_fallback(photo):
    entered = asyncio.Event()

    class Blocked(Transport):
        async def send_message(self, **kwargs):
            self.calls.append(kwargs)
            entered.set()
            await asyncio.Event().wait()

    transport = Blocked([])
    presenter = TelegramPresenter()
    task = asyncio.create_task(send(presenter, transport, photo))
    await entered.wait()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert len(transport.calls) == 1
    assert presenter.entity_wire_rejections == 0
