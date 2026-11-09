"""Independent component proof: real MCP and Gmail owners, mock provider only."""

from __future__ import annotations

import asyncio
import base64
from contextlib import asynccontextmanager
from email import policy
from email.parser import BytesParser, Parser
import importlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import httpx
from mcp import Client

from test_services_mcp import Fixture, configuration, eventually


NAMESPACE = os.environ.get("TOOLKIT_SERVICES_TEST_NAMESPACE", "meerkat_toolkit")
adapter = importlib.import_module(NAMESPACE + ".adapters.services_mcp")
config_module = importlib.import_module(NAMESPACE + ".adapters.services_config")
PRIVATE = "fixture-provider-private-detail"


def message(**changes):
    value = {
        "from_address": "Approved sender <sender@example.invalid>",
        "to": ["First Reader <first@example.invalid>"],
        "subject": "Project summary",
        "body_text": "The project is ready for review.",
    }
    value.update(changes)
    return value


class Provider:
    """Record physical requests and response closure without network sockets."""

    def __init__(self, mode="success"):
        self.mode = mode
        self.requests = []
        self.events = []
        self.client_closed = 0
        self.settings = None

    async def handle(self, request):
        self.requests.append({"method": request.method, "url": str(request.url),
                              "authorization": request.headers.get("authorization"),
                              "content": bytes(request.content)})
        self.events.append("request")
        if self.mode == "timeout":
            raise httpx.ReadTimeout(PRIVATE, request=request)
        if self.mode == "disconnect":
            raise httpx.ReadError(PRIVATE, request=request)
        status = {"rate_limit": 429, "redirect": 302, "authentication": 401,
                  "permission": 403}.get(self.mode, 200)
        payload = {"id": "message-1", "threadId": "thread-1"}
        if self.mode in {"malformed", "rate_limit", "redirect", "authentication", "permission"}:
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

    def factory(self, **kwargs):
        self.settings = kwargs
        provider = self

        class OwnedClient(httpx.AsyncClient):
            async def aclose(self):
                await super().aclose()
                provider.client_closed += 1
                provider.events.append("client_closed")

        return OwnedClient(transport=httpx.MockTransport(self.handle), **kwargs)


@asynccontextmanager
async def connected(provider, *, operations=("send_message",), user="routed@example.invalid",
                    token="fixture-token-A", token_env="SELECTED_TOKEN", maximum=8192):
    configuration = config_module.validate_configuration({"profile": "gmail", "operations": list(operations),
        "config": {"token_env": token_env, "user_id": user, "timeout_seconds": 2,
                   "max_response_bytes": 4096, "max_message_bytes": maximum}})
    with patch.dict(os.environ, {token_env: token}, clear=True):
        profile = await adapter.create_profile(configuration, client_factory=provider.factory)
    try:
        async with Client(adapter.create_server(profile)) as client:
            yield client
    finally:
        await profile.aclose()


def decode_request(request):
    payload = json.loads(request["content"])
    if set(payload) != {"raw"}:
        raise AssertionError("Structured send added an unrequested API routing field")
    mime = base64.urlsafe_b64decode(payload["raw"])
    # SMTPUTF8 header octets are literal UTF-8. BytesParser exposes those in
    # address objects as surrogateescape; decoding first avoids that artifact.
    return mime, Parser(policy=policy.SMTPUTF8).parsestr(mime.decode("utf-8"))


