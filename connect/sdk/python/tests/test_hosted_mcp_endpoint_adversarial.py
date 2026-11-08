"""Independent hosted boundary tests; inert native client, no provider or HTTP server.

These exercise Connect transport custody only. They do not qualify a new Core
host-attempt handle, registered native callback, or the actual Forge mount.
"""
import asyncio
import json
import time
import unittest
from urllib.parse import urlencode

from meerkat_toolkit.mcp_auth_host import HostedMcpAuthBinding
from meerkat_toolkit.mcp_auth import McpAuthCancelled, McpAuthError
from test_mcp_auth import CODE, STATE, TARGET
from test_mcp_auth_https import HttpsClient


ORIGIN = "https://forge.invalid"
REDIRECT = ORIGIN + "/host/toolkit/mcp/callback"
QUERY = urlencode({"state": STATE, "code": CODE})
PRIVATE = "FIXTURE-PRIVATE-HOST-MARKER"


class LoginContext:
    """Retained host login; fields alone cannot authenticate a substituted object."""
    def __init__(self, *, principal="person-a", scope="scope-a", org="org-a"):
        self.principal = principal
        self.login_scope_id = scope
        self.org = org
        self.origin = ORIGIN
        self.expires_at_ms = time.time_ns() // 1_000_000 + 60_000
        self.current = True

    def is_current(self):
        return self.current and self.expires_at_ms > time.time_ns() // 1_000_000


