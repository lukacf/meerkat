"""Telegram wire observations and thread custody against real loopback HTTP."""

from __future__ import annotations

import asyncio
from contextlib import contextmanager
from dataclasses import asdict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from email.parser import BytesParser
from email.policy import default
import socket
import threading
import traceback
from urllib.error import URLError

import pytest

from meerkat_toolkit.frontend import telegram
from meerkat_toolkit.frontend.telegram import (
    ProviderResponse,
    TelegramBotIdentity,
    TelegramBotApiTransport,
    TelegramMediaTooLarge,
    TelegramTransportError,
    TelegramWebhookInfo,
    classify_send_response,
)


@contextmanager
def server(respond):
    calls = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            calls.append((self.path, body, dict(self.headers)))
            respond(self, body)

        do_GET = do_POST

    service = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=service.serve_forever, daemon=True)
    worker.start()
    try:
        yield f"http://127.0.0.1:{service.server_port}", calls
    finally:
        service.shutdown()
        service.server_close()
        worker.join(timeout=3)
        assert not worker.is_alive()


def reply(handler, status=200, body=None, headers=()):
    raw = body if isinstance(body, bytes) else json.dumps(body).encode()
    handler.send_response(status)
    handler.send_header("Content-Length", str(len(raw)))
    for name, value in headers:
        handler.send_header(name, value)
    handler.end_headers()
    try:
        handler.wfile.write(raw)
    except (BrokenPipeError, ConnectionResetError):
        pass


def local_transport(origin, token="123:fixture-secret"):
    transport = TelegramBotApiTransport(token)
    # Deliberate test-only override. Production has fixed HTTPS origins and no
    # endpoint discovery/configuration or credentials in test requests.
    transport._api_base = f"{origin}/bot{token}/"
    transport._file_base = f"{origin}/file/bot{token}/"
    return transport


@pytest.mark.parametrize(
    "status,body",
    [
        (500, {"ok": False, "error_code": 500, "description": "Internal Server Error"}),
        (502, {"ok": False, "error_code": 400, "description": "Bad Request"}),
        (400, {}),
        (400, {"ok": False, "description": "missing error code"}),
        (400, {"ok": 0, "error_code": 400, "description": "bad bool"}),
        (400, {"ok": False, "error_code": "400", "description": "bad integer"}),
        (400, {"ok": False, "error_code": True, "description": "bad integer"}),
        (400, {"ok": False, "error_code": 401, "description": "mismatch"}),
        (400, {"ok": False, "error_code": 400, "description": ["invalid"]}),
        (400, {"ok": False, "error_code": 400, "description": "bad", "result": {}}),
        (400, {"ok": False, "error_code": 400, "description": "bad", "parameters": []}),
        (408, {"ok": False, "error_code": 408, "description": "Request Timeout"}),
        (409, {"ok": False, "error_code": 409, "description": "unknown send code"}),
        (302, {"ok": True, "result": {"message_id": 7}}),
        (201, {"ok": True, "result": {"message_id": 7}}),
        (200, {"ok": True}),
        (200, {"ok": True, "result": None}),
        (200, {"ok": True, "result": {}}),
        (200, {"ok": 1, "result": {"message_id": 7}}),
        (200, {"ok": True, "result": {"message_id": True}}),
        (200, {"ok": True, "result": {"message_id": "7"}}),
        (200, {"ok": True, "result": {"message_id": {"id": 7}}}),
        (200, {"ok": True, "result": {"message_id": -1}}),
        (200, {"ok": True, "result": {"message_id": 7}, "error_code": 400}),
    ],
)
def test_incomplete_or_contradictory_receipts_never_prove_effect_or_refusal(status, body):
    receipt = classify_send_response(ProviderResponse(status, body))
    assert receipt.outcome == "unknown"
    assert receipt.provider_message_id is None
    assert receipt.error_code is None


@pytest.mark.parametrize("code", [400, 401, 403, 404, 429])
def test_complete_explicit_refusal_is_distinct_from_retry_authorization(code):
    receipt = classify_send_response(
        ProviderResponse(
            code,
            {
                "ok": False,
                "error_code": code,
                "description": "Bad Request: chat not found",
                "parameters": {"retry_after": 7},
            },
        )
    )
    assert receipt.outcome == "known_not_sent"
    assert receipt.error_code == code
    assert receipt.error == "Bad Request: chat not found"
    assert receipt.retry_after_seconds == 7


