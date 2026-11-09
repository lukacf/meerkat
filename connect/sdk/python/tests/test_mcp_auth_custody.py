"""Host transport custody only, with inert native replies and real loopback I/O."""
import asyncio
import gc
import unittest

from meerkat_toolkit.mcp_auth import (
    JoinedMcpLogin, McpAuthCancelled, McpAuthError, McpLoginCustodian,
    McpTarget, StartedMcpLogin,
)
from test_mcp_auth import Client, TARGET, URL, callback, closed


class McpLoginCustodianTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.loop_errors = []
        loop = asyncio.get_running_loop()
        previous = loop.get_exception_handler()
        loop.set_exception_handler(lambda _loop, context: self.loop_errors.append(context))
        self.addCleanup(loop.set_exception_handler, previous)

    async def asyncTearDown(self):
        gc.collect()
        await asyncio.sleep(0)
        self.assertEqual(self.loop_errors, [])

    def make(self, client=None, **kwargs):
        client = client or Client()
        custodian = McpLoginCustodian(client, TARGET, **kwargs)

        async def cleanup():
            for gate in (client.start_gate, client.cancel_gate, client.complete_gate):
                if gate is not None:
                    gate.set()
            try:
                await custodian.close()
            except (McpAuthError, McpAuthCancelled):
                pass

        self.addAsyncCleanup(cleanup)
        return client, custodian

    async def entered(self, client):
        async def wait():
            while client.redirect is None:
                await asyncio.sleep(0)
        await asyncio.wait_for(wait(), 1)

    async def failure(self, operation, code):
        with self.assertRaises(McpAuthError) as caught:
            await operation
        self.assertEqual(caught.exception.code, code)
        self.assertNotIn("FIXTURE", repr(caught.exception))
        return caught.exception

    async def test_pending_start_survives_cancel_and_returns_same_handle(self):
        client = Client()
        client.start_gate = asyncio.Event()
        client, custodian = self.make(client)
        first = asyncio.create_task(custodian.begin())
        await self.entered(client)
        first.cancel("FIXTURE-CALLER-PRIVATE")
        with self.assertRaises(asyncio.CancelledError) as caught:
            await first
        self.assertEqual(str(caught.exception), "mcp_auth_start_wait_cancelled")
        self.assertIsNone(caught.exception.__context__)
        self.assertIsNone(caught.exception.__cause__)
        self.assertFalse(await closed(client.redirect))
        client.start_gate.set()
        handle = await custodian.begin()
        self.assertIsInstance(handle, StartedMcpLogin)
        self.assertIs(await custodian.begin(), handle)
        seen = []
        async def presenter(url):
            seen.append(url)
        self.assertTrue(await handle.present(presenter))
        self.assertEqual(seen, [URL])
        await callback(client.redirect)
        self.assertEqual((await handle.wait()).account_id, TARGET.oauth_account)
        self.assertEqual(client.calls.count("start"), 1)
        self.assertEqual((await custodian.close()).native_cancel, "not_needed")

    async def test_concurrent_callers_share_one_start_and_one_handle(self):
        client = Client()
        client.start_gate = asyncio.Event()
        client, custodian = self.make(client)
        callers = [asyncio.create_task(custodian.begin()) for _ in range(6)]
        await self.entered(client)
        callers[0].cancel()
        with self.assertRaises(asyncio.CancelledError):
            await callers[0]
        client.start_gate.set()
        handles = await asyncio.gather(*callers[1:])
        self.assertTrue(all(handle is handles[0] for handle in handles))
        self.assertEqual(client.calls, ["start"])

    async def test_uncertain_failure_cached_as_values_without_exception_reuse(self):
        client = Client()
        client.start_error = "FIXTURE-NATIVE-PRIVATE"
        client, custodian = self.make(client)
        first = await self.failure(custodian.begin(), "start_uncertain")
        second = await self.failure(custodian.begin(), "start_uncertain")
        self.assertIsNot(first, second)
        self.assertEqual(first.cleanup, second.cleanup)
        self.assertTrue(first.cleanup.callback_closed)
        self.assertEqual(first.cleanup.native_cancel, "not_requested")
        cleanup_error = await self.failure(custodian.close(), "start_uncertain")
        self.assertEqual(cleanup_error.cleanup, first.cleanup)
        await self.failure(custodian.begin(), "custodian_closed")
        self.assertTrue(await closed(client.redirect))
        self.assertEqual(client.calls, ["start"])

    async def test_abandoned_waiter_failed_start_has_no_unhandled_exception(self):
        client = Client()
        client.start_gate = asyncio.Event()
        client.start_error = "FIXTURE-NATIVE-PRIVATE"
        client, custodian = self.make(client)
        waiter = asyncio.create_task(custodian.begin())
        await self.entered(client)
        waiter.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await waiter
        client.start_gate.set()
        await self.failure(custodian.close(), "start_uncertain")
        del waiter
        gc.collect()
        await asyncio.sleep(0)
        self.assertEqual(self.loop_errors, [])
        self.assertEqual(client.calls, ["start"])

    async def test_native_start_cancellation_is_redacted_and_cached(self):
        class CancelClient(Client):
            async def auth_mcp_login_start(self, *args, **kwargs):
                await super().auth_mcp_login_start(*args, **kwargs)
                raise asyncio.CancelledError("FIXTURE-NATIVE-CANCEL-PRIVATE")
        client, custodian = self.make(CancelClient())
        for operation in (custodian.begin, custodian.begin, custodian.close, custodian.close):
            with self.assertRaises(McpAuthCancelled) as caught:
                await operation()
            self.assertEqual(str(caught.exception), "mcp_auth_cancelled")
            self.assertIsNone(caught.exception.__context__)
            self.assertIsNone(caught.exception.__cause__)
            self.assertTrue(caught.exception.cleanup.callback_closed)
            self.assertEqual(caught.exception.cleanup.native_cancel, "not_requested")
        self.assertTrue(await closed(client.redirect))
        self.assertEqual(client.calls, ["start"])

    async def test_timeout_is_cached_without_restart(self):
        client = Client()
        client.start_gate = asyncio.Event()
        client, custodian = self.make(client, rpc_timeout=.03)
        for _ in range(2):
            error = await self.failure(custodian.begin(), "start_uncertain")
            self.assertEqual(error.cleanup.native_cancel, "not_requested")
        self.assertTrue(await closed(client.redirect))
        self.assertEqual(client.calls, ["start"])

    async def test_malformed_native_reply_retains_uncertainty(self):
        client = Client()
        client.start_override = {"mcp": {"server_name": "wrong"}}
        client, custodian = self.make(client)
        for operation in (custodian.begin, custodian.begin, custodian.close):
            error = await self.failure(operation(), "invalid_start_response_uncertain")
            self.assertEqual(error.cleanup.native_cancel, "not_requested")
        self.assertEqual(client.calls, ["start"])

    async def test_close_during_pending_start_wins_and_never_revives(self):
        client = Client()
        client.start_gate = asyncio.Event()
        client, custodian = self.make(client)
        starting = asyncio.create_task(custodian.begin())
        await self.entered(client)
        closing = asyncio.create_task(custodian.close())
        await asyncio.sleep(0)
        self.assertFalse(closing.done())
        await self.failure(custodian.begin(), "custodian_closed")
        client.start_gate.set()
        cleanup = await closing
        await self.failure(starting, "custodian_closed")
        self.assertEqual(cleanup.native_cancel, "acknowledged")
        self.assertEqual(await custodian.close(), cleanup)
        self.assertTrue(await closed(client.redirect))
        self.assertEqual(client.calls.count("start"), 1)
        self.assertEqual(sum(isinstance(x, tuple) and x[0] == "cancel" for x in client.calls), 1)

    async def test_repeated_close_waiter_cancellation_joins_pending_cleanup(self):
        client = Client()
        client.start_gate = asyncio.Event()
        client.cancel_gate = asyncio.Event()
        client, custodian = self.make(client)
        starting = asyncio.create_task(custodian.begin())
        await self.entered(client)
        closing = asyncio.create_task(custodian.close())
        await asyncio.sleep(0)
        closing.cancel("FIXTURE-CLOSE-PRIVATE")
        await asyncio.sleep(0)
        self.assertFalse(closing.done())
        client.start_gate.set()
        await asyncio.wait_for(client.cancel_entered.wait(), 1)
        closing.cancel("FIXTURE-REPEATED-CLOSE-PRIVATE")
        await asyncio.sleep(0)
        self.assertFalse(closing.done())
        client.cancel_gate.set()
        with self.assertRaises(McpAuthCancelled) as caught:
            await closing
        self.assertEqual(str(caught.exception), "mcp_auth_cancelled")
        self.assertIsNone(caught.exception.__context__)
        self.assertIsNone(caught.exception.__cause__)
        self.assertEqual(caught.exception.cleanup.native_cancel, "acknowledged")
        self.assertEqual(await custodian.close(), caught.exception.cleanup)
        await self.failure(starting, "custodian_closed")
        self.assertEqual(sum(isinstance(x, tuple) and x[0] == "cancel" for x in client.calls), 1)

    async def test_close_preserves_uncertain_native_cleanup(self):
        client = Client()
        client.cancel_error = "FIXTURE-CANCEL-PRIVATE"
        client, custodian = self.make(client)
        handle = await custodian.begin()
        cleanup = await custodian.close()
        self.assertTrue(cleanup.callback_closed)
        self.assertEqual(cleanup.native_cancel, "unconfirmed")
        self.assertEqual(await custodian.close(), cleanup)
        await self.failure(custodian.begin(), "custodian_closed")
        seen = []
        async def presenter(url):
            seen.append(url)
        await self.failure(handle.present(presenter), "presentation_no_longer_available")
        self.assertEqual(seen, [])
        self.assertEqual(sum(isinstance(x, tuple) and x[0] == "cancel" for x in client.calls), 1)

    async def test_joined_handle_never_cancels_other_custodian(self):
        client = Client()
        client.disposition = "joined"
        client, custodian = self.make(client)
        joined = await custodian.begin()
        self.assertIsInstance(joined, JoinedMcpLogin)
        self.assertIs(await custodian.begin(), joined)
        self.assertFalse(hasattr(joined, "present"))
        self.assertEqual((await custodian.close()).native_cancel, "not_requested")
        self.assertTrue(await closed(client.redirect))
        self.assertEqual(client.calls, ["start"])

    async def test_close_before_start_does_not_bind_or_call_native(self):
        client, custodian = self.make()
        self.assertEqual(client.calls, [])
        cleanup = await custodian.close()
        self.assertTrue(cleanup.callback_closed)
        self.assertEqual(cleanup.native_cancel, "not_requested")
        self.assertEqual(await custodian.close(), cleanup)
        await self.failure(custodian.begin(), "custodian_closed")
        self.assertIsNone(client.redirect)
        self.assertEqual(client.calls, [])

    async def test_two_fixed_targets_keep_accounts_and_callbacks_separate(self):
        first, a = self.make()
        second = Client()
        other = McpTarget(TARGET.server_name, TARGET.server_url, "account-b")
        b = McpLoginCustodian(second, other)
        self.addAsyncCleanup(b.close)
        handles = await asyncio.gather(a.begin(), b.begin())
        self.assertNotEqual(first.redirect, second.redirect)
        self.assertEqual(first.target, TARGET)
        self.assertEqual(second.target, other)
        with self.assertRaises(AttributeError):
            b.target = TARGET
        with self.assertRaises(AttributeError):
            b.client = first
        await asyncio.gather(callback(first.redirect), callback(second.redirect))
        observations = await asyncio.gather(*(h.wait() for h in handles))
        self.assertEqual([x.account_id for x in observations], ["account-a", "account-b"])
        self.assertEqual(first.calls[1][-1], "account-a")
        self.assertEqual(second.calls[1][-1], "account-b")

    async def test_invalid_fixed_configuration_fails_before_native_or_binding(self):
        client = Client()
        for bound in (0, -1, 301, float("nan"), float("inf"), True, "1"):
            for name in ("callback_timeout", "rpc_timeout"):
                with self.assertRaises(McpAuthError) as caught:
                    McpLoginCustodian(client, TARGET, **{name: bound})
                self.assertEqual(caught.exception.code, "invalid_wait_bound")
        for target in (McpTarget("x", "javascript:bad", "a"),
                       McpTarget("x", TARGET.server_url, ""), None):
            with self.assertRaises(McpAuthError) as caught:
                McpLoginCustodian(client, target)
            self.assertEqual(caught.exception.code, "invalid_target")
        self.assertEqual(client.calls, [])
        self.assertIsNone(client.redirect)

    async def test_repr_excludes_fixed_target_and_native_authorization_data(self):
        client, custodian = self.make()
        handle = await custodian.begin()
        rendered = repr((custodian, handle))
        for value in (TARGET.server_url, TARGET.oauth_account, URL, "FIXTURE"):
            self.assertNotIn(value, rendered)

    async def test_invalid_target_exception_has_no_private_cause_graph(self):
        target = McpTarget("fixture", "https://mcp.invalid:FIXTURE-PRIVATE-PORT", "account-a")
        with self.assertRaises(McpAuthError) as caught:
            McpLoginCustodian(Client(), target)
        error = caught.exception
        self.assertEqual(error.code, "invalid_target")
        self.assertIsNone(error.__context__)
        self.assertIsNone(error.__cause__)
        pending, seen, rendered = [error], set(), []
        while pending:
            current = pending.pop()
            if id(current) in seen:
                continue
            seen.add(id(current))
            rendered.append(repr(current))
            pending.extend(x for x in (current.__context__, current.__cause__) if x is not None)
        self.assertNotIn("FIXTURE", " ".join(rendered))


if __name__ == "__main__":
    unittest.main()
