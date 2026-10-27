"""Offline connector transport checks; all OAuth/account material is synthetic."""
import asyncio
import copy
import dataclasses
import gc
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import urlencode, urlsplit

from meerkat_toolkit.connector_auth import (
    ConnectorAuthCancelled, ConnectorAuthError, ConnectorLoginCustodian,
    ConnectorSlot, ConnectorTarget, Discover, Known, StartedConnectorLogin,
    observe_connector_auth,
)
from test_mcp_auth import STATE, CODE, URL, callback, closed, request

SLOT = ConnectorSlot("realm-a", "host-slot-a")
TARGET = ConnectorTarget(SLOT, "https://issuer.invalid", "fixture-client",
                         "urn:fixture:service", ("read",), "fixture-strategy", Discover())
SUBJECT = "provider-subject-a"


def account(target=TARGET, subject=SUBJECT):
    return {"issuer": target.issuer, "strategy_id": target.strategy_id, "subject": subject}


def ready(target=TARGET):
    subject = target.account_selection.account if isinstance(target.account_selection, Known) else SUBJECT
    return {"connector": target.slot._wire(), "verified_account": account(target, subject),
            "scope_evidence": {"kind": "token_endpoint_response"}, "scopes": ["read"],
            "has_refresh_token": True, "extra_private": "FIXTURE-RESPONSE-PRIVATE"}


class Client:
    def __init__(self, target=TARGET):
        self.target, self.redirect = target, None
        self.calls = []
        self.start_gate = self.complete_gate = self.cancel_gate = None
        self.start_error = self.complete_error = self.cancel_error = None
        self.start_override = self.complete_result = self.cancel_result = None
        self.status_result = {**ready(target), "phase": "authorized"}
        self.complete_entered = asyncio.Event()
        self.cancel_entered = asyncio.Event()

    async def auth_connector_login_start(self, target, redirect_uri):
        self.calls.append(("start", copy.deepcopy(target), redirect_uri))
        self.redirect = redirect_uri
        _, writer = await asyncio.open_connection("127.0.0.1", urlsplit(redirect_uri).port)
        writer.close()
        await writer.wait_closed()
        if self.start_gate:
            await self.start_gate.wait()
        if self.start_error:
            raise self.start_error
        value = {"connector": self.target.slot._wire(), "authorize_url": URL,
                 "state": STATE, "redirect_uri": redirect_uri}
        if self.start_override is not None:
            value.update(self.start_override)
        # Mutating a client-owned serialized copy cannot change the retained target.
        target["scopes"].append("untrusted-client-mutation")
        target["slot"]["slot_id"] = "untrusted-client-slot"
        return value

    async def auth_connector_login_complete(self, target, *, code, state, redirect_uri):
        self.calls.append(("complete", copy.deepcopy(target), code, state, redirect_uri))
        self.complete_entered.set()
        if self.complete_gate:
            await self.complete_gate.wait()
        if self.complete_error:
            raise self.complete_error
        return self.complete_result if self.complete_result is not None else ready(self.target)

    async def auth_connector_login_cancel(self, realm_id, slot_id, state):
        self.calls.append(("cancel", realm_id, slot_id, state))
        self.cancel_entered.set()
        if self.cancel_gate:
            await self.cancel_gate.wait()
        if self.cancel_error:
            raise self.cancel_error
        return self.cancel_result if self.cancel_result is not None else {
            "connector": self.target.slot._wire(), "cancelled": True}

    async def auth_connector_status(self, realm_id, slot_id):
        self.calls.append(("status", realm_id, slot_id))
        if isinstance(self.status_result, BaseException):
            raise self.status_result
        return self.status_result


