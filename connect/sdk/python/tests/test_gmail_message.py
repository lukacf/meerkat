"""Public pure-helper contract, frozen before implementation."""
import base64
from copy import deepcopy
from dataclasses import FrozenInstanceError
from email import policy
from email.parser import BytesParser
import importlib
import traceback

import pytest


def api():
    return importlib.import_module("meerkat_toolkit.connectors.gmail_message")


def encoded(value):
    return base64.urlsafe_b64encode(value).decode("ascii").rstrip("=")


def part(value=b"hello", *, mime="text/plain", headers=None, **changes):
    return {"partId": "p0", "mimeType": mime, "filename": "",
            "headers": headers or [], "body": {"size": len(value), "data": encoded(value)},
            **changes}


def message(payload=None):
    return {"id": "m1", "threadId": "t1", "labelIds": ["INBOX", "UNREAD"],
            "snippet": "short", "internalDate": "123", "payload": payload or part()}


def parent():
    return {"id": "parent_1", "threadId": "thread_1", "payload": {"headers": [
        {"name": "Subject", "value": "Topic"},
        {"name": "Message-ID", "value": "<parent@example.test>"},
        {"name": "References", "value": "<old@example.test>"}]}}


def test_public_format_helpers_exist():
    module = api()
    for name in ("decode_message", "validate_reply_selection", "reply_linkage",
                 "compose_message", "GmailDecodeLimits", "GmailMessageFormatError"):
        assert callable(getattr(module, name))


def test_immutable_outer_headers_parts_and_caller_input():
    raw = message(part(headers=[{"name": "From", "value": "first@example.test"},
                                {"name": "from", "value": "second@example.test"}]))
    saved = deepcopy(raw)
    view = api().decode_message(raw)
    assert [(h.name, h.value) for h in view.headers] == [
        ("From", "first@example.test"), ("from", "second@example.test")]
    assert view.plain_text == ("hello",) and view.complete
    assert view.parts[0].path == () and view.parts[0].part_id == "p0"
    with pytest.raises(FrozenInstanceError):
        view.parts[0].text = "changed"
    raw["payload"]["headers"][0]["value"] = "mutated"
    assert view.headers[0].value == saved["payload"]["headers"][0]["value"]


def test_forwarded_headers_and_body_stay_separate():
    nested = part(b"not the outer body", headers=[{"name": "From", "value": "nested"}])
    forwarded = {"partId": "forwarded", "mimeType": "message/rfc822",
                 "filename": "forward.eml", "headers": [{"name": "Subject", "value": "inside"}],
                 "body": {}, "parts": [nested]}
    payload = {"partId": "root", "mimeType": "multipart/mixed",
               "headers": [{"name": "Subject", "value": "outside"}], "body": {},
               "parts": [part(b"outer"), forwarded]}
    view = api().decode_message(message(payload))
    assert [(h.name, h.value) for h in view.headers] == [("Subject", "outside")]
    assert view.plain_text == ("outer",)
    assert view.attachments[0].part_id == "forwarded"
    assert view.parts[-1].headers[0].value == "nested"
    assert not view.parts[-1].inline_body
    assert view.complete


@pytest.mark.parametrize("encoding,charset", [
    ("utf-8", "UTF-8"), ("ascii", "us-ascii"),
    ("iso-8859-1", "latin1"), ("cp1252", "windows-1252")])
def test_explicit_common_charset(encoding, charset):
    text = "plain" if encoding == "ascii" else "café"
    raw = message(part(text.encode(encoding), headers=[
        {"name": "Content-Type", "value": "text/plain; charset=" + charset}]))
    assert api().decode_message(raw).plain_text == (text,)


def test_explicit_default_charset_is_not_guessed():
    raw = message(part("räksmörgås".encode("utf-8")))
    with pytest.raises(api().GmailMessageFormatError):
        api().decode_message(raw)
    assert api().decode_message(raw, default_charset="utf-8").plain_text == ("räksmörgås",)


@pytest.mark.parametrize("data", ["!!!", "a", "a===", "YQ==\n", "YR", "a+b/"])
def test_malformed_base64_is_not_empty_success(data):
    raw = message(part(body={"data": data}))
    with pytest.raises(api().GmailMessageFormatError) as caught:
        api().decode_message(raw)
    assert caught.value.code == "invalid_message"


def test_size_mismatch_is_not_partial_success():
    raw = message(part(body={"size": 8, "data": encoded(b"short")}))
    with pytest.raises(api().GmailMessageFormatError):
        api().decode_message(raw)


@pytest.mark.parametrize("body,complete,reason", [
    ({"size": 0}, True, None), ({"data": "", "size": 0}, True, None),
    ({"attachmentId": "text-ref", "size": 100}, False, "external_body"),
    ({}, False, "missing_body_data")])
