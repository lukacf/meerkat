"""Fixed Gmail recipient limits through canonical configuration and real MCP."""

import base64
from contextlib import asynccontextmanager
import importlib
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import httpx
from mcp import Client

from test_services_mcp import ConfigurationError, Fixture, adapter, config_module, configuration
from test_structured_send import Provider, decode_request, message


NAMESPACE = os.environ.get("TOOLKIT_SERVICES_TEST_NAMESPACE", "meerkat_toolkit")
prepare_httpx_client = importlib.import_module(NAMESPACE + ".httpx_client").prepare_httpx_client
OMITTED = object()


def scoped(addresses=OMITTED, operations=("send_message",)):
    options = {} if addresses is OMITTED else {"allowed_recipient_addresses": addresses}
    return configuration("gmail", operations, max_message_bytes=8192, **options)


class CountingAuth(httpx.Auth):
    def __init__(self):
        self.entries = 0

    async def async_auth_flow(self, request):
        self.entries += 1
        request.headers["Authorization"] = "Bearer inert-host-token"
        yield request


@asynccontextmanager
async def borrowed(provider, value):
    auth = CountingAuth()
    async with httpx.AsyncClient(base_url="https://gmail.googleapis.com/", auth=auth,
            transport=httpx.MockTransport(provider.handle), trust_env=False) as transport:
        prepare_httpx_client(transport)
        with patch.object(adapter, "selected_token", side_effect=AssertionError("credential lookup")):
            profile = adapter.create_profile_from_client(config_module.validate_configuration(value), transport)
        try:
            async with Client(adapter.create_server(profile)) as client:
                yield client, auth, profile
        finally:
            await profile.aclose()
            if transport.is_closed:
                raise AssertionError("Borrowed transport was closed")


class RecipientConfigurationTests(unittest.TestCase):
    def test_bare_addresses_normalize_only_ascii_domain_and_freeze(self):
        value = scoped(["Local+tag@EXAMPLE.INVALID", "local+tag@example.invalid",
                        "first@ex\u00c4mple.invalid", "\u8bfb\u8005@\u4f8b\u5b50.\u6d4b\u8bd5"])
        parsed = config_module.validate_configuration(value)
        value["config"]["allowed_recipient_addresses"].clear()
        expected = ("Local+tag@example.invalid", "local+tag@example.invalid",
                    "first@ex\u00c4mple.invalid", "\u8bfb\u8005@\u4f8b\u5b50.\u6d4b\u8bd5")
        self.assertEqual(parsed.config["allowed_recipient_addresses"], expected)
        with self.assertRaises(TypeError):
            parsed.config["allowed_recipient_addresses"] = ()
        self.assertEqual(adapter._profile_configuration(parsed).config["allowed_recipient_addresses"], expected)

    def test_invalid_configuration_has_only_fixed_error(self):
        invalid = [None, "a@example.invalid", {}, ("a@example.invalid",), [None], [False], [1], [""],
                   ["a@example.invalid", "a@EXAMPLE.INVALID"], ["Person <a@example.invalid>"],
                   ["<a@example.invalid>"], ["a@example.invalid (comment)"], [" a@example.invalid"],
                   ["a@example.invalid "], ["Team: a@example.invalid;"], ["a@example.invalid,b@example.invalid"],
                   ["a@example.invalid\r\nBcc: b@example.invalid"], ['"a\tb"@example.invalid'],
                   ["not-a-mailbox"], ["a@"], ["a@\ud800.invalid"], ["a" * 2049 + "@example.invalid"],
                   [f"reader{i}@example.invalid" for i in range(257)]]
        for addresses in invalid:
            with self.subTest(addresses=repr(addresses)[:60]), self.assertRaises(ConfigurationError) as error:
                config_module.validate_configuration(scoped(addresses))
            self.assertEqual(str(error.exception), "Invalid service MCP configuration")
            self.assertIsNone(error.exception.__context__)
        self.assertEqual(len(config_module.validate_configuration(
            scoped([f"reader{i}@example.invalid" for i in range(256)])).config["allowed_recipient_addresses"]), 256)

    def test_empty_omitted_and_optional_help_are_distinct(self):
        self.assertNotIn("allowed_recipient_addresses", config_module.validate_configuration(scoped()).config)
        self.assertEqual(config_module.validate_configuration(scoped([])).config["allowed_recipient_addresses"], ())
        self.assertEqual(config_module.configuration_help("gmail")["config_fields"]["allowed_recipient_addresses"],
                         {"kind": "string_list", "required": False})
        self.assertNotIn("allowed_recipient_addresses", config_module.configuration_help("slack")["config_fields"])
        notes = " ".join(row["instruction"] for row in config_module.configuration_help("gmail")["prerequisites"])
        for text in ("empty array", "omitting", "send_raw", "create_draft_raw", "ASCII domain case"):
            self.assertIn(text, notes)

    def test_raw_operations_and_wrong_profile_cannot_select_ceiling(self):
        for addresses in ([], ["first@example.invalid"]):
            for operation in ("send_raw", "create_draft_raw"):
                with self.subTest(addresses=addresses, operation=operation), self.assertRaises(ConfigurationError):
                    config_module.validate_configuration(scoped(addresses, ("send_message", operation)))
        with self.assertRaises(ConfigurationError):
            config_module.validate_configuration(configuration("slack", ["auth_test"],
                allowed_recipient_addresses=["first@example.invalid"]))
        self.assertEqual(config_module.validate_configuration(
            scoped(operations=("send_raw", "create_draft_raw"))).operations, ("send_raw", "create_draft_raw"))