def test_scheduled_receipt_is_neither_non_delivery_nor_usable_acceptance():
    receipt = classify_send_response(
        ProviderResponse(
            200,
            {
                "ok": True,
                "result": {"message_id": 0},
            },
        )
    )
    assert receipt.outcome == "provider_queued"
    assert receipt.provider_message_id is None
    assert "scheduled" in receipt.error and "unusable" in receipt.error
    accepted = classify_send_response(
        ProviderResponse(
            200,
            {
                "ok": True,
                "result": {"message_id": 87},
            },
        )
    )
    assert (accepted.outcome, accepted.provider_message_id) == ("provider_accepted", "87")


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "status,raw",
    [
        (400, b"<html>proxy failure</html>"),
        (500, b"not JSON"),
        (400, b'{"ok":true,"ok":false,"error_code":400,"description":"bad"}'),
        (400, b'{"ok":false,"error_code":400,"description":"bad","extra":NaN}'),
        (200, b"[]"),
    ],
)
async def test_real_http_malformed_body_is_not_a_synthetic_refusal(status, raw):
    with server(lambda h, _: reply(h, status, raw)) as (origin, calls):
        transport = local_transport(origin)
        with pytest.raises(TelegramTransportError):
            await transport.send_message(recipient="42", text="hello")
        await transport.close()
        assert len(calls) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("code", [301, 302, 303, 307, 308])
async def test_real_http_redirect_never_forwards_token_or_message(code):
    with server(lambda h, _: reply(h, body={"ok": True, "result": {"message_id": 1}})) as (
        target,
        target_calls,
    ):
        with server(lambda h, _: reply(h, code, {}, [("Location", target + "/stolen")])) as (
            origin,
            calls,
        ):
            transport = local_transport(origin)
            result = await transport.send_message(recipient="42", text="private fixture")
            assert classify_send_response(result).outcome == "unknown"
            await transport.close()
            assert len(calls) == 1
            assert target_calls == []


@pytest.mark.asyncio
async def test_actual_connection_loss_does_not_expose_url_or_allow_refusal():
    def drop(handler, _):
        handler.connection.shutdown(socket.SHUT_RDWR)
        handler.connection.close()

    with server(drop) as (origin, calls):
        transport = local_transport(origin)
        with pytest.raises(TelegramTransportError) as caught:
            await transport.send_message(recipient="42", text="hello")
        rendered = "".join(traceback.format_exception(caught.value))
        assert "fixture-secret" not in rendered
        assert "127.0.0.1" not in str(caught.value)
        await transport.close()
        assert len(calls) == 1


@pytest.mark.asyncio
async def test_repeated_cancel_and_close_join_actual_entered_http_work():
    entered, release, finished = threading.Event(), threading.Event(), threading.Event()

    def gated(handler, _):
        entered.set()
        assert release.wait(5)
        reply(handler, body={"ok": True, "result": {"message_id": 9}})

    with server(gated) as (origin, calls):
        transport = local_transport(origin)
        original = transport._request_sync

        def tracked(*args):
            try:
                return original(*args)
            finally:
                finished.set()

        transport._request_sync = tracked
        sender = asyncio.create_task(transport.send_message(recipient="42", text="hello"))
        assert await asyncio.to_thread(entered.wait, 2)
        sender.cancel("first")
        await asyncio.sleep(0)
        sender.cancel("second")
        closer = asyncio.create_task(transport.close())
        await asyncio.sleep(0)
        closer.cancel("close cancelled")
        await asyncio.sleep(0)
        closer.cancel("close cancelled twice")
        await asyncio.sleep(0)
        assert not sender.done() and not closer.done() and not finished.is_set()
        with pytest.raises(TelegramTransportError, match="closed"):
            await transport.send_message(recipient="42", text="do not start")
        release.set()
        with pytest.raises(asyncio.CancelledError, match="first"):
            await sender
        with pytest.raises(asyncio.CancelledError, match="close cancelled"):
            await closer
        assert finished.is_set()
        assert not transport._operations
        assert len(calls) == 1
        await transport.close()


