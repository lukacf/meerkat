"""Independent real-MCP checks for structured drafts without provider sockets."""

from __future__ import annotations

import asyncio
import base64
from email import policy
from email.parser import Parser
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import httpx
from mcp import Client

from test_gmail_recipient_scope import OMITTED, borrowed, scoped
from test_services_mcp import ConfigurationError, Fixture, config_module, configuration, eventually
from test_structured_send import PRIVATE, Provider, connected, message


DRAFT = "gmail_create_draft_message"
RECEIPT = {"id": "draft-1", "message": {"id": "message-1", "threadId": "thread-1",
                                       "labelIds": ["DRAFT"]}}


class DraftProvider(Provider):
    """Model draft acceptance independently from whether its receipt arrives."""

    def __init__(self, mode="success"):
        super().__init__(mode)
        self.accepted_drafts = []

    async def handle(self, request):
        self.requests.append({"method": request.method, "url": str(request.url),
                              "authorization": request.headers.get("authorization"),
                              "content": bytes(request.content)})
        self.events.append("request")
        if self.mode == "accepted_disconnect":
            self.accepted_drafts.append(json.loads(request.content))
            raise httpx.ReadError(PRIVATE, request=request)
        if self.mode == "timeout":
            raise httpx.ReadTimeout(PRIVATE, request=request)
        status = {"rate_limit": 429, "redirect": 302, "authentication": 401,
                  "permission": 403, "server": 503}.get(self.mode, 200)
        payload = RECEIPT
        if self.mode == "message_receipt":
            payload = {"id": "message-1", "threadId": "thread-1"}
        elif self.mode == "missing_message":
            payload = {"id": "draft-1"}
        elif self.mode == "missing_draft_id":
            payload = {"message": {"id": "message-1"}}
        elif status != 200:
            payload = {"private": PRIVATE}
        provider = self

        class Stream(httpx.AsyncByteStream):
            async def __aiter__(self):
                yield json.dumps(payload).encode()

            async def aclose(self):
                provider.events.append("response_closing")
                provider.events.append("response_closed")

        return httpx.Response(status, request=request, stream=Stream(), headers={
            "content-type": "application/json", "retry-after": "3",
            "location": "https://unselected.example.invalid/"})


def decode_draft(request):
    payload = json.loads(request["content"])
    if set(payload) != {"message"} or set(payload["message"]) != {"raw"}:
        raise AssertionError("Structured draft changed the explicit draft API envelope")
    mime = base64.urlsafe_b64decode(payload["message"]["raw"])
    return mime, Parser(policy=policy.SMTPUTF8).parsestr(mime.decode("utf-8"))