class StructuredSendTests(unittest.IsolatedAsyncioTestCase):
    async def test_unicode_message_has_exact_explicit_envelope_and_one_send(self):
        value = message(from_address="\u00c5sa Sender <sender@example.invalid>",
                        to=["Zo\u00eb <first@example.invalid>", "second@example.invalid"],
                        cc=["\u00c9quipe <team@example.invalid>"], bcc=["audit@example.invalid"],
                        subject="Weekly summary: caf\u00e9, \u6771\u4eac, \U0001f680",
                        body_text="Hej team!\n\nCaf\u00e9: 42\n\u6771\u4eac: ready \U0001f680\nhttps://example.invalid/notes")
        provider = Provider()
        async with connected(provider) as client:
            listing = await client.list_tools()
            self.assertEqual([tool.name for tool in listing.tools], ["gmail_send_message"])
            tool = listing.tools[0]
            self.assertFalse(tool.annotations.read_only_hint)
            self.assertTrue(tool.annotations.destructive_hint)
            self.assertFalse(tool.annotations.idempotent_hint)
            result = await client.call_tool("gmail_send_message", value)
            self.assertFalse(result.is_error)
            self.assertEqual(result.structured_content, {"value": {"id": "message-1", "threadId": "thread-1"}})
        self.assertEqual(len(provider.requests), 1)
        request = provider.requests[0]
        self.assertEqual(request["method"], "POST")
        self.assertEqual(request["url"], "https://gmail.googleapis.com/gmail/v1/users/routed%40example.invalid/messages/send")
        self.assertEqual(request["authorization"], "Bearer fixture-token-A")
        mime, parsed = decode_request(request)
        self.assertEqual(parsed.defects, [])
        self.assertEqual([a.addr_spec for a in parsed["From"].addresses], ["sender@example.invalid"])
        self.assertEqual(parsed["From"].addresses[0].display_name, "\u00c5sa Sender")
        self.assertEqual([a.addr_spec for a in parsed["To"].addresses], ["first@example.invalid", "second@example.invalid"])
        self.assertEqual([a.addr_spec for a in parsed["Cc"].addresses], ["team@example.invalid"])
        self.assertEqual([a.addr_spec for a in parsed["Bcc"].addresses], ["audit@example.invalid"])
        self.assertEqual(str(parsed["Subject"]), value["subject"])
        self.assertEqual(parsed.get_content_type(), "text/plain")
        self.assertEqual(parsed.get_content_charset(), "utf-8")
        self.assertEqual(parsed.get_content().replace("\r\n", "\n"), value["body_text"] + "\n")
        self.assertEqual(BytesParser(policy=policy.default).parsebytes(mime).get_payload(decode=True),
                         parsed.get_payload(decode=True))
        self.assertEqual(list(parsed.iter_attachments()), [])
        self.assertEqual(set(parsed.keys()), {"From", "To", "Cc", "Bcc", "Subject", "Content-Type", "Content-Transfer-Encoding", "MIME-Version"})
        self.assertNotIn(b"\n", mime.replace(b"\r\n", b""))
        self.assertLessEqual(len(mime), 8192)
        self.assertFalse(provider.settings["trust_env"])
        self.assertFalse(provider.settings["follow_redirects"])
        self.assertEqual(provider.events.count("response_closed"), 1)
        self.assertEqual(provider.client_closed, 1)

    async def test_empty_optional_recipients_and_empty_text_do_not_invent_content(self):
        provider = Provider()
        async with connected(provider) as client:
            result = await client.call_tool("gmail_send_message", message(cc=[], bcc=[], subject="", body_text=""))
            self.assertFalse(result.is_error)
        _, parsed = decode_request(provider.requests[0])
        self.assertIsNone(parsed["Cc"])
        self.assertIsNone(parsed["Bcc"])
        self.assertEqual(str(parsed["Subject"]), "")
        self.assertEqual(parsed.get_content().replace("\r\n", "\n"), "\n")

    async def test_international_mailboxes_are_not_replaced_or_inferred(self):
        provider = Provider()
        value = message(from_address="\u00c5sa <\u00e5sa@ex\u00e4mple.invalid>",
                        to=["\u53d7\u4fe1\u8005 <\u8bfb\u8005@\u4f8b\u5b50.\u6d4b\u8bd5>"])
        async with connected(provider) as client:
            self.assertFalse((await client.call_tool("gmail_send_message", value)).is_error)
        _, parsed = decode_request(provider.requests[0])
        self.assertEqual(parsed["From"].addresses[0].addr_spec, "\u00e5sa@ex\u00e4mple.invalid")
        self.assertEqual(parsed["To"].addresses[0].addr_spec, "\u8bfb\u8005@\u4f8b\u5b50.\u6d4b\u8bd5")
        self.assertIn("/users/routed%40example.invalid/", provider.requests[0]["url"])

    async def test_encoded_word_subject_is_literal_and_body_cannot_add_headers(self):
        provider = Provider()
        subjects = ["=?utf-8?b?Qm9iAEluamVjdGVk?=", "=?utf-8?b?" +
                    base64.b64encode(b"text\r\nBcc: hidden@example.invalid").decode() + "?="]
        body = "Ordinary body\r\nBcc: body-only@example.invalid\r\n\x00literal text"
        async with connected(provider) as client:
            for subject in subjects:
                self.assertFalse((await client.call_tool("gmail_send_message", message(subject=subject, body_text=body))).is_error)
        for request, subject in zip(provider.requests, subjects):
            _, parsed = decode_request(request)
            self.assertEqual(str(parsed["Subject"]), subject)
            self.assertIsNone(parsed["Bcc"])
            self.assertEqual(parsed.get_content().replace("\r\n", "\n"), body.replace("\r\n", "\n") + "\n")

    async def test_invalid_fields_and_header_injection_are_rejected_before_provider(self):
        bad = []
        for field in ("from_address", "to", "subject", "body_text"):
            absent = message(); del absent[field]; bad.append(absent)
        for field in ("from_address", "subject"):
            for text in ("x\r\nBcc: thief@example.invalid", "x\nBcc: thief@example.invalid", "x\x00y"):
                bad.append(message(**{field: text}))
        for field in ("to", "cc", "bcc"):
            for address in ("ok@example.invalid\r\nBcc: thief@example.invalid", "a@example.invalid,b@example.invalid",
                            "Team: a@example.invalid;", "not-an-address", "a@example.invalid\x00"):
                bad.append(message(**{field: [address]}))
            bad.append(message(**{field: "a@example.invalid"}))
        for clear in (b"Bob\x00Injected", b"Bob\r\nBcc: injected@example.invalid"):
            encoded = clear
            for _ in range(3):
                encoded = b"=?utf-8?b?" + base64.b64encode(encoded) + b"?="
                address = encoded.decode() + " <a@example.invalid>"
                bad.append(message(from_address=address))
                bad.extend(message(**{field: [address]}) for field in ("to", "cc", "bcc"))
        bad.extend([message(to=[]), message(to=[1]), message(from_address=None), message(subject=3),
                    message(body_text=False), message(body_text="\ud800"),
                    message(headers={"Bcc": "thief@example.invalid"}), message(thread_id="thread-2"),
                    message(user_id="other@example.invalid"), message(token="secret"), message(html="<b>x</b>")])
        provider = Provider()
        async with connected(provider) as client:
            for args in bad:
                with self.subTest(args=repr(args)):
                    result = await client.call_tool("gmail_send_message", args)
                    self.assertTrue(result.is_error)
                    self.assertEqual(result.structured_content["error"]["kind"], "invalid_arguments")
                    self.assertFalse(result.structured_content["error"]["operation_invoked"])
                    self.assertEqual(result.structured_content["error"]["write_outcome"], "not_attempted")
                    self.assertEqual(provider.requests, [])

    async def test_serialized_mime_byte_limit_is_checked_before_dispatch(self):
        value = message(subject="\u6771\u4eac" * 12, body_text="\U0001f680" * 30)
        successful = Provider()
        async with connected(successful) as client:
            self.assertFalse((await client.call_tool("gmail_send_message", value)).is_error)
        mime, _ = decode_request(successful.requests[0])
        for limit, accepted in ((len(mime), True), (len(mime) - 1, False), (1, False)):
            provider = Provider()
            async with connected(provider, maximum=limit) as client:
                result = await client.call_tool("gmail_send_message", value)
                self.assertEqual(result.is_error, not accepted)
                if not accepted:
                    self.assertEqual(result.structured_content["error"]["write_outcome"], "not_attempted")
                    self.assertFalse(result.structured_content["error"]["operation_invoked"])
            self.assertEqual(len(provider.requests), int(accepted))

    async def test_explicit_sender_does_not_change_selected_account_or_credential(self):
        first, second = Provider(), Provider()
        async with connected(first, user="mailbox-a@example.invalid", token="token-A", token_env="ACCOUNT_A") as a:
            async with connected(second, user="mailbox-b@example.invalid", token="token-B", token_env="ACCOUNT_B") as b:
                for client in (a, b):
                    result = await client.call_tool("gmail_send_message", message(from_address="approved-alias@example.invalid"))
                    self.assertFalse(result.is_error)
        for provider, suffix in ((first, "a"), (second, "b")):
            self.assertEqual(len(provider.requests), 1)
            request = provider.requests[0]
            self.assertIn("/users/mailbox-" + suffix + "%40example.invalid/", request["url"])
            self.assertEqual(request["authorization"], "Bearer token-" + suffix.upper())
            self.assertEqual(decode_request(request)[1]["From"].addresses[0].addr_spec, "approved-alias@example.invalid")
            self.assertEqual(provider.client_closed, 1)

    async def test_structured_and_raw_operations_are_independently_selected(self):
        for allowed, refused in (("send_message", ("gmail_send_raw", "gmail_create_draft_raw", "gmail_get_message")),
                                 ("send_raw", ("gmail_send_message",)),
                                 ("get_message", ("gmail_send_message",))):
            provider = Provider()
            async with connected(provider, operations=(allowed,)) as client:
                self.assertEqual([t.name for t in (await client.list_tools()).tools], ["gmail_" + allowed])
                for name in refused:
                    result = await client.call_tool(name, message())
                    self.assertTrue(result.is_error)
                    self.assertEqual(result.structured_content["error"]["kind"], "unknown_function")
            self.assertEqual(provider.requests, [])

    async def test_entered_failures_are_unknown_and_never_replayed(self):
        for mode, kind in (("timeout", "timeout"), ("disconnect", "transport"), ("malformed", "invalid_response"),
                           ("rate_limit", "rate_limited"), ("redirect", "redirect"),
                           ("authentication", "authentication"), ("permission", "permission")):
            with self.subTest(mode=mode):
                provider = Provider(mode)
                async with connected(provider) as client:
                    result = await client.call_tool("gmail_send_message", message(body_text=PRIVATE))
                    self.assertTrue(result.is_error)
                    error = result.structured_content["error"]
                    self.assertEqual(error["kind"], kind)
                    self.assertTrue(error["operation_invoked"])
                    self.assertEqual(error["write_outcome"], "unknown")
                    self.assertNotIn(PRIVATE, result.model_dump_json())
                    self.assertNotIn("fixture-token-A", result.model_dump_json())
                    self.assertEqual(len(provider.requests), 1)
                    # Listing remains usable but does not recover or replay the write.
                    await client.list_tools()
                    self.assertEqual(len(provider.requests), 1)
                self.assertEqual(provider.client_closed, 1)
                if mode not in {"timeout", "disconnect"}:
                    self.assertEqual(provider.events.count("response_closed"), 1)

    async def test_cancelled_send_keeps_cleanup_and_excludes_following_send_until_closed(self):
        # Cancellation crosses the real stdio protocol, reusing the existing
        # fixture's response-close gate instead of in-process SDK cancellation.
        with tempfile.TemporaryDirectory() as folder:
            fixture = Fixture(Path(folder) / "structured-cancel",
                              configuration("gmail", ["send_message"], max_message_bytes=8192), "gated")
            async with Client(fixture.params, mode="legacy") as client:
                active = asyncio.create_task(client.call_tool("gmail_send_message", message()))
                following = None
                try:
                    await eventually(lambda: fixture.events("response_closing"))
                    active.cancel()
                    with self.assertRaises(asyncio.CancelledError):
                        await active
                    await asyncio.wait_for(client.send_ping(), 1)
                    self.assertEqual(len(fixture.events("request")), 1)
                    self.assertFalse(fixture.events("response_closed"))
                    following = asyncio.create_task(client.call_tool("gmail_send_message", message(subject="Explicit second request")))
                    await asyncio.sleep(.05)
                    self.assertFalse(following.done())
                    self.assertEqual(len(fixture.events("request")), 1)
                    fixture.release.touch()
                    self.assertFalse((await asyncio.wait_for(following, 3)).is_error)
                finally:
                    fixture.release.touch()
                    await asyncio.gather(*[task for task in (active, following) if task], return_exceptions=True)
            events = fixture.events()
            self.assertEqual(len(fixture.events("request")), 2)
            self.assertLess(next(i for i, row in enumerate(events) if row["event"] == "response_closed"),
                            [i for i, row in enumerate(events) if row["event"] == "request"][1])
            self.assertEqual(len(fixture.events("client_closed")), 1)
            self.assertEqual(events[-1]["event"], "client_closed")


if __name__ == "__main__":
    unittest.main()