@pytest.mark.asyncio
async def test_cancel_before_entry_and_pre_cancelled_caller_do_not_start_a_request():
    with server(lambda h, _: reply(h, body={"ok": True, "result": {"message_id": 9}})) as (
        origin,
        calls,
    ):
        transport = local_transport(origin)
        sender = asyncio.create_task(transport.send_message(recipient="42", text="hello"))
        sender.cancel()
        with pytest.raises(asyncio.CancelledError):
            await sender

        async def already_cancelled():
            asyncio.current_task().cancel()
            await transport.send_message(recipient="42", text="hello")

        with pytest.raises(asyncio.CancelledError):
            await asyncio.create_task(already_cancelled())
        await transport.close()
        assert calls == []


@pytest.mark.asyncio
async def test_actual_body_limits_and_separate_token_clients(monkeypatch):
    def respond(handler, raw):
        if handler.path.endswith("sendMessage"):
            token = handler.path.split("/")[1]
            reply(handler, body={"ok": True, "result": {"message_id": 1 if "one" in token else 2}})
        else:
            reply(handler, body=b"abcde")

    with server(respond) as (origin, calls):
        one, two = local_transport(origin, "1:one"), local_transport(origin, "2:two")
        a, b = await asyncio.gather(
            one.send_message(recipient="1", text="first"),
            two.send_message(recipient="2", text="second"),
        )
        assert classify_send_response(a).provider_message_id == "1"
        assert classify_send_response(b).provider_message_id == "2"
        for path, raw, _ in calls:
            expected = (
                {"chat_id": "1", "text": "first"}
                if "one" in path
                else {
                    "chat_id": "2",
                    "text": "second",
                }
            )
            assert json.loads(raw) == expected
        with pytest.raises(TelegramMediaTooLarge):
            await one.download_file("photos/file.jpg", 4)
        monkeypatch.setattr(telegram, "_JSON_RESPONSE_LIMIT", 8)
        with pytest.raises(TelegramMediaTooLarge):
            await one.send_message(recipient="1", text="first")
        await one.close()
        await two.close()


@pytest.mark.asyncio
async def test_token_redaction_and_no_ambient_proxy(monkeypatch):
    with server(
        lambda h, _: reply(
            h,
            400,
            {
                "ok": False,
                "error_code": 400,
                "description": "bad 123:fixture-secret",
            },
        )
    ) as (origin, calls):
        monkeypatch.setenv("http_proxy", "http://127.0.0.1:1")
        monkeypatch.setenv("HTTP_PROXY", "http://127.0.0.1:1")
        monkeypatch.setenv("no_proxy", "")
        monkeypatch.setenv("NO_PROXY", "")
        transport = local_transport(origin)
        response = await transport.send_message(recipient="42", text="hello")
        assert classify_send_response(response).error == "bad [redacted]"
        assert "fixture-secret" not in repr(response)
        assert len(calls) == 1
        await transport.close()


@pytest.mark.asyncio
async def test_url_bearing_exception_is_redacted_in_traceback():
    transport = TelegramBotApiTransport("123:fixture-secret")

    def fail(_):
        raise URLError(transport._api_base)

    transport._open = fail
    with pytest.raises(TelegramTransportError) as caught:
        await transport.send_message(recipient="42", text="hello")
    assert "fixture-secret" not in "".join(traceback.format_exception(caught.value))
    await transport.close()


@pytest.mark.asyncio
async def test_malformed_http_error_response_is_closed():
    from urllib.error import HTTPError

    with server(lambda h, _: reply(h, 400, b"not JSON")) as (origin, _):
        transport = local_transport(origin)
        opened = []
        original = transport._open

        def capture(request):
            try:
                return original(request)
            except HTTPError as error:
                opened.append(error)
                raise

        transport._open = capture
        with pytest.raises(TelegramTransportError):
            await transport.send_message(recipient="42", text="hello")
        assert len(opened) == 1 and opened[0].closed
        await transport.close()


@pytest.mark.asyncio
async def test_media_redirect_is_blocked_and_does_not_fetch_target():
    with server(lambda h, _: reply(h, body=b"private data")) as (target, target_calls):
        with server(lambda h, _: reply(h, 302, {}, [("Location", target + "/stolen")])) as (
            origin,
            _,
        ):
            transport = local_transport(origin)
            with pytest.raises(TelegramTransportError, match="download failed"):
                await transport.download_file("photos/file.jpg", 64)
            assert target_calls == []
            await transport.close()