class HostedEndpointAdversarialTests(unittest.IsolatedAsyncioTestCase):
    def make(self, *, callback_timeout=2, rpc_timeout=1):
        client, context = HttpsClient(), LoginContext()
        binding = HostedMcpAuthBinding(client, TARGET, login_context=context,
            redirect_uri=REDIRECT, callback_timeout=callback_timeout,
            rpc_timeout=rpc_timeout)

        async def cleanup():
            for gate in (client.start_gate, client.complete_gate, client.cancel_gate):
                if gate is not None:
                    gate.set()
            try:
                await binding.shutdown()
            except (McpAuthError, McpAuthCancelled, RuntimeError):
                pass

        self.addAsyncCleanup(cleanup)
        return client, context, binding

    async def post(self, binding, context, *, query=QUERY, **kwargs):
        options = dict(method="POST", redirect_uri=REDIRECT, origin=ORIGIN,
                       body=json.dumps({"query": query}).encode())
        options.update(kwargs)
        return await binding.handle_callback(context, **options)

    def rejected(self, response):
        self.assertGreaterEqual(response.status, 400)
        self.assertLess(response.status, 500)
        self.assertNotIn(PRIVATE, repr(response))
        self.assertNotIn(STATE, repr(response))
        self.assertNotIn(CODE, repr(response))

    def complete_calls(self, client):
        return [call for call in client.calls if isinstance(call, tuple) and call[0] == "complete"]

    async def test_context_substitution_cannot_start_read_close_or_consume(self):
        client, context, binding = self.make()
        substitutes = [None, LoginContext(), LoginContext(principal="person-b"),
                       LoginContext(scope="scope-b"), LoginContext(org="org-b")]
        for other in substitutes:
            with self.subTest(stage="start", context=type(other).__name__):
                with self.assertRaises(RuntimeError):
                    await binding.begin(other)
        self.assertEqual(client.calls, [])
        await binding.begin(context)
        for other in substitutes:
            for name in ("present", "wait", "observe", "close"):
                with self.subTest(operation=name, context=type(other).__name__):
                    with self.assertRaises(RuntimeError):
                        await asyncio.wait_for(getattr(binding, name)(other), .2)
            self.rejected(await self.post(binding, other))
        self.assertEqual(client.calls, ["start"])
        self.assertEqual((await self.post(binding, context)).status, 202)
        self.assertEqual((await binding.wait(context)).phase, "authorized")
        self.assertEqual(len(self.complete_calls(client)), 1)

    async def test_get_and_rejected_posts_leave_valid_callback_available(self):
        client, context, binding = self.make()
        landing = await binding.handle_callback(method="GET", redirect_uri=REDIRECT)
        self.assertEqual(landing.status, 200)
        self.assertEqual(client.calls, [])
        self.rejected(await self.post(binding, context))
        await binding.begin(context)
        landing = await binding.handle_callback(method="GET", redirect_uri=REDIRECT)
        self.assertEqual(landing.status, 200)
        for overrides in (
            {"origin": "https://attacker.invalid"}, {"origin": None},
            {"redirect_uri": REDIRECT + "/other"},
            {"body": b'{"query":"a","query":"b"}'},
            {"body": b'{"query":7}'}, {"body": b'{"query":"a","target":"other"}'},
            {"body": json.dumps({"query": QUERY, "attempt_ref": "public-other"}).encode()},
            {"body": json.dumps({"query": QUERY, "host_attempt_handle": PRIVATE}).encode()},
            {"body": b'[]'}, {"body": b'{'}, {"body": b'\xff'},
            {"body": b'x' * 100_000},
        ):
            with self.subTest(overrides=tuple(overrides)):
                self.rejected(await self.post(binding, context, **overrides))
        for bad in (QUERY + "&%73tate=again", QUERY + "&code=again",
                    QUERY + "&error=denied", "state=" + STATE + "&code=%ff",
                    "state=" + STATE + "&code=%QQ", "state=wrong&code=" + CODE,
                    QUERY + "#fragment", QUERY + "\n"):
            self.rejected(await self.post(binding, context, query=bad))
        self.assertEqual(client.calls, ["start"])
        self.assertEqual((await self.post(binding, context,
            query=QUERY + "&account=other&scope=untrusted&route=unknown")).status, 202)
        result = await binding.wait(context)
        self.assertEqual(result.account_id, TARGET.oauth_account)
        self.assertEqual(result.scopes, ("read",))
        self.assertEqual(self.complete_calls(client)[0][1:3],
                         (TARGET.server_name, TARGET.server_url))

    async def test_two_concurrent_callbacks_enter_native_completion_once(self):
        client, context, binding = self.make()
        client.complete_gate = asyncio.Event()
        await binding.begin(context)
        responses = await asyncio.gather(self.post(binding, context), self.post(binding, context))
        self.assertEqual(sum(r.status == 202 for r in responses), 1)
        for response in responses:
            if response.status != 202:
                self.rejected(response)
        await asyncio.wait_for(client.complete_entered.wait(), 1)
        self.assertEqual(len(self.complete_calls(client)), 1)
        client.complete_gate.set()
        self.assertEqual((await binding.wait(context)).phase, "authorized")
        self.rejected(await self.post(binding, context))
        self.assertEqual(len(self.complete_calls(client)), 1)

    async def test_expired_login_and_callback_deadline_do_not_complete(self):
        client, context, binding = self.make(callback_timeout=.03)
        await binding.begin(context)
        context.expires_at_ms = 0
        self.rejected(await self.post(binding, context))
        with self.assertRaises(RuntimeError):
            await binding.observe(context)
        self.assertEqual(client.calls, ["start"])
        # Host shutdown still owns cleanup after the human session expires.
        await binding.shutdown()
        self.assertEqual(len(self.complete_calls(client)), 0)
        self.assertEqual(sum(isinstance(c, tuple) and c[0] == "cancel" for c in client.calls), 1)

        client, context, binding = self.make(callback_timeout=.02)
        await binding.begin(context)
        with self.assertRaises(McpAuthError) as caught:
            await asyncio.wait_for(binding.wait(context), 1)
        self.assertEqual(caught.exception.code, "callback_wait_abandoned")
        self.rejected(await self.post(binding, context))
        self.assertEqual(len(self.complete_calls(client)), 0)

    async def test_waiter_cancellation_does_not_cancel_owned_ceremony(self):
        client, context, binding = self.make()
        await binding.begin(context)
        waiter = asyncio.create_task(binding.wait(context))
        await asyncio.sleep(0)
        waiter.cancel(PRIVATE)
        with self.assertRaises(asyncio.CancelledError) as caught:
            await waiter
        self.assertNotIn(PRIVATE, str(caught.exception))
        self.assertEqual(client.calls, ["start"])
        self.assertEqual((await self.post(binding, context)).status, 202)
        self.assertEqual((await binding.wait(context)).phase, "authorized")
        self.assertFalse(any(isinstance(c, tuple) and c[0] == "cancel" for c in client.calls))

    async def test_explicit_close_joins_cleanup_despite_caller_cancellation(self):
        client, context, binding = self.make()
        client.cancel_gate = asyncio.Event()
        await binding.begin(context)
        closing = asyncio.create_task(binding.close(context))
        await asyncio.wait_for(client.cancel_entered.wait(), 1)
        closing.cancel(PRIVATE)
        await asyncio.sleep(0)
        self.assertFalse(closing.done())
        client.cancel_gate.set()
        with self.assertRaises(asyncio.CancelledError) as caught:
            await closing
        self.assertNotIn(PRIVATE, str(caught.exception))
        self.rejected(await self.post(binding, context))
        cleanup = await binding.shutdown()
        self.assertTrue(cleanup.callback_closed)
        self.assertEqual(sum(isinstance(c, tuple) and c[0] == "cancel" for c in client.calls), 1)

    async def test_revocation_during_completion_withholds_result(self):
        client, context, binding = self.make()
        client.complete_gate = asyncio.Event()
        await binding.begin(context)
        self.assertEqual((await self.post(binding, context)).status, 202)
        await asyncio.wait_for(client.complete_entered.wait(), 1)
        waiter = asyncio.create_task(binding.wait(context))
        await asyncio.sleep(0)
        context.current = False
        client.complete_gate.set()
        with self.assertRaises(RuntimeError):
            await waiter
        self.assertEqual(len(self.complete_calls(client)), 1)
        self.rejected(await self.post(binding, context))

    async def test_local_receipt_does_not_claim_uncertain_native_completion(self):
        client, context, binding = self.make()
        client.complete_error = PRIVATE + STATE + CODE
        await binding.begin(context)
        response = await self.post(binding, context)
        self.assertEqual(response.status, 202)
        self.assertNotIn("authorized", repr(response).lower())
        with self.assertRaises(McpAuthError) as caught:
            await binding.wait(context)
        self.assertEqual(caught.exception.code, "completion_uncertain")
        exposed = repr((binding, response, caught.exception, caught.exception.cleanup))
        for marker in (PRIVATE, STATE, CODE):
            self.assertNotIn(marker, exposed)
        self.assertIsNone(caught.exception.__context__)
        self.assertIsNone(caught.exception.__cause__)
        self.rejected(await self.post(binding, context))
        self.assertEqual(len(self.complete_calls(client)), 1)

    async def test_issuer_is_preserved_and_old_native_binding_is_not_used(self):
        client, context, binding = self.make()
        await binding.begin(context)
        issuer = "https://issuer.invalid/Tenant/%2F?aud=a+b&opaque=%2f"
        for bad in (QUERY + "&iss=", QUERY + "&iss=%00", QUERY + "&iss=one&%69ss=two"):
            self.rejected(await self.post(binding, context, query=bad))
        query = urlencode({"state": STATE, "code": CODE, "iss": issuer,
                           "attempt_ref": "public-other", "host_attempt_handle": PRIVATE})
        response = await self.post(binding, context, query=query)
        self.assertEqual(response.status, 202)
        # Inspect only inert fixture custody before the owned completion task runs.
        retained = binding._callback.callback.result()
        self.assertEqual(retained.issuer, issuer)
        self.assertEqual(retained.code, CODE)
        for marker in (issuer, CODE, STATE, PRIVATE):
            self.assertNotIn(marker, repr((retained, binding, response)))
        with self.assertRaises(McpAuthError) as caught:
            await binding.wait(context)
        self.assertEqual(caught.exception.code, "hosted_issuer_binding_unavailable")
        self.assertEqual(len(self.complete_calls(client)), 0)
        self.assertEqual(caught.exception.cleanup.native_cancel, "acknowledged")
        self.rejected(await self.post(binding, context, query=query))

    async def test_nonboolean_or_raising_currentness_does_not_admit(self):
        client, context, binding = self.make()
        for result in (1, "true", object()):
            context.is_current = lambda: result
            with self.assertRaises(RuntimeError):
                await binding.begin(context)
        def raising():
            raise RuntimeError(PRIVATE)
        context.is_current = raising
        with self.assertRaises(RuntimeError) as caught:
            await binding.begin(context)
        self.assertNotIn(PRIVATE, repr(caught.exception))
        self.assertIsNone(caught.exception.__context__)
        self.assertIsNone(caught.exception.__cause__)
        self.assertEqual(client.calls, [])


if __name__ == "__main__":
    unittest.main()
