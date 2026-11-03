"""Host callback handoff only. No HTTPS server, native process or provider runs."""
import asyncio
import unittest
from unittest.mock import patch
from urllib.parse import urlencode

from meerkat_toolkit.mcp_auth import (
    JoinedMcpLogin, McpAuthCancelled, McpAuthError, McpLoginCustodian,
    McpTarget, RegisteredHttpsCallback, begin_mcp_login,
)
from test_mcp_auth import Client, CODE, STATE, TARGET, URL


REDIRECT = "https://protected.invalid:443/connect/%63allback"
QUERY = urlencode({"state": STATE, "code": CODE})


class HttpsClient(Client):
    """Existing SDK signatures with an inert accepted-profile reply, not .51."""

    def __init__(self):
        super().__init__()
        self.start_entered = asyncio.Event()

    async def auth_mcp_login_start(self, name, url, redirect_uri, *, oauth_account):
        self.calls.append("start")
        self.target = McpTarget(name, url, oauth_account)
        self.redirect = redirect_uri
        self.start_entered.set()
        if self.start_gate:
            await self.start_gate.wait()
        if self.start_error:
            raise RuntimeError(self.start_error)
        result = {"mcp": self.target._wire(), "state": STATE, "authorize_url": URL,
                  "redirect_uri": redirect_uri, "disposition": self.disposition}
        if self.start_override:
            result.update(self.start_override)
        return result