def test_empty_and_unavailable_body_are_distinct(body, complete, reason):
    view = api().decode_message(message(part(body=body)))
    assert view.complete is complete
    assert view.parts[0].unavailable_reason == reason
    assert view.parts[0].attachment_id == body.get("attachmentId")
    assert view.plain_text == (("",) if complete else ())


@pytest.mark.parametrize("headers", [
    [{"name": "Content-Type", "value": "text/plain"},
     {"name": "content-type", "value": "text/plain"}],
    [{"name": "Content-Type", "value": "text/plain; charset=utf-8; charset=ascii"}],
    [{"name": "Content-Type", "value": "text/html; charset=utf-8"}],
    [{"name": "Content-Type", "value": "text/plain; charset=x-private"}],
])
def test_ambiguous_or_unsupported_content_type_refuses(headers):
    with pytest.raises(api().GmailMessageFormatError):
        api().decode_message(message(part(headers=headers)))


@pytest.mark.parametrize("limit,value", [
    ("max_parts", 1), ("max_depth", 1), ("max_headers", 1),
    ("max_header_bytes", 4), ("max_decoded_bytes", 2)])
def test_resource_limits_refuse_without_clipping(limit, value):
    inner = part(b"abcdef", headers=[{"name": "X-One", "value": "alpha"},
                                    {"name": "X-Two", "value": "beta"}])
    raw = message({"mimeType": "multipart/mixed", "body": {}, "parts": [
        {"mimeType": "multipart/mixed", "body": {}, "parts": [inner]}]})
    with pytest.raises(api().GmailMessageFormatError) as caught:
        api().decode_message(raw, limits=api().GmailDecodeLimits(**{limit: value}))
    assert caught.value.code == "limit_exceeded"


def test_python_cycle_refuses_without_mutation():
    payload = {"mimeType": "multipart/mixed", "body": {}, "parts": []}
    payload["parts"].append(payload)
    with pytest.raises(api().GmailMessageFormatError) as caught:
        api().decode_message(message(payload))
    assert caught.value.code == "invalid_message"
    assert payload["parts"][0] is payload


def test_reply_linkage_and_wire_preserve_explicit_choices():
    module = api()
    reply = module.reply_linkage(parent(), parent_id="parent_1",
                                 thread_id="thread_1", subject="Topic")
    assert reply.parent_message_id == "<parent@example.test>"
    assert reply.references == ("<old@example.test>", "<parent@example.test>")
    wire = module.compose_message(to="Chosen <to@example.test>", cc="cc@example.test",
                                  bcc="bcc@example.test", subject="Topic",
                                  body_text="Räksmörgås\nsecond", reply=reply)
    parsed = BytesParser(policy=policy.default).parsebytes(wire)
    assert str(parsed["To"]) == "Chosen <to@example.test>"
    assert str(parsed["Cc"]) == "cc@example.test"
    assert str(parsed["Bcc"]) == "bcc@example.test"
    assert str(parsed["In-Reply-To"]) == "<parent@example.test>"
    assert str(parsed["References"]).split() == list(reply.references)
    assert parsed["Message-ID"] is None
    assert parsed.get_content().replace("\r\n", "\n") == "Räksmörgås\nsecond\n"


@pytest.mark.parametrize("field", ["to", "cc", "bcc", "subject"])
def test_all_header_injection_boundaries_refuse(field):
    args = {"to": "to@example.test", "subject": "Subject", "body_text": "body"}
    args[field] = "synthetic-private-canary\r\nBcc: hidden@example.test"
    with pytest.raises(api().GmailMessageFormatError) as caught:
        api().compose_message(**args)
    assert caught.value.code == "invalid_composition"
    assert "synthetic-private-canary" not in str(caught.value)
    assert caught.value.__cause__ is None and caught.value.__context__ is None
    assert "synthetic-private-canary" not in "".join(traceback.format_exception_only(caught.value))


@pytest.mark.parametrize("field,value", [("id", "other"), ("threadId", "other")])
def test_reply_cannot_rebind_parent(field, value):
    raw = parent()
    raw[field] = value
    with pytest.raises(api().GmailMessageFormatError):
        api().reply_linkage(raw, parent_id="parent_1", thread_id="thread_1", subject="Topic")



def test_empty_external_data_retains_unavailable_size():
    view = api().decode_message(message(part(body={
        "data": "", "size": 100, "attachmentId": "body-ref"})))
    assert view.complete is False
    assert view.parts[0].size == 100 and view.parts[0].text is None
    assert view.parts[0].unavailable_reason == "external_body"


def test_container_cannot_silently_discard_inline_bytes():
    with pytest.raises(api().GmailMessageFormatError):
        api().decode_message(message(part(mime="multipart/mixed")))