@pytest.mark.asyncio
async def test_late_thread_failure_preserves_repeated_caller_cancellation():
    entered, release, finished = threading.Event(), threading.Event(), threading.Event()
    transport = TelegramBotApiTransport("123:fixture-secret")

    def fail_after_entry(*_):
        entered.set()
        assert release.wait(5)
        finished.set()
        raise TelegramTransportError("late failure")

    transport._request_sync = fail_after_entry
    sender = asyncio.create_task(transport.send_message(recipient="42", text="hello"))
    assert await asyncio.to_thread(entered.wait, 2)
    sender.cancel("original stop")
    await asyncio.sleep(0)
    sender.cancel("later stop")
    await asyncio.sleep(0)
    assert not sender.done()
    release.set()
    with pytest.raises(asyncio.CancelledError, match="original stop"):
        await sender
    assert finished.is_set() and not transport._operations
    await transport.close()


def test_photo_header_injection_refused_before_transport_entry():
    with pytest.raises(ValueError, match="MIME"):
        TelegramBotApiTransport._multipart_photo(
            recipient="42",
            data=b"fixture",
            mime_type="image/png\r\nInjected: value",
            caption=None,
        )


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "raw",
    [
        b'{"ok":true,"result":{"message_id":87,"extra":1e999}}',
        b'{"ok":true,"result":[{"update_id":1,"extra":-1e999}]}',
    ],
)
async def test_non_finite_exponent_is_rejected_on_actual_send_and_updates(raw):
    with server(lambda h, _: reply(h, 200, raw)) as (origin, calls):
        transport = local_transport(origin)
        with pytest.raises(TelegramTransportError, match="invalid JSON"):
            if raw.startswith(b'{"ok":true,"result":['):
                await transport.get_updates(None)
            else:
                await transport.send_message(recipient="42", text="hello")
        assert len(calls) == 1
        await transport.close()


@pytest.mark.asyncio
@pytest.mark.parametrize('topic', [None, 1, 2147483647])
async def test_explicit_topic_json_and_multipart_on_actual_loopback(topic):
    with server(lambda h, _: reply(h, body={'ok': True, 'result': {'message_id': 9}})) as (origin, calls):
        transport = local_transport(origin)
        await transport.send_message(recipient='-99', text='hello', message_thread_id=topic)
        await transport.send_photo(
            recipient='-99', data=b'\x00\xffphoto', mime_type='image/png',
            caption='caption', message_thread_id=topic)
        await transport.close()
    expected = {'chat_id': '-99', 'text': 'hello'}
    if topic is not None:
        expected['message_thread_id'] = topic
    assert json.loads(calls[0][1]) == expected
    content_type = calls[1][2]['Content-Type']
    multipart = BytesParser(policy=default).parsebytes(
        b'Content-Type: ' + content_type.encode() + b'\r\nMIME-Version: 1.0\r\n\r\n' + calls[1][1])
    fields = {part.get_param('name', header='content-disposition'): part.get_payload(decode=True)
              for part in multipart.iter_parts()}
    expected_fields = {'chat_id': b'-99', 'caption': b'caption', 'photo': b'\x00\xffphoto'}
    if topic is not None:
        expected_fields['message_thread_id'] = str(topic).encode()
    assert fields == expected_fields


@pytest.mark.asyncio
async def test_invalid_topic_never_enters_transport_worker(monkeypatch):
    transport = TelegramBotApiTransport('123:fixture-secret')
    entries = []

    async def forbidden(*args):
        entries.append(args)
        raise AssertionError('invalid topic entered provider work')

    monkeypatch.setattr(transport, '_run_thread', forbidden)
    for invalid in [True, False, 0, -1, 2147483648, '12', 12.0, {}, []]:
        with pytest.raises(ValueError, match='message_thread_id'):
            await transport.send_message(recipient='-99', text='hello', message_thread_id=invalid)
        with pytest.raises(ValueError, match='message_thread_id'):
            await transport.send_photo(recipient='-99', data=b'photo', mime_type='image/png',
                                       caption=None, message_thread_id=invalid)
    assert entries == []
    await transport.close()


def bot_result(**changes):
    return {"id": 123, "is_bot": True, "first_name": "Example", "username": "example_bot",
            "can_join_groups": True, "can_read_all_group_messages": False, **changes}