class ConnectorAuthTests(unittest.IsolatedAsyncioTestCase):
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

    def make(self, client=None, target=TARGET, **kwargs):
        client = client or Client(target)
        custodian = ConnectorLoginCustodian(client, target, **kwargs)
        async def cleanup():
            for gate in (client.start_gate, client.complete_gate, client.cancel_gate):
                if gate is not None:
                    gate.set()
            try:
                await custodian.close()
            except (ConnectorAuthError, ConnectorAuthCancelled):
                pass
        self.addAsyncCleanup(cleanup)
        return client, custodian

    async def failure(self, operation, code):
        with self.assertRaises(ConnectorAuthError) as caught:
            await operation
        error = caught.exception
        self.assertEqual(error.code, code)
        self.assertNotIn("FIXTURE", repr(error))
        self.assertIsNone(error.__context__)
        self.assertIsNone(error.__cause__)
        return error

    async def entered(self, client):
        async def wait():
            while client.redirect is None:
                await asyncio.sleep(0)
        await asyncio.wait_for(wait(), 1)

    async def test_discover_exact_target_callback_and_allowlisted_observation(self):
        client, custodian = self.make()
        handle = await custodian.begin()
        self.assertIsInstance(handle, StartedConnectorLogin)
        seen = []
        async def present(url):
            seen.append(url)
        self.assertTrue(await handle.present(present))
        self.assertEqual(seen, [URL])
        self.assertEqual(handle.presentation.authorization_origin, "https://auth.invalid")
        self.assertEqual(handle.presentation.callback_profile, "loopback_local")
        self.assertEqual(handle.presentation.slot, SLOT)
        self.assertEqual(handle.presentation.account_selection, Discover())
        response = await callback(client.redirect, account="forged-account", scope="write")
        observation = await handle.wait()
        self.assertEqual(observation.slot, SLOT)
        self.assertEqual(observation.verified_account.subject, SUBJECT)
        self.assertNotEqual(observation.slot.slot_id, observation.verified_account.subject)
        self.assertEqual(observation.scopes, ("read",))
        self.assertEqual(observation.scope_evidence.kind, "token_endpoint_response")
        self.assertIsNone(observation.expires_at)
        self.assertEqual(client.calls, [
            ("start", TARGET._wire(), client.redirect),
            ("complete", TARGET._wire(), CODE, STATE, client.redirect)])
        for marker in (STATE, CODE, "FIXTURE-URL-SECRET", "FIXTURE-RESPONSE-PRIVATE", "forged-account"):
            self.assertNotIn(marker, repr((handle, handle.presentation, observation, response)))
        with self.assertRaises(dataclasses.FrozenInstanceError):
            observation.phase = "ready"
        self.assertTrue(await closed(client.redirect))
        self.assertEqual((await custodian.close()).native_cancel, "not_needed")

    async def test_known_context_uses_subject_and_keeps_native_scope_evidence(self):
        target = dataclasses.replace(TARGET, account_selection=Known(SUBJECT), scopes=("write",))
        client, custodian = self.make(target=target)
        client.complete_result = {**ready(target), "scopes": [],
            "scope_evidence": {"kind": "retained_on_refresh", "granted_at": "2026-10-05T00:00:00Z"}}
        handle = await custodian.begin()
        await callback(client.redirect)
        value = await handle.wait()
        self.assertEqual(value.verified_account.subject, SUBJECT)
        self.assertEqual(value.scopes, ())
        self.assertEqual(value.scope_evidence.granted_at, "2026-10-05T00:00:00Z")
        self.assertEqual(client.calls[0][1]["account_selection"], {"mode": "known", "account": SUBJECT})

    async def test_complete_wrong_slot_account_context_and_malformed_preserve_uncertainty(self):
        known = dataclasses.replace(TARGET, account_selection=Known(SUBJECT))
        cases = [
            {"connector": {"realm_id": "realm-b", "slot_id": SLOT.slot_id}},
            {"connector": {"realm_id": SLOT.realm_id, "slot_id": "slot-b"}},
            {"verified_account": account(known, "wrong-subject")},
            {"verified_account": {**account(known), "issuer": "https://other.invalid"}},
            {"verified_account": {**account(known), "strategy_id": "other"}},
            {"verified_account": None}, {"scopes": None}, {"scopes": "read"},
            {"scopes": ["read", "read"]}, {"has_refresh_token": 1},
            {"scope_evidence": {"kind": "provider_introspection"}},
            {"scope_evidence": {"kind": "retained_on_refresh", "from": "wrong"}},
            {"scope_evidence": {"kind": "token_endpoint_response", "extra": "FIXTURE"}},
        ]
        for change in cases:
            with self.subTest(change=change):
                client, custodian = self.make(target=known)
                client.complete_result = {**ready(known), **change}
                handle = await custodian.begin()
                await callback(client.redirect)
                failure = await self.failure(handle.wait(), "invalid_completion_response_uncertain")
                self.assertEqual(failure.cleanup.native_cancel, "acknowledged")
                await self.failure(handle.wait(), "invalid_completion_response_uncertain")
                self.assertEqual([c[0] for c in client.calls], ["start", "complete", "cancel"])
                self.assertTrue(await closed(client.redirect))

    async def test_start_no_disposition_and_unknown_fields_never_join(self):
        client, custodian = self.make()
        client.start_override = {"disposition": "joined", "private": "FIXTURE-UNKNOWN"}
        handle = await custodian.begin()
        self.assertIsInstance(handle, StartedConnectorLogin)
        self.assertEqual((await custodian.close()).native_cancel, "acknowledged")

    async def test_malformed_or_cross_slot_start_is_cached_without_guessing_cancel(self):
        changes = [
            {"connector": {"realm_id": "other", "slot_id": SLOT.slot_id}},
            {"connector": {"realm_id": SLOT.realm_id, "slot_id": "other"}},
            {"connector": {**SLOT._wire(), "extra": "FIXTURE"}},
            {"state": ""}, {"state": 12}, {"redirect_uri": "http://127.0.0.1:12/callback"},
            {"authorize_url": "https://user:FIXTURE@auth.invalid"},
            {"authorize_url": "javascript:FIXTURE"},
        ]
        for change in changes:
            with self.subTest(change=change):
                client, custodian = self.make()
                client.start_override = change
                for operation in (custodian.begin, custodian.begin, custodian.close):
                    error = await self.failure(operation(), "invalid_start_response_uncertain")
                    self.assertEqual(error.cleanup.native_cancel, "not_requested")
                self.assertEqual([c[0] for c in client.calls], ["start"])
                self.assertTrue(await closed(client.redirect))

    async def test_lost_cancelled_or_timed_out_start_never_retries(self):
        for cause in (RuntimeError("FIXTURE-NATIVE-PRIVATE"), asyncio.CancelledError("FIXTURE-CANCEL"), None):
            with self.subTest(cause=type(cause).__name__):
                client = Client()
                client.start_error = cause
                if cause is None:
                    client.start_gate = asyncio.Event()
                client, custodian = self.make(client, rpc_timeout=.03)
                first = await self.failure(custodian.begin(), "start_uncertain")
                second = await self.failure(custodian.begin(), "start_uncertain")
                self.assertIsNot(first, second)
                self.assertEqual(first.cleanup.native_cancel, "not_requested")
                await self.failure(custodian.close(), "start_uncertain")
                self.assertEqual([c[0] for c in client.calls], ["start"])
                self.assertTrue(await closed(client.redirect))

    async def test_lost_or_timed_out_complete_does_not_retry_or_claim_no_credential(self):
        for fail in (True, False):
            client = Client()
            if fail:
                client.complete_error = RuntimeError("SlotOccupied: FIXTURE-PROSE-NOT-TYPED")
            else:
                client.complete_gate = asyncio.Event()
            client, custodian = self.make(client, rpc_timeout=.03)
            handle = await custodian.begin()
            await callback(client.redirect)
            error = await self.failure(handle.wait(), "completion_uncertain")
            self.assertEqual(error.cleanup.native_cancel, "acknowledged")
            await self.failure(handle.wait(), "completion_uncertain")
            self.assertEqual([c[0] for c in client.calls], ["start", "complete", "cancel"])

    async def test_false_wrong_slot_malformed_lost_cancel_are_unconfirmed(self):
        cases = [
            {"connector": SLOT._wire(), "cancelled": False},
            {"connector": SLOT._wire(), "cancelled": 1},
            {"connector": {"realm_id": "other", "slot_id": SLOT.slot_id}, "cancelled": True},
            {"connector": SLOT._wire()}, [],
            RuntimeError("FIXTURE-PRIVATE"), asyncio.CancelledError("FIXTURE-PRIVATE"),
        ]
        for value in cases:
            with self.subTest(value=type(value).__name__):
                client, custodian = self.make()
                if isinstance(value, BaseException):
                    client.cancel_error = value
                else:
                    client.cancel_result = value
                await custodian.begin()
                cleanup = await custodian.close()
                self.assertTrue(cleanup.callback_closed)
                self.assertEqual(cleanup.native_cancel, "unconfirmed")
                self.assertEqual(await custodian.close(), cleanup)
                self.assertEqual([c[0] for c in client.calls], ["start", "cancel"])

    async def test_callback_refusal_replay_and_extra_query_never_supply_authority(self):
        client, custodian = self.make()
        client.complete_gate = asyncio.Event()
        handle = await custodian.begin()
        for query, kwargs in [
            ("state=wrong&code=" + CODE, {}),
            (f"state={STATE}&code={CODE}&code=duplicate", {}),
            (f"state={STATE}&code={CODE}&error=denied", {}),
            (f"state={STATE}&code=%ff", {}),
            (f"state={STATE}&code=%QQ", {}),
            (f"state={STATE}&code={CODE}", {"host": "other.invalid"}),
            (f"state={STATE}&code={CODE}", {"path": "/other"}),
        ]:
            self.assertTrue((await request(client.redirect, query, **kwargs)).startswith(b"HTTP/1.1 400"))
        self.assertEqual(len(client.calls), 1)
        await callback(client.redirect, account="forged", scope="write")
        await client.complete_entered.wait()
        self.assertTrue((await callback(client.redirect)).startswith(b"HTTP/1.1 400"))
        client.complete_gate.set()
        self.assertEqual((await handle.wait()).scopes, ("read",))
        self.assertEqual([c[0] for c in client.calls], ["start", "complete"])

    async def test_provider_error_callback_is_only_callback_observation(self):
        client, custodian = self.make()
        handle = await custodian.begin()
        await request(client.redirect, urlencode({"state": STATE, "error": "FIXTURE-PROVIDER-PRIVATE"}))
        error = await self.failure(handle.wait(), "callback_reported_error")
        self.assertEqual(error.cleanup.native_cancel, "acknowledged")
        self.assertEqual([c[0] for c in client.calls], ["start", "cancel"])

    async def test_wait_budget_is_not_native_expiry_and_status_does_not_restart(self):
        client, custodian = self.make(callback_timeout=.02)
        handle = await custodian.begin()
        error = await self.failure(handle.wait(), "callback_wait_abandoned")
        self.assertEqual(error.cleanup.native_cancel, "acknowledged")
        self.assertEqual((await observe_connector_auth(client, TARGET)).phase, "authorized")
        self.assertEqual([c[0] for c in client.calls], ["start", "cancel", "status"])

    async def test_disposed_and_concurrent_waiters_keep_one_started_handle(self):
        client = Client()
        client.start_gate = asyncio.Event()
        client, custodian = self.make(client)
        waiters = [asyncio.create_task(custodian.begin()) for _ in range(5)]
        await self.entered(client)
        waiters[0].cancel("FIXTURE-PANEL-PRIVATE")
        with self.assertRaises(asyncio.CancelledError) as caught:
            await waiters[0]
        self.assertEqual(str(caught.exception), "connector_auth_start_wait_cancelled")
        self.assertIsNone(caught.exception.__context__)
        client.start_gate.set()
        handles = await asyncio.gather(*waiters[1:])
        self.assertTrue(all(h is handles[0] for h in handles))
        handle = handles[0]
        self.assertIs(await custodian.begin(), handle)
        waiter = asyncio.create_task(handle.wait())
        await asyncio.sleep(0)
        waiter.cancel("FIXTURE-PANEL-PRIVATE")
        with self.assertRaises(asyncio.CancelledError) as caught:
            await waiter
        self.assertEqual(str(caught.exception), "connector_auth_completion_wait_cancelled")
        self.assertIsNone(caught.exception.__context__)
        self.assertEqual([c[0] for c in client.calls], ["start"])
        await callback(client.redirect)
        self.assertEqual((await handle.wait()).verified_account.subject, SUBJECT)
        self.assertEqual([c[0] for c in client.calls], ["start", "complete"])

    async def test_presenter_failure_and_cancellation_do_not_cancel_or_restart(self):
        client, custodian = self.make()
        handle = await custodian.begin()
        async def fail(_url):
            raise RuntimeError("FIXTURE-PRESENTER-PRIVATE")
        self.assertFalse(await handle.present(fail))
        async def cancel(_url):
            raise asyncio.CancelledError("FIXTURE-PRESENTER-PRIVATE")
        with self.assertRaises(asyncio.CancelledError) as caught:
            await handle.present(cancel)
        self.assertEqual(str(caught.exception), "connector_auth_presentation_wait_cancelled")
        self.assertIsNone(caught.exception.__context__)
        self.assertEqual([c[0] for c in client.calls], ["start"])
        await callback(client.redirect)
        await handle.wait()

    async def test_explicit_close_during_start_wins_after_joining_and_cancel_once(self):
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
        self.assertEqual((await closing).native_cancel, "acknowledged")
        await self.failure(starting, "custodian_closed")
        self.assertEqual([c[0] for c in client.calls], ["start", "cancel"])

    async def test_repeated_close_waiter_cancel_joins_cleanup_and_redacts(self):
        client = Client()
        client.cancel_gate = asyncio.Event()
        client, custodian = self.make(client)
        await custodian.begin()
        closing = asyncio.create_task(custodian.close())
        await client.cancel_entered.wait()
        closing.cancel("FIXTURE-CLOSE-PRIVATE")
        await asyncio.sleep(0)
        closing.cancel("FIXTURE-CLOSE-PRIVATE")
        await asyncio.sleep(0)
        self.assertFalse(closing.done())
        client.cancel_gate.set()
        with self.assertRaises(ConnectorAuthCancelled) as caught:
            await closing
        self.assertEqual(caught.exception.cleanup.native_cancel, "acknowledged")
        self.assertEqual(str(caught.exception), "connector_auth_cancelled")
        self.assertIsNone(caught.exception.__context__)
        self.assertEqual(await custodian.close(), caught.exception.cleanup)
        self.assertEqual([c[0] for c in client.calls], ["start", "cancel"])

    async def test_close_during_complete_keeps_completion_uncertain(self):
        client = Client()
        client.complete_gate = asyncio.Event()
        client, custodian = self.make(client)
        handle = await custodian.begin()
        await callback(client.redirect)
        await client.complete_entered.wait()
        self.assertEqual((await custodian.close()).native_cancel, "acknowledged")
        await self.failure(handle.wait(), "completion_uncertain")
        self.assertEqual([c[0] for c in client.calls], ["start", "complete", "cancel"])

    async def test_close_without_begin_never_starts(self):
        client, custodian = self.make()
        self.assertEqual((await custodian.close()).native_cancel, "not_requested")
        await self.failure(custodian.begin(), "custodian_closed")
        self.assertEqual(client.calls, [])

    async def test_timed_out_cancel_is_retained_without_second_cancel(self):
        client = Client()
        client.cancel_gate = asyncio.Event()
        client, custodian = self.make(client, rpc_timeout=.03)
        await custodian.begin()
        cleanup = await custodian.close()
        self.assertEqual(cleanup.native_cancel, "unconfirmed")
        self.assertTrue(cleanup.callback_closed)
        self.assertEqual(await custodian.close(), cleanup)
        self.assertEqual([c[0] for c in client.calls], ["start", "cancel"])

    async def test_abandoned_failed_start_retains_outcome_without_unhandled_error(self):
        client = Client()
        client.start_gate = asyncio.Event()
        client.start_error = RuntimeError("FIXTURE-LOST-START")
        client, custodian = self.make(client)
        waiter = asyncio.create_task(custodian.begin())
        await self.entered(client)
        waiter.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await waiter
        client.start_gate.set()
        await self.failure(custodian.close(), "start_uncertain")
        self.assertTrue(await closed(client.redirect))
        self.assertEqual([c[0] for c in client.calls], ["start"])

    async def test_bind_failure_and_unsupported_client_have_no_native_effects(self):
        client, custodian = self.make()
        with patch("meerkat_toolkit.connector_auth._Listener.bind", side_effect=OSError("FIXTURE")):
            error = await self.failure(custodian.begin(), "callback_bind_failed")
        self.assertEqual(error.cleanup.native_cancel, "not_requested")
        self.assertEqual(client.calls, [])
        client, custodian = self.make()
        client.auth_connector_login_complete = None
        error = await self.failure(custodian.begin(), "unsupported_client")
        self.assertEqual(error.cleanup.native_cancel, "not_requested")
        self.assertEqual(client.calls, [])

    async def test_missing_completion_fields_and_null_status_scopes_refuse(self):
        for key in ("connector", "verified_account", "scope_evidence", "scopes", "has_refresh_token"):
            with self.subTest(key=key):
                client, custodian = self.make()
                client.complete_result = ready()
                del client.complete_result[key]
                handle = await custodian.begin()
                await callback(client.redirect)
                await self.failure(handle.wait(), "invalid_completion_response_uncertain")
                self.assertEqual([c[0] for c in client.calls], ["start", "complete", "cancel"])

    async def test_invalid_immutable_target_and_bounds_refuse_before_effects(self):
        client = Client()
        for changes in ({"scopes": ["read"]}, {"scopes": ("read", "read")},
                        {"account_selection": {"mode": "discover"}}, {"issuer": "invalid"},
                        {"slot": ConnectorSlot("", "slot")}, {"account_selection": Known("")}):
            with self.subTest(changes=changes), self.assertRaises(ConnectorAuthError) as caught:
                ConnectorLoginCustodian(client, dataclasses.replace(TARGET, **changes))
            self.assertEqual(caught.exception.code, "invalid_target")
        for value in (True, 0, -1, float("nan"), float("inf"), 301):
            with self.assertRaises(ConnectorAuthError) as caught:
                ConnectorLoginCustodian(client, TARGET, rpc_timeout=value)
            self.assertEqual(caught.exception.code, "invalid_wait_bound")
        with self.assertRaises(dataclasses.FrozenInstanceError):
            TARGET.slot.slot_id = "other"
        self.assertEqual(client.calls, [])

    async def test_status_keeps_missing_empty_and_verified_facts_separate(self):
        client = Client()
        for phase in ("authorized", "reauth_required", "authorization_required"):
            client.status_result = {"connector": SLOT._wire(), "phase": phase, "has_refresh_token": False}
            value = await observe_connector_auth(client, TARGET)
            self.assertEqual(value.phase, phase)
            self.assertIsNone(value.verified_account)
            self.assertIsNone(value.scope_evidence)
            self.assertIsNone(value.scopes)
            client.status_result["scopes"] = []
            self.assertEqual((await observe_connector_auth(client, TARGET)).scopes, ())
        client.status_result = {**ready(), "phase": "reauth_required", "expires_at": "2026-10-04T00:00:00Z"}
        value = await observe_connector_auth(client, TARGET)
        self.assertEqual(value.verified_account.subject, SUBJECT)
        self.assertEqual(value.phase, "reauth_required")
        self.assertTrue(all(c[0] == "status" for c in client.calls))

    async def test_status_wrong_slot_context_unknown_evidence_or_lost_is_not_authorized(self):
        known = dataclasses.replace(TARGET, account_selection=Known(SUBJECT))
        client = Client(known)
        for change in ({"connector": {"realm_id": "other", "slot_id": SLOT.slot_id}},
                       {"verified_account": account(known, "other")}, {"phase": "ready"},
                       {"has_refresh_token": "true"}, {"scopes": None},
                       {"scope_evidence": {"kind": "provider_introspection"}}):
            client.status_result = {**ready(known), "phase": "authorized", **change}
            await self.failure(observe_connector_auth(client, known), "invalid_status_response")
        client.status_result = RuntimeError("FIXTURE-NATIVE-PRIVATE")
        await self.failure(observe_connector_auth(client, known), "status_unconfirmed")
        client.status_result = asyncio.CancelledError("FIXTURE-NATIVE-PRIVATE")
        with self.assertRaises(asyncio.CancelledError) as caught:
            await observe_connector_auth(client, known)
        self.assertEqual(str(caught.exception), "connector_auth_status_wait_cancelled")
        self.assertIsNone(caught.exception.__context__)

    async def test_two_host_slots_are_distinct_despite_same_verified_subject(self):
        other = dataclasses.replace(TARGET, slot=ConnectorSlot(SLOT.realm_id, "host-slot-b"))
        pairs = [self.make(), self.make(target=other)]
        observations = []
        for client, custodian in pairs:
            handle = await custodian.begin()
            await callback(client.redirect)
            observations.append(await handle.wait())
        self.assertNotEqual(observations[0].slot, observations[1].slot)
        self.assertEqual(observations[0].verified_account, observations[1].verified_account)