class RecipientMCPTests(unittest.IsolatedAsyncioTestCase):
    def assert_refused(self, result, kind="permission"):
        self.assertTrue(result.is_error)
        self.assertEqual(result.structured_content["error"], {
            "kind": kind, "effect": "write", "operation_invoked": False, "write_outcome": "not_attempted"})

    async def test_each_recipient_header_is_checked_before_connector_auth_or_transport(self):
        provider = Provider()
        async with borrowed(provider, scoped(["first@example.invalid"])) as (client, auth, profile):
            with patch.object(profile.operations, "send_raw", side_effect=AssertionError("connector entry")) as raw:
                for field in ("to", "cc", "bcc"):
                    for address in ("outside@example.invalid", '"first@example.invalid" <outside@example.invalid>',
                                    "=?utf-8?b?Rmlyc3QgUmVhZGVy?= <outside@example.invalid>"):
                        with self.subTest(field=field, address=address):
                            result = await client.call_tool("gmail_send_message", message(**{
                                field: ["first@example.invalid", address]}))
                            self.assert_refused(result)
                            self.assertNotIn("outside@example.invalid", result.model_dump_json())
                raw.assert_not_called()
            self.assertEqual(auth.entries, 0)
            self.assertEqual(provider.requests, [])

    async def test_allowed_all_headers_keep_exact_mime_and_one_post(self):
        provider = Provider()
        value = scoped(["first@EXAMPLE.INVALID", "copy@example.invalid", "blind@example.invalid"])
        async with borrowed(provider, value) as (client, auth, _):
            value["config"]["allowed_recipient_addresses"][:] = ["outside@example.invalid"]
            sent = message(to=["=?utf-8?b?Rmlyc3QgUmVhZGVy?= <first@Example.Invalid>"],
                           cc=["Copy <copy@example.invalid>"], bcc=["blind@example.invalid"],
                           body_text="Bcc: body-only@example.invalid")
            self.assertFalse((await client.call_tool("gmail_send_message", sent)).is_error)
            self.assertEqual(auth.entries, 1)
        self.assertEqual(len(provider.requests), 1)
        _, mime = decode_request(provider.requests[0])
        self.assertEqual(mime["To"].addresses[0].addr_spec, "first@Example.Invalid")
        self.assertEqual(mime["Cc"].addresses[0].addr_spec, "copy@example.invalid")
        self.assertEqual(mime["Bcc"].addresses[0].addr_spec, "blind@example.invalid")
        self.assertIn("Bcc: body-only@example.invalid", mime.get_content())

    async def test_local_parts_aliases_and_nonascii_domains_are_not_folded(self):
        provider = Provider()
        addresses = ["First+tag@example.invalid", "first.last@example.invalid", "a@ex\u00c4mple.invalid",
                     "\u8bfb\u8005@\u4f8b\u5b50.\u6d4b\u8bd5", "*@literal.invalid"]
        async with borrowed(provider, scoped(addresses)) as (client, auth, _):
            for address in ("first+tag@example.invalid", "First@example.invalid", "firstlast@example.invalid",
                            "a@ex\u00e4mple.invalid", "a@xn--exmple-cua.invalid", "reader@literal.invalid"):
                self.assert_refused(await client.call_tool("gmail_send_message", message(to=[address])))
            self.assertEqual(auth.entries, 0)
            for address in addresses:
                self.assertFalse((await client.call_tool("gmail_send_message", message(to=[address]))).is_error)
            self.assertEqual(auth.entries, len(addresses))

    async def test_malformed_headers_and_injection_do_not_enter_transport(self):
        provider = Provider()
        async with borrowed(provider, scoped(["first@example.invalid"])) as (client, auth, _):
            bad = ["Team: first@example.invalid;", "first@example.invalid,outside@example.invalid",
                   "first@example.invalid\r\nBcc: outside@example.invalid", "first@example.invalid\x00"]
            for raw in (b"First\x00Injected", b"First\r\nBcc: outside@example.invalid"):
                for _ in range(3):
                    raw = b"=?utf-8?b?" + base64.b64encode(raw) + b"?="
                    bad.append(raw.decode() + " <first@example.invalid>")
            for field in ("to", "cc", "bcc"):
                for address in bad:
                    with self.subTest(field=field, address=address):
                        self.assert_refused(await client.call_tool("gmail_send_message", message(**{field: [address]})),
                                            "invalid_arguments")
            self.assertEqual(auth.entries, 0)
            self.assertEqual(provider.requests, [])

    async def test_empty_denies_sends_but_omitted_preserves_legacy(self):
        for addresses, denied in (([], True), (OMITTED, False)):
            provider = Provider()
            async with borrowed(provider, scoped(addresses)) as (client, auth, _):
                result = await client.call_tool("gmail_send_message", message())
                if denied:
                    self.assert_refused(result)
                else:
                    self.assertFalse(result.is_error)
                self.assertEqual(auth.entries, int(not denied))
            self.assertEqual(len(provider.requests), int(not denied))

    async def test_two_borrowed_profiles_do_not_share_recipient_limits(self):
        first, second = Provider(), Provider()
        async with borrowed(first, scoped(["first@example.invalid"])) as (a, auth_a, _):
            async with borrowed(second, scoped(["second@example.invalid"])) as (b, auth_b, _):
                for client, own, other in ((a, "first", "second"), (b, "second", "first")):
                    self.assert_refused(await client.call_tool("gmail_send_message", message(to=[other + "@example.invalid"])))
                    self.assertFalse((await client.call_tool("gmail_send_message", message(to=[own + "@example.invalid"]))).is_error)
                self.assertEqual((auth_a.entries, auth_b.entries), (1, 1))

    async def test_raw_calls_unavailable_and_read_operations_remain_usable_over_stdio(self):
        with tempfile.TemporaryDirectory() as folder:
            fixture = Fixture(Path(folder) / "scoped", scoped([], ("send_message", "list_messages_page")))
            async with Client(fixture.params, mode="legacy") as client:
                self.assertEqual({item.name for item in (await client.list_tools()).tools},
                                 {"gmail_send_message", "gmail_list_messages_page"})
                for operation in ("gmail_send_raw", "gmail_create_draft_raw"):
                    result = await client.call_tool(operation, {
                        "mime_base64": base64.b64encode(b"To: outside@example.invalid\r\n\r\nbody").decode()})
                    self.assertEqual(result.structured_content["error"]["kind"], "unknown_function")
                    self.assertFalse(result.structured_content["error"]["operation_invoked"])
                self.assert_refused(await client.call_tool("gmail_send_message", message()))
                self.assertEqual(fixture.events("request"), [])
                self.assertFalse((await client.call_tool("gmail_list_messages_page", {})).is_error)
                self.assertEqual(len(fixture.events("request")), 1)

    async def test_forged_configuration_refused_before_credentials_or_borrowing(self):
        forged = config_module.Configuration("gmail", ("send_raw",), scoped([])["config"])
        with patch.object(adapter, "selected_token", side_effect=AssertionError("credential lookup")) as lookup:
            with self.assertRaises(ConfigurationError):
                await adapter.create_profile(forged)
            with self.assertRaises(ConfigurationError):
                adapter.create_profile_from_client(forged, object())
            lookup.assert_not_called()

    async def test_after_entry_failures_remain_unknown_and_are_not_retried(self):
        for mode, kind in (("permission", "permission"), ("timeout", "timeout"), ("malformed", "invalid_response")):
            with self.subTest(mode=mode):
                provider = Provider(mode)
                async with borrowed(provider, scoped(["first@example.invalid"])) as (client, auth, _):
                    result = await client.call_tool("gmail_send_message", message())
                    self.assertTrue(result.is_error)
                    error = result.structured_content["error"]
                    self.assertEqual(error["kind"], kind)
                    self.assertEqual(error["write_outcome"], "unknown")
                    self.assertTrue(error["operation_invoked"])
                    await client.list_tools()
                    self.assertEqual(auth.entries, 1)
                    self.assertEqual(len(provider.requests), 1)


if __name__ == "__main__":
    unittest.main()