def webhook_result(**changes):
    return {"url": "https://other.invalid/private-path?secret=webhook-private",
            "has_custom_certificate": False, "pending_update_count": 4,
            "allowed_updates": ["message"], **changes}


@pytest.mark.asyncio
async def test_setup_inspection_observes_account_and_webhook_without_mutation_or_secret_output():
    def respond(handler, _):
        result = bot_result() if handler.path.endswith("getMe") else webhook_result(
            last_error_message="webhook-private at https://other.invalid/private-path")
        reply(handler, body={"ok": True, "result": result})

    with server(respond) as (origin, calls):
        transport = local_transport(origin)
        identity = await transport.get_me()
        webhook = await transport.get_webhook_info()
        assert identity == TelegramBotIdentity("123", "example_bot", True, False)
        assert webhook == TelegramWebhookInfo(True, False, 4, ("message",), has_last_error=True)
        rendered = repr(identity) + repr(webhook) + json.dumps(asdict(webhook))
        assert all(secret not in rendered for secret in (
            "fixture-secret", "webhook-private", "other.invalid", "private-path"))
        assert [(path.rsplit("/", 1)[-1], json.loads(raw)) for path, raw, _ in calls] == [
            ("getMe", {}), ("getWebhookInfo", {})]
        assert not hasattr(webhook, "ready") and not hasattr(webhook, "polling_available")
        await transport.close()


@pytest.mark.asyncio
async def test_setup_inspection_preserves_absent_vs_explicit_provider_fields():
    responses = [
        {"id": 2**52 - 1, "is_bot": True, "first_name": "Example"},
        {"url": "", "has_custom_certificate": False, "pending_update_count": 0},
        webhook_result(url="", allowed_updates=[]),
    ]
    with server(lambda h, _: reply(h, body={"ok": True, "result": responses.pop(0)})) as (origin, calls):
        transport = local_transport(origin)
        assert await transport.get_me() == TelegramBotIdentity(str(2**52 - 1), None, None, None)
        assert await transport.get_webhook_info() == TelegramWebhookInfo(False, False, 0, None)
        assert await transport.get_webhook_info() == TelegramWebhookInfo(False, False, 4, ())
        await transport.close()
        assert len(calls) == 3


@pytest.mark.asyncio
async def test_bot_inspection_rejects_malformed_or_nonbot_identity_without_private_diagnostics():
    invalid = [None, [], {}, bot_result(id=True), bot_result(id="123"), bot_result(id=0),
               bot_result(id=2**52), bot_result(is_bot=False), bot_result(is_bot=1),
               bot_result(first_name=""), bot_result(username=None), bot_result(username="bad/name"),
               bot_result(can_join_groups=1), bot_result(can_read_all_group_messages=None)]
    responses = iter(invalid)
    with server(lambda h, _: reply(h, body={"ok": True, "result": next(responses)})) as (origin, calls):
        transport = local_transport(origin)
        for _ in invalid:
            with pytest.raises(TelegramTransportError, match="getMe returned an invalid result") as caught:
                await transport.get_me()
            assert "fixture-secret" not in "".join(traceback.format_exception(caught.value))
        await transport.close()
        assert len(calls) == len(invalid)
        assert all(path.endswith("/getMe") for path, _, _ in calls)


@pytest.mark.asyncio
async def test_webhook_inspection_rejects_malformed_facts_without_returning_callback_material():
    invalid = [None, [], {}, webhook_result(url=None), webhook_result(has_custom_certificate=1),
               webhook_result(pending_update_count=True), webhook_result(pending_update_count=-1),
               webhook_result(pending_update_count=2**31), webhook_result(allowed_updates=None),
               webhook_result(allowed_updates="message"), webhook_result(allowed_updates=[True]),
               webhook_result(allowed_updates=["secret=webhook-private"]),
               webhook_result(allowed_updates=["message"] * 129)]
    responses = iter(invalid)
    with server(lambda h, _: reply(h, body={"ok": True, "result": next(responses)})) as (origin, calls):
        transport = local_transport(origin)
        for _ in invalid:
            with pytest.raises(TelegramTransportError, match="getWebhookInfo returned an invalid result") as caught:
                await transport.get_webhook_info()
            rendered = "".join(traceback.format_exception(caught.value))
            assert "webhook-private" not in rendered and "fixture-secret" not in rendered
        await transport.close()
        assert len(calls) == len(invalid)
        assert all(path.endswith("/getWebhookInfo") for path, _, _ in calls)