# Run only as the real public SDK's explicitly selected JSON-RPC child. This is
# an inert framing fixture, not native Meerkat, a model or an OAuth provider.
RPC_FIXTURE = '''import json,sys
from pathlib import Path
from meerkat.generated.types import CONTRACT_VERSION
log=Path(__file__).with_suffix('.calls')
started={}
for line in sys.stdin:
 r=json.loads(line); m=r['method']; p=r['params']
 with log.open('a') as out: out.write(json.dumps(r)+'\\n')
 if m=='initialize': result={'contract_version':CONTRACT_VERSION,'methods':[]}
 elif m=='capabilities/get': result={'capabilities':[]}
 elif m=='auth/login/start':
  t=p['connector']; started[t['slot']['slot_id']] = (t,p['redirect_uri'])
  result={'connector':t['slot'],'authorize_url':'https://auth.invalid/?state=FIXTURE-STATE-SECRET','state':'FIXTURE-STATE-SECRET','redirect_uri':p['redirect_uri']}
 elif m=='auth/login/complete':
  t=p['connector']; prior,redirect=started[t['slot']['slot_id']]
  assert t==prior and p['redirect_uri']==redirect and p['state']=='FIXTURE-STATE-SECRET' and p['code']=='FIXTURE-CODE-SECRET'
  subject=t['account_selection'].get('account','provider-subject-a')
  result={'connector':t['slot'],'verified_account':{'issuer':t['issuer'],'strategy_id':t['strategy_id'],'subject':subject},'scope_evidence':{'kind':'token_endpoint_response'},'scopes':[],'has_refresh_token':False,'extra_private':'FIXTURE-RESPONSE-PRIVATE'}
 elif m=='auth/login/cancel':
  assert p['state']=='FIXTURE-STATE-SECRET'
  result={'connector':p['connector'],'cancelled':True}
 elif m=='auth/status/get':
  result={'connector':p['connector'],'phase':'authorization_required','has_refresh_token':False}
 else: raise AssertionError(m)
 print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':result}),flush=True)
'''