class RegisteredCallbackTests(unittest.IsolatedAsyncioTestCase):
    def make(self, client=None, **kwargs):
        client = client or HttpsClient()
        inbox = RegisteredHttpsCallback(TARGET, REDIRECT)
        custodian = McpLoginCustodian(client, TARGET, callback=inbox, **kwargs)

        async def cleanup():
            for gate in (client.start_gate, client.complete_gate, client.cancel_gate):
                if gate is not None:
                    gate.set()
            try:
                await custodian.close()
            except (McpAuthError, McpAuthCancelled):
                pass

        self.addAsyncCleanup(cleanup)
        return client, inbox, custodian

    async def test_exact_handoff_arms_before_presentation_and_completes_once(self):
        client, inbox, custodian = self.make()
        client.complete_gate = asyncio.Event()
        self.assertFalse(inbox.receive(redirect_uri=REDIRECT, query=QUERY))
        with patch("asyncio.start_server", side_effect=AssertionError("unexpected listener")):
            login = await custodian.begin()
        self.assertEqual(client.redirect, REDIRECT)

        async def protected_presenter(url):
            self.assertEqual(url, URL)
            self.assertTrue(inbox.receive(redirect_uri=REDIRECT, query=QUERY))

        self.assertTrue(await login.present(protected_presenter))
        await asyncio.wait_for(client.complete_entered.wait(), 1)
        self.assertFalse(inbox.receive(redirect_uri=REDIRECT, query=QUERY))
        client.complete_gate.set()
        self.assertEqual((await login.wait()).account_id, TARGET.oauth_account)
        self.assertEqual(client.calls, ["start", (
            "complete", TARGET.server_name, TARGET.server_url, CODE, STATE,
            REDIRECT, TARGET.oauth_account)])
        self.assertTrue((await custodian.close()).callback_closed)
        self.assertFalse(inbox.receive(redirect_uri=REDIRECT, query=QUERY))

    async def test_bad_queries_do_not_consume_the_valid_callback(self):
        client, inbox, custodian = self.make()
        login = await custodian.begin()
        queries = [
            "state=wrong&code=" + CODE,
            QUERY + "&state=again", QUERY + "&%73tate=again",
            QUERY + "&code=again", QUERY + "&error=denied",
            "state=" + STATE + "&code=%QQ",
            "state=" + STATE + "&code=%ff",
            "state=" + STATE + "&code=",
            QUERY + "&extra=1&extra=2", QUERY + "#fragment",
            QUERY + "\n", QUERY + "&x=" + "z" * 8192,
            "&".join(f"x{i}=1" for i in range(33)),
        ]
        for query in queries:
            with self.subTest(query=query):
                self.assertFalse(inbox.receive(redirect_uri=REDIRECT, query=query))
        # Equivalent-looking URI spellings are not the host's exact registration.
        self.assertFalse(inbox.receive(
            redirect_uri="https://protected.invalid/connect/callback", query=QUERY))
        self.assertEqual(client.calls, ["start"])
        self.assertTrue(inbox.receive(
            redirect_uri=REDIRECT, query=QUERY + "&account=other&scope=untrusted"))
        result = await login.wait()
        self.assertEqual(result.account_id, TARGET.oauth_account)
        self.assertEqual(result.scopes, ("read",))

    async def test_pending_start_waiter_cancellation_retains_inbox(self):
        client, inbox, custodian = self.make()
        client.start_gate = asyncio.Event()
        waiter = asyncio.create_task(custodian.begin())
        await asyncio.wait_for(client.start_entered.wait(), 1)
        self.assertFalse(inbox.receive(redirect_uri=REDIRECT, query=QUERY))
        waiter.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await waiter
        client.start_gate.set()
        login = await custodian.begin()
        self.assertIs(await custodian.begin(), login)
        self.assertTrue(inbox.receive(redirect_uri=REDIRECT, query=QUERY))
        self.assertEqual((await login.wait()).phase, "authorized")
        self.assertEqual(client.calls.count("start"), 1)
        self.assertFalse(any(isinstance(x, tuple) and x[0] == "cancel" for x in client.calls))

    async def test_denial_and_explicit_close_keep_native_cleanup_separate(self):
        client, inbox, custodian = self.make()
        login = await custodian.begin()
        self.assertTrue(inbox.receive(redirect_uri=REDIRECT, query=urlencode({
            "state": STATE, "error": "access_denied", "error_description": "FIXTURE-PRIVATE"})))
        with self.assertRaises(McpAuthError) as caught:
            await login.wait()
        self.assertEqual(caught.exception.code, "authorization_denied")
        self.assertEqual(caught.exception.cleanup.native_cancel, "acknowledged")
        self.assertTrue(caught.exception.cleanup.callback_closed)
        self.assertEqual(len(client.calls), 2)
        self.assertFalse(inbox.receive(redirect_uri=REDIRECT, query=QUERY))

        client, inbox, custodian = self.make()
        client.complete_gate = asyncio.Event()
        client.cancel_error = "FIXTURE-NATIVE-PRIVATE"
        login = await custodian.begin()
        self.assertTrue(inbox.receive(redirect_uri=REDIRECT, query=QUERY))
        await asyncio.wait_for(client.complete_entered.wait(), 1)
        cleanup = await custodian.close()
        self.assertTrue(cleanup.callback_closed)
        self.assertEqual(cleanup.native_cancel, "unconfirmed")
        with self.assertRaises(McpAuthError) as caught:
            await login.wait()
        self.assertEqual(caught.exception.code, "completion_uncertain")
        self.assertEqual(await custodian.close(), cleanup)
        self.assertEqual(len(client.calls), 3)
        self.assertFalse(inbox.receive(redirect_uri=REDIRECT, query=QUERY))

    async def test_join_and_invalid_returned_binding_never_cancel_another_attempt(self):
        client, inbox, custodian = self.make()
        client.disposition = "joined"
        joined = await custodian.begin()
        self.assertIsInstance(joined, JoinedMcpLogin)
        self.assertFalse(hasattr(joined, "present"))
        self.assertFalse(hasattr(joined, "close"))
        self.assertTrue((await custodian.close()).callback_closed)
        self.assertFalse(inbox.receive(redirect_uri=REDIRECT, query=QUERY))
        self.assertEqual(client.calls, ["start"])

        for override in ({"redirect_uri": "https://other.invalid/callback"},
                         {"mcp": McpTarget("other", TARGET.server_url, TARGET.oauth_account)._wire()}):
            with self.subTest(override=override):
                client, inbox, custodian = self.make()
                client.start_override = override
                for operation in (custodian.begin, custodian.begin, custodian.close):
                    with self.assertRaises(McpAuthError) as caught:
                        await operation()
                    self.assertEqual(caught.exception.code, "invalid_start_response_uncertain")
                    self.assertTrue(caught.exception.cleanup.callback_closed)
                    self.assertEqual(caught.exception.cleanup.native_cancel, "not_requested")
                self.assertFalse(inbox.receive(redirect_uri=REDIRECT, query=QUERY))
                self.assertEqual(client.calls, ["start"])

    async def test_unsupported_native_and_timeout_are_cached_without_fallback(self):
        for mode in ("unsupported", "timeout"):
            client, inbox, custodian = self.make(rpc_timeout=.02)
            if mode == "unsupported":
                client.start_error = "FIXTURE loopback-only native refusal"
            else:
                client.start_gate = asyncio.Event()
            for operation in (custodian.begin, custodian.begin, custodian.close):
                with self.assertRaises(McpAuthError) as caught:
                    await operation()
                self.assertEqual(caught.exception.code, "start_uncertain")
                self.assertEqual(caught.exception.cleanup.native_cancel, "not_requested")
                self.assertTrue(caught.exception.cleanup.callback_closed)
            self.assertFalse(inbox.receive(redirect_uri=REDIRECT, query=QUERY))
            self.assertEqual(client.calls, ["start"])

    async def test_invalid_uri_and_client_refuse_before_native_start(self):
        for uri in ("http://protected.invalid/callback", REDIRECT + "?x=fixed",
                    REDIRECT + "?", REDIRECT + "#", REDIRECT + "#fragment",
                    "https://user:FIXTURE-SECRET@protected.invalid/callback",
                    "https://protected.invalid:FIXTURE-SECRET/callback"):
            with self.subTest(uri=uri):
                with self.assertRaises(McpAuthError) as caught:
                    RegisteredHttpsCallback(TARGET, uri)
                self.assertEqual(caught.exception.code, "invalid_callback_uri")
                self.assertIsNone(caught.exception.__context__)
                self.assertIsNone(caught.exception.__cause__)
        inbox = RegisteredHttpsCallback(TARGET, REDIRECT)
        with self.assertRaises(McpAuthError) as caught:
            await begin_mcp_login(object(), TARGET, callback=inbox)
        self.assertEqual(caught.exception.code, "unsupported_client")
        self.assertTrue(caught.exception.cleanup.callback_closed)
        self.assertFalse(inbox.receive(redirect_uri=REDIRECT, query=QUERY))

    async def test_close_before_begin_leaves_supplied_inbox_unclaimed(self):
        client, inbox, custodian = self.make()
        cleanup = await custodian.close()
        self.assertFalse(cleanup.callback_closed)
        self.assertEqual(cleanup.native_cancel, "not_requested")
        self.assertEqual(client.calls, [])
        with self.assertRaises(McpAuthError) as caught:
            await custodian.begin()
        self.assertEqual(caught.exception.code, "custodian_closed")
        # The never-started custodian did not claim or retire this host inbox.
        login = await begin_mcp_login(client, TARGET, callback=inbox)
        self.addAsyncCleanup(login.close)
        self.assertTrue(inbox.receive(redirect_uri=REDIRECT, query=QUERY))
        self.assertEqual((await login.wait()).account_id, TARGET.oauth_account)
        self.assertEqual(client.calls.count("start"), 1)


if __name__ == "__main__":
    unittest.main()