@pytest.mark.asyncio
@pytest.mark.parametrize("method", ["get_me", "get_webhook_info"])
async def test_setup_inspection_refusals_are_redacted_and_do_not_trigger_other_operations(method):
    responses = iter([
        (401, {"ok": False, "error_code": 401, "description": "token 123:fixture-secret"}),
        (200, {"ok": True, "error_code": 401, "result": bot_result()}),
        (500, {"ok": True, "result": webhook_result()}),
    ])
    def respond(handler, _):
        status, body = next(responses)
        reply(handler, status, body)
    with server(respond) as (origin, calls):
        transport = local_transport(origin)
        for _ in range(3):
            with pytest.raises(TelegramTransportError) as caught:
                await getattr(transport, method)()
            assert "fixture-secret" not in "".join(traceback.format_exception(caught.value))
        await transport.close()
        assert len(calls) == 3
        assert len({path for path, _, _ in calls}) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("method", ["get_me", "get_webhook_info"])
async def test_setup_inspection_cancellation_joins_entered_request_and_close_refuses_new_reads(method):
    entered, release = threading.Event(), threading.Event()
    def gated(handler, _):
        entered.set()
        assert release.wait(5)
        reply(handler, body={"ok": True, "result": bot_result() if method == "get_me" else webhook_result()})
    with server(gated) as (origin, calls):
        transport = local_transport(origin)
        operation = asyncio.create_task(getattr(transport, method)())
        try:
            assert await asyncio.to_thread(entered.wait, 2)
            operation.cancel("first")
            await asyncio.sleep(0)
            operation.cancel("second")
            closer = asyncio.create_task(transport.close())
            await asyncio.sleep(0)
            assert not operation.done() and not closer.done()
            with pytest.raises(TelegramTransportError, match="closed"):
                await getattr(transport, method)()
        finally:
            release.set()
        with pytest.raises(asyncio.CancelledError, match="first"):
            await operation
        await closer
        assert not transport._operations and len(calls) == 1


@pytest.mark.asyncio
async def test_setup_inspection_keeps_accounts_separate_and_observes_each_read_again():
    reads = {"one": 0, "two": 0}
    def respond(handler, _):
        account = "one" if "1:one" in handler.path else "two"
        reads[account] += 1
        reply(handler, body={"ok": True, "result": bot_result(
            id=1 if account == "one" else 2, username=account + "_bot",
            can_join_groups=reads[account] > 1)})
    with server(respond) as (origin, calls):
        one, two = local_transport(origin, "1:one"), local_transport(origin, "2:two")
        first, other = await asyncio.gather(one.get_me(), two.get_me())
        assert (first.bot_id, other.bot_id) == ("1", "2")
        assert first.username == "one_bot" and other.username == "two_bot"
        assert first.can_join_groups is False and other.can_join_groups is False
        assert (await one.get_me()).can_join_groups is True
        await one.close()
        with pytest.raises(TelegramTransportError, match="closed"):
            await one.get_me()
        assert (await two.get_me()).can_join_groups is True
        await two.close()
        assert reads == {"one": 2, "two": 2} and len(calls) == 4


@pytest.mark.asyncio
async def test_webhook_comparison_is_exact_local_and_rechecked_without_returning_urls():
    expected = "https://Home.invalid:443/callback%2Fprivate?secret=expected-private"
    observations = [
        (expected, None, True, None),
        ("", expected, False, False),
        (expected, expected, True, True),
        (expected.lower(), expected, True, False),
        (expected.replace(":443", ""), expected, True, False),
        (expected.replace("%2F", "/"), expected, True, False),
        (expected + "/", expected, True, False),
        (expected.replace("expected-private", "other-private"), expected, True, False),
        ("https://other.invalid/callback", expected, True, False),
    ]
    responses = iter(observations)
    with server(lambda h, _: reply(h, body={"ok": True, "result": webhook_result(
            url=next(responses)[0])})) as (origin, calls):
        transport = local_transport(origin)
        for _, compare, configured, matched in observations:
            result = await transport.get_webhook_info(expected_url=compare)
            assert result.configured is configured
            assert result.matches_expected_url is matched
            rendered = repr(result) + json.dumps(asdict(result))
            assert all(secret not in rendered for secret in (
                "fixture-secret", "Home.invalid", "home.invalid", "other.invalid",
                "expected-private", "other-private", "callback"))
        await transport.close()
        assert [(path.rsplit("/", 1)[-1], json.loads(raw)) for path, raw, _ in calls] == [
            ("getWebhookInfo", {})] * len(observations)