@unittest.skipUnless(os.environ.get("TOOLKIT_TEST_PUBLIC_SDK"), "exact public SDK source path required")
class PublicConnectorSdkTests(unittest.IsolatedAsyncioTestCase):
    async def test_public_sdk_exact_requests_completion_status_and_cancel(self):
        sdk = Path(os.environ["TOOLKIT_TEST_PUBLIC_SDK"]).resolve()
        sys.path.insert(0, str(sdk))
        self.addCleanup(sys.path.remove, str(sdk))
        from meerkat import MeerkatClient
        self.assertTrue(Path(sys.modules["meerkat"].__file__).resolve().is_relative_to(sdk))
        with tempfile.TemporaryDirectory(prefix="connector-auth-rpc-") as temp:
            script = Path(temp) / "scripted-rpc"
            script.write_text(f"#!{sys.executable}\n" + RPC_FIXTURE)
            script.chmod(0o700)
            # The scripted SDK child receives no inherited provider/model secrets.
            with patch.dict(os.environ, {"PYTHONPATH": str(sdk), "PATH": os.defpath}, clear=True):
                client = MeerkatClient(str(script))
                await client.connect()
                self.addAsyncCleanup(client.close)
                for selection in (Discover(), Known(SUBJECT)):
                    target = dataclasses.replace(TARGET, account_selection=selection)
                    custodian = ConnectorLoginCustodian(client, target)
                    self.addAsyncCleanup(custodian.close)
                    handle = await custodian.begin()
                    calls = [json.loads(line) for line in script.with_suffix(".calls").read_text().splitlines()]
                    redirect = calls[-1]["params"]["redirect_uri"]
                    await callback(redirect)
                    value = await handle.wait()
                    self.assertEqual(value.verified_account.subject, SUBJECT)
                    self.assertEqual(value.scopes, ())
                    self.assertNotIn("FIXTURE", repr(value))
                    await custodian.close()
                self.assertEqual((await observe_connector_auth(client, TARGET)).phase, "authorization_required")
                cancelled = ConnectorLoginCustodian(client, TARGET)
                self.addAsyncCleanup(cancelled.close)
                await cancelled.begin()
                self.assertEqual((await cancelled.close()).native_cancel, "acknowledged")
                await client.close()
            calls = [json.loads(line) for line in script.with_suffix(".calls").read_text().splitlines()]
            self.assertEqual([c["method"] for c in calls], [
                "initialize", "capabilities/get", "auth/login/start", "auth/login/complete",
                "auth/login/start", "auth/login/complete", "auth/status/get", "auth/login/start",
                "auth/login/cancel"])
            for start, complete in ((calls[2], calls[3]), (calls[4], calls[5])):
                self.assertEqual(complete["params"], {
                    **start["params"], "code": CODE, "state": STATE})
                self.assertNotIn("mcp", start["params"])
            self.assertEqual(calls[6]["params"], {"connector": SLOT._wire()})
            self.assertEqual(calls[8]["params"], {"connector": SLOT._wire(), "state": STATE})


if __name__ == "__main__":
    unittest.main()
