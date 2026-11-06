"""Inert hosted transport checks, not native HTTPS or provider qualification."""
import asyncio
import json
import unittest
from urllib.parse import urlencode

from meerkat_toolkit.mcp_auth import McpAuthCancelled, McpAuthError, NativeAuthFailure
from meerkat_toolkit.mcp_auth_host import HostedMcpAuthBinding, McpHostError
from test_mcp_auth import CODE, STATE, TARGET
from test_mcp_auth_https import HttpsClient

REDIRECT = "https://protected.invalid/connect/callback"
ORIGIN = "https://protected.invalid"
BODY = json.dumps({"query": urlencode({"state": STATE, "code": CODE})}).encode()


class Login:
    origin = ORIGIN

    def __init__(self):
        self.current = True

    def is_current(self):
        return self.current


class HostedCallbackTests(unittest.IsolatedAsyncioTestCase):
    def make(self):
        client, login = HttpsClient(), Login()
        binding = HostedMcpAuthBinding(client, TARGET, login_context=login,
            redirect_uri=REDIRECT)

        async def cleanup():
            for gate in (client.start_gate, client.complete_gate, client.cancel_gate):
                if gate is not None:
                    gate.set()
            try:
                await binding.shutdown()
            except McpAuthError:
                pass
        self.addAsyncCleanup(cleanup)
        return client, login, binding

    async def post(self, binding, login, body=BODY, **kwargs):
        args = dict(method="POST", redirect_uri=REDIRECT, origin=ORIGIN, body=body)
        args.update(kwargs)
        return await binding.handle_callback(login, **args)

    async def test_public_landing_does_not_consume_and_authenticated_post_completes(self):
        client, login, binding = self.make()
        self.assertEqual(await binding.begin(login), "started")
        landing = await binding.handle_callback(method="GET", redirect_uri=REDIRECT)
        self.assertEqual(landing.status, 200)
        self.assertEqual(client.calls, ["start"])
        self.assertIn(b"history.replaceState", landing.body)
        self.assertIn(b"credentials: 'same-origin'", landing.body)
        self.assertEqual(dict(landing.headers)["Referrer-Policy"], "no-referrer")
        accepted = await self.post(binding, login)
        self.assertEqual(accepted.status, 202)
        self.assertEqual((await binding.wait(login)).account_id, TARGET.oauth_account)
        self.assertEqual((await self.post(binding, login)).status, 400)
        for marker in (STATE, CODE):
            self.assertNotIn(marker, repr((binding, landing, accepted)))

    async def test_identity_origin_uri_and_payload_refusals_leave_callback_usable(self):
        client, login, binding = self.make()
        await binding.begin(login)
        self.assertEqual((await self.post(binding, Login())).status, 403)
        self.assertEqual((await self.post(binding, None)).status, 403)
        self.assertEqual((await self.post(binding, login, origin="https://other.invalid")).status, 403)
        self.assertEqual((await self.post(binding, login, redirect_uri=REDIRECT + "/other")).status, 404)
        for body in (b"", b"x" * 65537, b'{"query":"x","query":"y"}',
                     b'{"query":"x","account":"other"}', b'{"query":null}',
                     b'[]', b'{"query":"%ff"}', b'\xff'):
            self.assertEqual((await self.post(binding, login, body)).status, 400)
        self.assertEqual(client.calls, ["start"])
        self.assertEqual((await self.post(binding, login)).status, 202)
        await binding.wait(login)

    async def test_cancelled_waiter_keeps_attempt_and_expired_login_cannot_read_result(self):
        client, login, binding = self.make()
        client.complete_gate = asyncio.Event()
        await binding.begin(login)
        waiting = asyncio.create_task(binding.wait(login))
        await asyncio.sleep(0)
        waiting.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await waiting
        self.assertEqual((await self.post(binding, login)).status, 202)
        await client.complete_entered.wait()
        waiting = asyncio.create_task(binding.wait(login))
        await asyncio.sleep(0)
        login.current = False
        client.complete_gate.set()
        with self.assertRaises(McpHostError) as caught:
            await waiting
        self.assertEqual(str(caught.exception), "host_admission_unconfirmed")
        cleanup = await binding.shutdown()
        self.assertEqual(cleanup.native_cancel, "not_needed")
        self.assertFalse(any(isinstance(call, tuple) and call[0] == "cancel" for call in client.calls))

    async def test_explicit_close_retires_once_and_reports_native_uncertainty(self):
        client, login, binding = self.make()
        client.cancel_error = "FIXTURE-CANCEL-PRIVATE"
        await binding.begin(login)
        cleanup = await binding.close(login)
        self.assertTrue(cleanup.callback_closed)
        self.assertEqual(cleanup.native_cancel, "unconfirmed")
        self.assertEqual(await binding.shutdown(), cleanup)
        self.assertEqual((await self.post(binding, login)).status, 410)
        self.assertNotIn("FIXTURE-CANCEL-PRIVATE", repr(cleanup))
        self.assertEqual(sum(isinstance(call, tuple) and call[0] == "cancel" for call in client.calls), 1)

    async def test_revoked_context_does_not_start_or_cancel_owned_attempt(self):
        client, login, binding = self.make()
        login.current = False
        for method in (binding.begin, binding.present, binding.wait, binding.observe, binding.close):
            with self.assertRaises(McpHostError):
                await method(login)
        self.assertEqual(client.calls, [])
        login.current = True
        await binding.begin(login)
        with self.assertRaises(McpHostError):
            await binding.close(Login())
        self.assertEqual(client.calls, ["start"])
        login.current = False
        self.assertEqual((await binding.shutdown()).native_cancel, "acknowledged")

    async def test_hosted_issuer_is_retained_and_not_silently_sent_to_old_completion(self):
        client, login, binding = self.make()
        await binding.begin(login)
        issuer = "https://issuer.invalid/CaseSensitive%2Fissuer"
        body = json.dumps({"query": urlencode({"state": STATE, "code": CODE,
                                               "iss": issuer})}).encode()
        self.assertEqual((await self.post(binding, login, body)).status, 202)
        received = binding._callback.callback.result()
        self.assertEqual(received.issuer, issuer)
        self.assertNotIn(issuer, repr(received))
        self.assertNotIn(CODE, repr(received))
        with self.assertRaises(McpAuthError) as caught:
            await binding.wait(login)
        self.assertEqual(caught.exception.code, "hosted_issuer_binding_unavailable")
        self.assertEqual(caught.exception.cleanup.native_cancel, "acknowledged")
        self.assertFalse(any(isinstance(call, tuple) and call[0] == "complete"
                             for call in client.calls))

    async def test_failed_start_still_joins_owned_native_cleanup_and_preserves_error(self):
        for malformed in (False, True):
            with self.subTest(malformed=malformed):
                client, login, binding = self.make()
                if malformed:
                    client.start_override = {"authorize_url": "not-a-provider-url"}
                    expected = "invalid_start_response_uncertain"
                else:
                    client.start_error = "PRIVATE-START-FAILURE"
                    expected = "start_uncertain"
                closed = []
                async def close_native():
                    closed.append(True)
                client.close_owned_hosted_login = close_native
                with self.assertRaises(McpAuthError) as caught:
                    await binding.begin(login)
                self.assertEqual(caught.exception.code, expected)
                for _ in range(2):
                    with self.assertRaises(McpAuthError) as caught:
                        await binding.shutdown()
                    self.assertEqual(caught.exception.code, expected)
                    self.assertIsNone(caught.exception.__context__)
                    self.assertNotIn("PRIVATE-START-FAILURE", repr(caught.exception))
                self.assertEqual(closed, [True])
                self.assertEqual(client.calls, ["start"])

    async def test_shutdown_joins_pending_start_before_native_cleanup(self):
        client, login, binding = self.make()
        client.start_gate = asyncio.Event()
        client.start_error = "PRIVATE-START-FAILURE"
        closed = []
        async def close_native():
            closed.append(True)
        client.close_owned_hosted_login = close_native
        beginning = asyncio.create_task(binding.begin(login))
        await client.start_entered.wait()
        closing = asyncio.create_task(binding.shutdown())
        try:
            await asyncio.sleep(0)
            self.assertFalse(closing.done())
            self.assertEqual(closed, [])
            client.start_gate.set()
            with self.assertRaises(McpAuthError):
                await beginning
            with self.assertRaises(McpAuthError) as caught:
                await closing
            self.assertEqual(caught.exception.code, "start_uncertain")
            self.assertEqual(closed, [True])
        finally:
            client.start_gate.set()
            await asyncio.gather(beginning, closing, return_exceptions=True)

    async def test_failed_native_cleanup_keeps_original_error_and_reports_uncertainty(self):
        client, login, binding = self.make()
        client.start_error = "PRIVATE-START-FAILURE"
        async def close_native():
            raise RuntimeError("PRIVATE-CLEANUP-FAILURE")
        client.close_owned_hosted_login = close_native
        with self.assertRaises(McpAuthError):
            await binding.begin(login)
        with self.assertRaises(McpAuthError) as caught:
            await binding.shutdown()
        self.assertEqual(caught.exception.code, "start_uncertain")
        self.assertTrue(caught.exception.cleanup.callback_closed)
        self.assertEqual(caught.exception.cleanup.native_cancel, "unconfirmed")
        self.assertIsNone(caught.exception.__context__)
        self.assertNotIn("PRIVATE-", repr(caught.exception))

    async def test_malformed_owned_cleanup_never_certifies_retirement(self):
        for value in (None, {"mcp": {}, "cancelled": True},
                      {"mcp": TARGET._wire(), "cancelled": 1},
                      {"mcp": TARGET._wire(), "completed": True}):
            with self.subTest(value=value):
                client, login, binding = self.make()
                client.start_error = "PRIVATE-START-FAILURE"
                async def close_native():
                    return value
                client.close_owned_hosted_login = close_native
                with self.assertRaises(McpAuthError):
                    await binding.begin(login)
                with self.assertRaises(McpAuthError) as caught:
                    await binding.shutdown()
                self.assertEqual(caught.exception.code, "start_uncertain")
                self.assertEqual(caught.exception.cleanup.native_cancel, "unconfirmed")
                self.assertIsNone(caught.exception.native_failure)

    async def test_missing_cleanup_evidence_and_secondary_diagnostic_keep_original_provenance(self):
        client, _, binding = self.make()
        # This is the existing _join local_cleanup_unconfirmed path: no
        # CleanupObservation is attached to a cancelled custodian task.
        task = asyncio.create_task(asyncio.sleep(60))
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)
        binding._custodian._close_task = task
        closed = []
        async def close_native():
            closed.append(True)
            raise McpAuthError("native_auth_failure",
                native_failure=NativeAuthFailure("upstream_failure"))
        client.close_owned_hosted_login = close_native
        with self.assertRaises(McpAuthError) as caught:
            await binding.shutdown()
        self.assertEqual(caught.exception.code, "local_cleanup_unconfirmed")
        self.assertIsNone(caught.exception.native_failure)
        self.assertFalse(caught.exception.cleanup.callback_closed)
        self.assertEqual(caught.exception.cleanup.native_cancel, "unconfirmed")
        self.assertIsNone(caught.exception.__context__)
        self.assertEqual(closed, [True])

    async def test_shutdown_cancellation_waits_for_native_cleanup_then_preserves_replay(self):
        for cleanup_fails in (False, True):
            with self.subTest(cleanup_fails=cleanup_fails):
                client, login, binding = self.make()
                client.start_error = "PRIVATE-START-FAILURE"
                entered, release = asyncio.Event(), asyncio.Event()
                closed = []
                async def close_native():
                    entered.set()
                    await release.wait()
                    closed.append(True)
                    if cleanup_fails:
                        raise RuntimeError("PRIVATE-CLEANUP-FAILURE")
                client.close_owned_hosted_login = close_native
                with self.assertRaises(McpAuthError):
                    await binding.begin(login)
                closing = asyncio.create_task(binding.shutdown())
                try:
                    await asyncio.wait_for(entered.wait(), 1)
                    for _ in range(2):
                        closing.cancel("PRIVATE-CANCEL")
                        await asyncio.sleep(0)
                        self.assertFalse(closing.done())
                    release.set()
                    with self.assertRaises(McpAuthCancelled) as caught:
                        await closing
                    self.assertNotIn("PRIVATE-", str(caught.exception))
                    if cleanup_fails:
                        self.assertEqual(caught.exception.cleanup.native_cancel, "unconfirmed")
                    self.assertEqual(closed, [True])
                    with self.assertRaises(McpAuthError) as caught:
                        await binding.shutdown()
                    self.assertEqual(caught.exception.code, "start_uncertain")
                    self.assertEqual(closed, [True])
                finally:
                    release.set()
                    await asyncio.gather(closing, return_exceptions=True)

    async def test_truthy_or_async_current_is_not_admission(self):
        client, login, binding = self.make()
        login.current = "yes"
        with self.assertRaises(McpHostError):
            await binding.begin(login)
        async def wrong_current():
            return True
        login.is_current = wrong_current
        with self.assertRaises(McpHostError):
            await binding.begin(login)
        self.assertEqual(client.calls, [])


if __name__ == "__main__":
    unittest.main()