@pytest.mark.asyncio
async def test_webhook_comparison_rejects_invalid_expected_value_before_transport_entry():
    with server(lambda h, _: reply(h, body={"ok": True, "result": webhook_result()})) as (origin, calls):
        transport = local_transport(origin)
        for invalid in ["", False, 12, [], {"url": "expected-private"}]:
            with pytest.raises(ValueError, match="expected_url must be a nonempty string or None") as caught:
                await transport.get_webhook_info(expected_url=invalid)
            assert "expected-private" not in "".join(traceback.format_exception(caught.value))
        await transport.close()
        assert calls == []


@pytest.mark.asyncio
async def test_webhook_last_error_presence_is_not_current_health_or_raw_diagnostic_text():
    observations = [
        ({}, False, None),
        ({"last_error_message": ""}, True, None),
        ({"last_error_message": "https://private.invalid?secret=diagnostic-private"}, True, None),
        ({"last_error_date": 0}, True, 0),
        ({"last_error_date": 2147483647}, True, 2147483647),
        ({"last_error_date": 1, "last_error_message": "historical-private"}, True, 1),
        ({"last_synchronization_error_date": 2, "future_provider_field": "unknown-private"}, False, None),
    ]
    responses = iter(observations)
    with server(lambda h, _: reply(h, body={"ok": True, "result": webhook_result(
            **next(responses)[0])})) as (origin, calls):
        transport = local_transport(origin)
        for _, present, timestamp in observations:
            result = await transport.get_webhook_info()
            assert result.has_last_error is present and result.last_error_date == timestamp
            assert result.matches_expected_url is None
            rendered = repr(result) + json.dumps(asdict(result))
            assert all(secret not in rendered for secret in (
                "diagnostic-private", "historical-private", "unknown-private", "private.invalid",
                "webhook-private", "fixture-secret"))
            assert not hasattr(result, "healthy") and not hasattr(result, "last_error_message")
        await transport.close()
        assert len(calls) == len(observations)
        assert all(path.endswith("/getWebhookInfo") and json.loads(raw) == {} for path, raw, _ in calls)


@pytest.mark.asyncio
async def test_webhook_comparison_refuses_malformed_errors_and_unauthenticated_matching_urls():
    expected = "https://private.invalid?secret=expected-private"
    invalid = [
        {"last_error_date": None}, {"last_error_date": True}, {"last_error_date": -1},
        {"last_error_date": 2147483648}, {"last_error_date": "1"}, {"last_error_date": 1.0},
        {"last_error_message": None}, {"last_error_message": False},
        {"last_error_message": {"diagnostic": "diagnostic-private"}},
    ]
    responses = iter([
        *((200, {"ok": True, "result": webhook_result(url=expected, **fields)}) for fields in invalid),
        (401, {"ok": True, "result": webhook_result(url=expected)}),
        (403, {"ok": False, "error_code": 403, "description": "diagnostic-private " + expected}),
        (500, {"ok": True, "result": webhook_result(url=expected)}),
        (200, {"ok": True, "error_code": 401, "result": webhook_result(url=expected)}),
    ])
    def respond(handler, _):
        status, body = next(responses)
        reply(handler, status, body)
    with server(respond) as (origin, calls):
        transport = local_transport(origin)
        for _ in range(len(invalid) + 4):
            with pytest.raises(TelegramTransportError) as caught:
                await transport.get_webhook_info(expected_url=expected)
            rendered = "".join(traceback.format_exception(caught.value))
            assert all(secret not in rendered for secret in (
                "fixture-secret", "expected-private", "diagnostic-private", "private.invalid"))
        await transport.close()
        assert [(path.rsplit("/", 1)[-1], json.loads(raw)) for path, raw, _ in calls] == [
            ("getWebhookInfo", {})] * (len(invalid) + 4)