class StructuredDraftTests(unittest.IsolatedAsyncioTestCase):
    async def test_draft_only_selection_exposes_explicit_mime_fields(self):
        provider = DraftProvider()
        async with connected(provider, operations=("create_draft_message",)) as client:
            listing = await client.list_tools()
            self.assertEqual([tool.name for tool in listing.tools], ["gmail_create_draft_message"])
            tool = listing.tools[0]
            self.assertEqual(set(tool.input_schema["required"]),
                             {"from_address", "to", "subject", "body_text"})
            self.assertEqual(set(tool.input_schema["properties"]),
                             {"from_address", "to", "cc", "bcc", "subject", "body_text"})
            self.assertFalse(tool.input_schema["additionalProperties"])
            self.assertFalse(tool.annotations.read_only_hint)
            self.assertTrue(tool.annotations.destructive_hint)
            self.assertFalse(tool.annotations.idempotent_hint)
        self.assertEqual(provider.requests, [])

    def assert_refused(self, result, kind="invalid_arguments"):
        self.assertTrue(result.is_error)
        self.assertEqual(result.structured_content["error"], {
            "kind": kind, "effect": "write", "operation_invoked": False,
            "write_outcome": "not_attempted"})

    def assert_draft_posts(self, provider, count):
        self.assertEqual(len(provider.requests), count)
        self.assertTrue(all(row["method"] == "POST" and row["url"].endswith("/drafts")
                            for row in provider.requests))
        self.assertFalse(any("/messages/send" in row["url"] or "/drafts/send" in row["url"]
                             for row in provider.requests))

    async def test_unicode_mime_posts_exact_draft_envelope_and_preserves_receipt(self):
        provider = DraftProvider()
        value = message(from_address="\u00c5sa <sender@example.invalid>",
                        to=["Zo\u00eb <first@example.invalid>", "\u8bfb\u8005@\u4f8b\u5b50.\u6d4b\u8bd5"],
                        cc=["\u00c9quipe <copy@example.invalid>"], bcc=["blind@example.invalid"],
                        subject="Draft: caf\u00e9, \u6771\u4eac, \U0001f680",
                        body_text="Hej!\n\u6771\u4eac: ready \U0001f680\nBcc: body-only@example.invalid")
        async with connected(provider, operations=("create_draft_message",)) as client:
            result = await client.call_tool(DRAFT, value)
            self.assertFalse(result.is_error)
            self.assertEqual(result.structured_content, {"value": RECEIPT})
            self.assertEqual(json.loads(result.content[0].text), RECEIPT)
        self.assert_draft_posts(provider, 1)
        row = provider.requests[0]
        self.assertEqual(row["url"], "https://gmail.googleapis.com/gmail/v1/users/routed%40example.invalid/drafts")
        self.assertEqual(row["authorization"], "Bearer fixture-token-A")
        mime, parsed = decode_draft(row)
        self.assertEqual(parsed.defects, [])
        self.assertEqual(parsed["From"].addresses[0].display_name, "\u00c5sa")
        self.assertEqual(parsed["From"].addresses[0].addr_spec, "sender@example.invalid")
        self.assertEqual([a.addr_spec for a in parsed["To"].addresses],
                         ["first@example.invalid", "\u8bfb\u8005@\u4f8b\u5b50.\u6d4b\u8bd5"])
        self.assertEqual(parsed["Cc"].addresses[0].addr_spec, "copy@example.invalid")
        self.assertEqual(parsed["Bcc"].addresses[0].addr_spec, "blind@example.invalid")
        self.assertEqual(str(parsed["Subject"]), value["subject"])
        self.assertEqual(parsed.get_content().replace("\r\n", "\n"), value["body_text"] + "\n")
        self.assertEqual((parsed.get_content_type(), parsed.get_content_charset()), ("text/plain", "utf-8"))
        self.assertEqual(set(parsed.keys()), {"From", "To", "Cc", "Bcc", "Subject", "Content-Type",
                                             "Content-Transfer-Encoding", "MIME-Version"})
        self.assertEqual(list(parsed.iter_attachments()), [])
        self.assertNotIn(b"\n", mime.replace(b"\r\n", b""))
        self.assertFalse(provider.settings["trust_env"])
        self.assertFalse(provider.settings["follow_redirects"])
        self.assertEqual(provider.events.count("response_closed"), 1)
        self.assertEqual(provider.client_closed, 1)

    async def test_unselected_send_raw_draft_and_read_tools_never_enter_provider(self):
        for selected, refused in (
            ("create_draft_message", ("gmail_send_message", "gmail_send_raw", "gmail_create_draft_raw", "gmail_get_message")),
            ("send_message", (DRAFT,)), ("create_draft_raw", (DRAFT,)), ("get_message", (DRAFT,))):
            provider = DraftProvider()
            async with connected(provider, operations=(selected,)) as client:
                self.assertEqual([t.name for t in (await client.list_tools()).tools], ["gmail_" + selected])
                for name in refused:
                    result = await client.call_tool(name, message())
                    self.assertTrue(result.is_error)
                    self.assertEqual(result.structured_content["error"]["kind"], "unknown_function")
                    self.assertFalse(result.structured_content["error"]["operation_invoked"])
            self.assertEqual(provider.requests, [])

    async def test_missing_fields_extra_routing_and_header_injection_are_local_refusals(self):
        bad = []
        for field in ("from_address", "to", "subject", "body_text"):
            value = message(); del value[field]; bad.append(value)
        for field in ("from_address", "subject"):
            for value in ("x\r\nBcc: thief@example.invalid", "x\nInjected: yes", "x\x00y"):
                bad.append(message(**{field: value}))
        for field in ("to", "cc", "bcc"):
            for value in ("first@example.invalid\r\nBcc: thief@example.invalid", "first@example.invalid\x00",
                          "a@example.invalid,b@example.invalid", "Team: first@example.invalid;", "no-mailbox"):
                bad.append(message(**{field: [value]}))
            bad.append(message(**{field: "first@example.invalid"}))
        for clear in (b"Name\x00Injected", b"Name\r\nBcc: thief@example.invalid"):
            encoded = clear
            for _ in range(3):
                encoded = b"=?utf-8?b?" + base64.b64encode(encoded) + b"?="
                value = encoded.decode() + " <first@example.invalid>"
                bad.append(message(from_address=value))
                bad.extend(message(**{field: [value]}) for field in ("to", "cc", "bcc"))
        bad.extend([message(to=[]), message(to=[1]), message(from_address=None), message(subject=False),
                    message(body_text=3), message(body_text="\ud800")])
        for field in ("thread_id", "threadId", "in_reply_to", "headers", "mime_base64", "attachments",
                      "html", "user_id", "token", "allowed_recipient_addresses"):
            bad.append(message(**{field: "unselected"}))
        provider = DraftProvider()
        async with borrowed(provider, scoped(["first@example.invalid"], ("create_draft_message",))) as (client, auth, _):
            for args in bad:
                with self.subTest(args=repr(args)):
                    self.assert_refused(await client.call_tool(DRAFT, args))
            self.assertEqual(auth.entries, 0)
        self.assertEqual(provider.requests, [])

    async def test_each_recipient_header_is_checked_before_connector_auth_or_transport(self):
        provider = DraftProvider()
        async with borrowed(provider, scoped(["first@example.invalid"], ("create_draft_message",))) as (client, auth, profile):
            with patch.object(profile.operations, "create_draft_raw", side_effect=AssertionError("draft entered")) as draft:
                with patch.object(profile.operations, "send_raw", side_effect=AssertionError("send entered")) as send:
                    for field in ("to", "cc", "bcc"):
                        for value in ("outside@example.invalid", '"first@example.invalid" <outside@example.invalid>',
                                      "=?utf-8?b?Rmlyc3Q=?= <outside@example.invalid>"):
                            with self.subTest(field=field, value=value):
                                result = await client.call_tool(DRAFT, message(**{field: ["first@example.invalid", value]}))
                                self.assert_refused(result, "permission")
                                self.assertNotIn("outside@example.invalid", result.model_dump_json())
                    draft.assert_not_called(); send.assert_not_called()
            self.assertEqual(auth.entries, 0)
        self.assertEqual(provider.requests, [])

    async def test_empty_recipient_ceiling_denies_draft_and_omission_retains_reach(self):
        for addresses, denied in (([], True), (OMITTED, False)):
            provider = DraftProvider()
            async with borrowed(provider, scoped(addresses, ("create_draft_message",))) as (client, auth, _):
                result = await client.call_tool(DRAFT, message())
                if denied:
                    self.assert_refused(result, "permission")
                else:
                    self.assertFalse(result.is_error)
                self.assertEqual(auth.entries, int(not denied))
            self.assert_draft_posts(provider, int(not denied))

    async def test_fixed_ceiling_keeps_to_cc_bcc_and_does_not_share_profiles(self):
        first, second = DraftProvider(), DraftProvider()
        first_config = scoped(["first@EXAMPLE.INVALID", "copy@example.invalid", "blind@example.invalid"],
                              ("create_draft_message",))
        async with borrowed(first, first_config) as (a, auth_a, _):
            async with borrowed(second, scoped(["second@example.invalid"], ("create_draft_message",))) as (b, auth_b, _):
                first_config["config"]["allowed_recipient_addresses"].clear()
                self.assert_refused(await a.call_tool(DRAFT, message(to=["second@example.invalid"])), "permission")
                self.assert_refused(await b.call_tool(DRAFT, message()), "permission")
                self.assertFalse((await a.call_tool(DRAFT, message(to=["First <first@Example.Invalid>"],
                    cc=["Copy <copy@example.invalid>"], bcc=["blind@example.invalid"]))).is_error)
                self.assertFalse((await b.call_tool(DRAFT, message(to=["second@example.invalid"]))).is_error)
                self.assertEqual((auth_a.entries, auth_b.entries), (1, 1))
        for provider in (first, second):
            self.assert_draft_posts(provider, 1)
        _, parsed = decode_draft(first.requests[0])
        self.assertEqual(parsed["To"].addresses[0].addr_spec, "first@Example.Invalid")
        self.assertEqual(parsed["Cc"].addresses[0].addr_spec, "copy@example.invalid")
        self.assertEqual(parsed["Bcc"].addresses[0].addr_spec, "blind@example.invalid")

    async def test_sender_and_arguments_cannot_change_account_or_credential(self):
        first, second = DraftProvider(), DraftProvider()
        async with connected(first, operations=("create_draft_message",), user="a@example.invalid",
                             token="token-A", token_env="ACCOUNT_A") as a:
            async with connected(second, operations=("create_draft_message",), user="b@example.invalid",
                                 token="token-B", token_env="ACCOUNT_B") as b:
                for client in (a, b):
                    self.assert_refused(await client.call_tool(DRAFT, message(user_id="other@example.invalid")))
                    self.assertFalse((await client.call_tool(DRAFT, message(from_address="alias@example.invalid"))).is_error)
        for provider, letter in ((first, "a"), (second, "b")):
            self.assert_draft_posts(provider, 1)
            row = provider.requests[0]
            self.assertIn("/users/" + letter + "%40example.invalid/drafts", row["url"])
            self.assertEqual(row["authorization"], "Bearer token-" + letter.upper())
            self.assertEqual(decode_draft(row)[1]["From"].addresses[0].addr_spec, "alias@example.invalid")
            self.assertEqual(provider.client_closed, 1)

    async def test_final_serialized_mime_byte_limit_is_exact_before_dispatch(self):
        value = message(subject="\u6771\u4eac" * 12, body_text="\U0001f680" * 30)
        provider = DraftProvider()
        async with connected(provider, operations=("create_draft_message",)) as client:
            self.assertFalse((await client.call_tool(DRAFT, value)).is_error)
        mime, _ = decode_draft(provider.requests[0])
        for maximum, allowed in ((len(mime), True), (len(mime) - 1, False), (1, False)):
            provider = DraftProvider()
            selected = configuration("gmail", ["create_draft_message"], max_message_bytes=maximum)
            async with borrowed(provider, selected) as (client, auth, _):
                result = await client.call_tool(DRAFT, value)
                if allowed:
                    self.assertFalse(result.is_error)
                else:
                    self.assert_refused(result)
                self.assertEqual(auth.entries, int(allowed))
            self.assert_draft_posts(provider, int(allowed))
            if allowed:
                self.assertEqual(decode_draft(provider.requests[0])[0], mime)

    async def test_empty_optional_fields_and_encoded_subject_remain_literal(self):
        provider = DraftProvider()
        subject = "=?utf-8?b?" + base64.b64encode(b"text\r\nBcc: hidden@example.invalid").decode() + "?="
        body = "Body\r\nBcc: body-only@example.invalid\r\n\x00literal"
        async with connected(provider, operations=("create_draft_message",)) as client:
            self.assertFalse((await client.call_tool(DRAFT, message(cc=[], bcc=[], subject="", body_text=""))).is_error)
            self.assertFalse((await client.call_tool(DRAFT, message(subject=subject, body_text=body))).is_error)
        self.assert_draft_posts(provider, 2)
        _, empty = decode_draft(provider.requests[0])
        self.assertIsNone(empty["Cc"]); self.assertIsNone(empty["Bcc"])
        self.assertEqual(str(empty["Subject"]), "")
        self.assertEqual(empty.get_content().replace("\r\n", "\n"), "\n")
        _, literal = decode_draft(provider.requests[1])
        self.assertEqual(str(literal["Subject"]), subject)
        self.assertIsNone(literal["Bcc"])
        self.assertEqual(literal.get_content().replace("\r\n", "\n"), body.replace("\r\n", "\n") + "\n")

    def test_raw_draft_cannot_bypass_a_configured_recipient_ceiling(self):
        for operations in (("create_draft_message", "create_draft_raw"), ("create_draft_message", "send_raw")):
            for addresses in ([], ["first@example.invalid"]):
                with self.subTest(operations=operations, addresses=addresses), self.assertRaises(ConfigurationError):
                    config_module.validate_configuration(scoped(addresses, operations))

    async def test_entered_provider_failures_are_unknown_with_one_post_and_cleanup(self):
        cases = (("timeout", "timeout"), ("accepted_disconnect", "transport"),
                 ("message_receipt", "invalid_response"), ("missing_message", "invalid_response"),
                 ("missing_draft_id", "invalid_response"), ("rate_limit", "rate_limited"),
                 ("redirect", "redirect"), ("authentication", "authentication"),
                 ("permission", "permission"), ("server", "http_status"))
        for mode, kind in cases:
            with self.subTest(mode=mode):
                provider = DraftProvider(mode)
                async with connected(provider, operations=("create_draft_message",)) as client:
                    result = await client.call_tool(DRAFT, message(body_text=PRIVATE))
                    self.assertTrue(result.is_error)
                    error = result.structured_content["error"]
                    self.assertEqual(error["kind"], kind)
                    self.assertEqual(error["effect"], "write")
                    self.assertTrue(error["operation_invoked"])
                    self.assertEqual(error["write_outcome"], "unknown")
                    self.assertNotIn(PRIVATE, result.model_dump_json())
                    self.assertNotIn("fixture-token-A", result.model_dump_json())
                    await client.list_tools()
                    self.assert_draft_posts(provider, 1)
                self.assertEqual(provider.client_closed, 1)
                if mode == "accepted_disconnect":
                    self.assertEqual(len(provider.accepted_drafts), 1)
                if mode not in {"timeout", "accepted_disconnect"}:
                    self.assertEqual(provider.events.count("response_closed"), 1)

    async def test_stdio_cancelled_active_and_queued_drafts_keep_cleanup_and_never_send(self):
        with tempfile.TemporaryDirectory() as folder:
            fixture = Fixture(Path(folder) / "draft-cancel",
                              configuration("gmail", ["create_draft_message"], max_message_bytes=8192), "gated")
            async with Client(fixture.params, mode="legacy") as client:
                active = asyncio.create_task(client.call_tool(DRAFT, message()))
                queued = following = None
                try:
                    await eventually(lambda: fixture.events("response_closing"))
                    queued = asyncio.create_task(client.call_tool(DRAFT, message(subject="Cancelled before entry")))
                    await asyncio.sleep(.05)
                    await asyncio.wait_for(client.list_tools(), 1)
                    self.assertFalse(queued.done())
                    queued.cancel()
                    with self.assertRaises(asyncio.CancelledError):
                        await queued
                    active.cancel()
                    with self.assertRaises(asyncio.CancelledError):
                        await active
                    await asyncio.wait_for(client.list_tools(), 1)
                    self.assertEqual(len(fixture.events("request")), 1)
                    self.assertFalse(fixture.events("response_closed"))
                    following = asyncio.create_task(client.call_tool(DRAFT, message(subject="Explicit subsequent draft")))
                    await asyncio.sleep(.05)
                    self.assertFalse(following.done())
                    self.assertEqual(len(fixture.events("request")), 1)
                    fixture.release.touch()
                    self.assertFalse((await asyncio.wait_for(following, 3)).is_error)
                finally:
                    fixture.release.touch()
                    await asyncio.gather(*[task for task in (active, queued, following) if task], return_exceptions=True)
            requests = fixture.events("request")
            self.assertEqual(len(requests), 2)
            self.assertTrue(all(row["method"] == "POST" and row["url"].endswith("/drafts") for row in requests))
            subjects = [str(decode_draft({"content": row["body"].encode()})[1]["Subject"]) for row in requests]
            self.assertEqual(subjects, ["Project summary", "Explicit subsequent draft"])
            events = fixture.events()
            self.assertLess(next(i for i, row in enumerate(events) if row["event"] == "response_closed"),
                            [i for i, row in enumerate(events) if row["event"] == "request"][1])
            self.assertEqual(len(fixture.events("response_closed")), 2)
            self.assertEqual(events[-1]["event"], "client_closed")
            self.assertEqual(len(fixture.events("client_closed")), 1)


if __name__ == "__main__":
    unittest.main()
