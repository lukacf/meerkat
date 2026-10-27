"""Existing host/UI composition with synthetic admission, never protection proof."""
import asyncio
import copy
import json
import unittest
from urllib.parse import urlencode, urlsplit

from meerkat_toolkit.connector_auth import (
    ConnectorAuthError, ConnectorSlot, ConnectorTarget, Discover, Known,
)
from meerkat_toolkit.connector_auth_host import (
    ConnectorHostError, ConnectorSignInBinding, HostStepReference,
    SignInControlRequest, credential_view_fields,
)


TARGET = ConnectorTarget(ConnectorSlot("fixture-realm", "fixture-slot"),
    "https://issuer.example.test", "fixture-client", "fixture-resource",
    ("read",), "fixture-strategy", Discover())
REFERENCE = HostStepReference("fixture-host", "existing-step", "revision-1")


def request(kind="open_authorization"):
    return {"scope": "person-a", "viewRevision": 7, "control": {
        "kind": kind, "offer": "existing-offer", "target": REFERENCE.to_view()}}


class Native:
    """Inert replies; start binds the adapter's real local callback listener."""
    def __init__(self):
        self.calls = []
        self.entered = asyncio.Event()
        self.release = asyncio.Event()
        self.release.set()
        self.status_release = asyncio.Event()
        self.status_release.set()

    async def auth_connector_login_start(self, target, redirect_uri):
        self.redirect_uri = redirect_uri
        self.calls.append(("start", copy.deepcopy(target)))
        self.entered.set()
        await self.release.wait()
        return {"connector": target["slot"], "redirect_uri": redirect_uri,
                "authorize_url": "https://issuer.example.test/auth?private=FIXTURE",
                "state": "FIXTURE-PRIVATE-STATE"}

    async def auth_connector_login_cancel(self, realm_id, slot_id, state):
        self.calls.append(("cancel", (realm_id, slot_id)))
        return {"connector": {"realm_id": realm_id, "slot_id": slot_id}, "cancelled": True}

    async def auth_connector_login_complete(self, target, *, code, state, redirect_uri):
        self.calls.append(("complete", copy.deepcopy(target)))
        return {"connector": target["slot"], "has_refresh_token": False,
                "verified_account": {"issuer": TARGET.issuer,
                    "strategy_id": TARGET.strategy_id, "subject": "discovered-user"},
                "scope_evidence": {"kind": "token_endpoint_response"}, "scopes": ["read"]}

    async def callback(self):
        url = urlsplit(self.redirect_uri)
        reader, writer = await asyncio.open_connection(url.hostname, url.port)
        query = urlencode({"code": "FIXTURE-PRIVATE-CODE", "state": "FIXTURE-PRIVATE-STATE"})
        writer.write(f"GET {url.path}?{query} HTTP/1.1\r\nHost: {url.netloc}\r\n\r\n".encode())
        await writer.drain()
        await reader.read()
        writer.close()
        await writer.wait_closed()

    async def auth_connector_status(self, realm_id, slot_id):
        self.calls.append(("status", (realm_id, slot_id)))
        self.entered.set()
        await self.status_release.wait()
        return {"connector": {"realm_id": realm_id, "slot_id": slot_id},
                "phase": "authorized", "has_refresh_token": False,
                "verified_account": {"issuer": TARGET.issuer,
                    "strategy_id": TARGET.strategy_id, "subject": "discovered-user"},
                "scope_evidence": {"kind": "token_endpoint_response"}, "scopes": ["read"]}


class Host:
    """Explicit test admission policy, not a production host or protected surface."""
    def __init__(self):
        self.calls = []
        self.denied = set()
        self.presented = []
        self.entered = asyncio.Event()
        self.release = asyncio.Event()
        self.release.set()

    async def revalidate(self, context, operation, control, target):
        self.calls.append((operation, control, target))
        self.entered.set()
        await self.release.wait()
        if context != "authenticated-person-a" or operation in self.denied or target != TARGET:
            raise RuntimeError("FIXTURE-PRIVATE-ADMISSION")
        if control is not None and (control.scope != "person-a" or control.view_revision != 7
                or control.offer != "existing-offer" or control.target != REFERENCE):
            raise RuntimeError("FIXTURE-PRIVATE-CONTROL")

    async def presenter(self, url):
        self.presented.append(url)


class HostCompositionTests(unittest.IsolatedAsyncioTestCase):
    def make(self, *, enabled=True, target=TARGET):
        native, host = Native(), Host()
        binding = ConnectorSignInBinding(native, target, reference=REFERENCE,
            revalidate=host.revalidate, protected_presenter=host.presenter if enabled else None,
            callback_timeout=5, rpc_timeout=1)

        async def cleanup():
            native.release.set()
            native.status_release.set()
            host.release.set()
            try:
                await binding.shutdown()
            except ConnectorAuthError:
                pass

        self.addAsyncCleanup(cleanup)
        return native, host, binding

    async def error(self, operation, code):
        with self.assertRaises(ConnectorHostError) as caught:
            await operation
        self.assertEqual(str(caught.exception), code)
        self.assertIsNone(caught.exception.__context__)
        self.assertIsNone(caught.exception.__cause__)

    async def project(self, binding, control=None, **kwargs):
        return await binding.project_action("authenticated-person-a", action_id="sign-in",
            integration="Fixture service", state="action_required", instructions="Review sign-in.",
            control_request=control, **kwargs)

    async def test_disabled_by_default_and_no_implicit_start(self):
        native, host, binding = self.make(enabled=False)
        view = await self.project(binding, request())
        self.assertEqual(view["controls"], [])
        self.assertIn("not supplied a protected", view["instructions"])
        self.assertNotIn("authorizationOrigin", view)
        await self.error(binding.dispatch("authenticated-person-a", request()),
                         "protected_presenter_unavailable")
        await self.error(binding.wait("authenticated-person-a"), "login_not_started")
        self.assertEqual(native.calls, [])
        self.assertEqual(host.presented, [])

    async def test_host_admission_rejects_wrong_person_scope_offer_and_revision(self):
        native, _, binding = self.make()
        await self.error(binding.dispatch("authenticated-person-b", request()),
                         "host_admission_unconfirmed")
        cases = [request() for _ in range(3)]
        cases[0]["scope"] = "person-b"
        cases[1]["viewRevision"] = 8
        cases[2]["control"]["offer"] = "stale-offer"
        for control in cases:
            await self.error(binding.dispatch("authenticated-person-a", control),
                             "host_admission_unconfirmed")
        self.assertEqual(native.calls, [])

    async def test_reference_malformed_and_unrelated_controls_rejected_before_effect(self):
        native, host, binding = self.make()
        different = request()
        different["control"]["target"]["reference"] = "other-step"
        await self.error(binding.dispatch("authenticated-person-a", different),
                         "host_reference_mismatch")
        for kind in ("disconnect", "cancel_task", "revoke_grant", "change_account"):
            await self.error(binding.dispatch("authenticated-person-a", request(kind)),
                             "unsupported_host_control")
        malformed = request()
        malformed["authorize_url"] = "FIXTURE-PRIVATE"
        await self.error(binding.dispatch("authenticated-person-a", malformed), "invalid_host_control")
        malformed = request()
        malformed["viewRevision"] = True
        await self.error(binding.dispatch("authenticated-person-a", malformed), "invalid_host_control")
        self.assertEqual(native.calls, [])
        self.assertEqual(host.calls, [])

    async def test_control_is_frozen_before_admission_await(self):
        native, host, binding = self.make()
        host.release.clear()
        control = request()
        task = asyncio.create_task(binding.dispatch("authenticated-person-a", control))
        await asyncio.wait_for(host.entered.wait(), 1)
        control["scope"] = "person-b"
        control["control"]["target"]["reference"] = "other-step"
        control["control"]["offer"] = "other-offer"
        host.release.set()
        self.assertEqual(await task, {"kind": "presentation_returned"})
        self.assertEqual([c[0] for c in host.calls], ["open_authorization", "present"])
        self.assertTrue(all(c[1] == SignInControlRequest.from_view(request()) for c in host.calls))
        self.assertEqual(native.calls, [("start", TARGET._wire())])

    async def test_revocation_between_start_and_present_keeps_attempt_but_withholds_url(self):
        native, host, binding = self.make()
        native.release.clear()
        task = asyncio.create_task(binding.dispatch("authenticated-person-a", request()))
        await asyncio.wait_for(native.entered.wait(), 1)
        host.denied.add("present")
        native.release.set()
        await self.error(task, "host_admission_unconfirmed")
        self.assertEqual(host.presented, [])
        self.assertEqual([c[0] for c in native.calls], ["start"])
        host.denied.clear()
        await binding.dispatch("authenticated-person-a", request())
        self.assertEqual([c[0] for c in native.calls], ["start"])
        self.assertEqual(len(host.presented), 1)

    async def test_cancelled_ui_request_and_explicit_replay_do_not_duplicate_start(self):
        native, host, binding = self.make()
        native.release.clear()
        task = asyncio.create_task(binding.dispatch("authenticated-person-a", request()))
        await asyncio.wait_for(native.entered.wait(), 1)
        task.cancel("FIXTURE-PRIVATE-REQUEST")
        with self.assertRaises(asyncio.CancelledError) as caught:
            await task
        self.assertNotIn("FIXTURE", str(caught.exception))
        self.assertIsNone(caught.exception.__context__)
        native.release.set()
        await binding.dispatch("authenticated-person-a", request())
        self.assertEqual([c[0] for c in native.calls], ["start"])
        self.assertEqual(len(host.presented), 1)

    async def test_status_read_revalidates_both_sides_and_cannot_project_ready(self):
        native, host, binding = self.make()
        native.status_release.clear()
        task = asyncio.create_task(binding.observe("authenticated-person-a"))
        await asyncio.wait_for(native.entered.wait(), 1)
        host.denied.add("read_result")
        native.status_release.set()
        await self.error(task, "host_admission_unconfirmed")
        host.denied.clear()
        observation = await binding.observe("authenticated-person-a")
        self.assertEqual([c[0] for c in native.calls], ["status", "status"])
        self.assertEqual([c[0] for c in host.calls], ["status", "read_result"] * 2)
        fields = credential_view_fields(observation)
        self.assertIn("discovered-user", fields["account"])
        self.assertNotIn(TARGET.slot.slot_id, fields["account"])
        self.assertEqual(fields["credentialScope"], "read")
        self.assertEqual(fields["status"]["tone"], "attention")
        self.assertNotIn("grants", fields)

    async def test_read_denial_precedes_native_status(self):
        native, host, binding = self.make()
        host.denied.add("status")
        await self.error(binding.observe("authenticated-person-a"), "host_admission_unconfirmed")
        self.assertEqual(native.calls, [])

    async def test_completion_result_read_can_be_denied_after_callback(self):
        native, host, binding = self.make()
        await binding.dispatch("authenticated-person-a", request())
        host.denied.add("read_result")
        await native.callback()
        await self.error(binding.wait("authenticated-person-a"), "host_admission_unconfirmed")
        host.denied.clear()
        result = await binding.wait("authenticated-person-a")
        self.assertEqual(result.verified_account.subject, "discovered-user")
        self.assertEqual([c[0] for c in native.calls], ["start", "complete"])

    async def test_admission_cancellation_redacted_without_cause_or_effect(self):
        native, host, binding = self.make()
        host.release.clear()
        task = asyncio.create_task(binding.dispatch("authenticated-person-a", request()))
        await asyncio.wait_for(host.entered.wait(), 1)
        task.cancel("FIXTURE-PRIVATE-ADMISSION-CANCEL")
        with self.assertRaises(asyncio.CancelledError) as caught:
            await task
        self.assertEqual(str(caught.exception), "connector_host_admission_wait_cancelled")
        self.assertIsNone(caught.exception.__context__)
        self.assertIsNone(caught.exception.__cause__)
        self.assertEqual(native.calls, [])

    async def test_stop_is_explicit_admitted_cleanup_not_task_or_credential_revoke(self):
        native, host, binding = self.make()
        await binding.dispatch("authenticated-person-a", request())
        host.denied.add("stop_setup")
        await self.error(binding.dispatch("authenticated-person-a", request("stop_setup")),
                         "host_admission_unconfirmed")
        self.assertEqual([c[0] for c in native.calls], ["start"])
        host.denied.clear()
        reply = await binding.dispatch("authenticated-person-a", request("stop_setup"))
        self.assertEqual(reply, {"kind": "cleanup_observed", "callbackClosed": True,
                                 "nativeCancel": "acknowledged"})
        self.assertEqual([c[0] for c in native.calls], ["start", "cancel"])
        await binding.dispatch("authenticated-person-a", request("stop_setup"))
        self.assertEqual([c[0] for c in native.calls], ["start", "cancel"])

    async def test_projection_contains_labels_only_and_host_supplied_state(self):
        native, host, binding = self.make()
        first = await self.project(binding, request())
        self.assertEqual(first["requestedAccountLabel"], "Account chosen during sign-in")
        await binding.dispatch("authenticated-person-a", request())
        second = await self.project(binding, request())
        self.assertEqual(second["state"], "action_required")
        self.assertEqual(second["authorizationOrigin"], "https://issuer.example.test")
        self.assertEqual(second["controls"], [request()["control"]])
        self.assertNotIn("FIXTURE", json.dumps(second))
        self.assertNotIn("expiresAt", second)
        self.assertEqual([c[0] for c in native.calls], ["start"])
        self.assertEqual(len(host.presented), 1)

    async def test_discover_cannot_be_reinterpreted_as_reconnect(self):
        native, _, binding = self.make()
        await self.error(binding.dispatch("authenticated-person-a", request("reconnect")),
                         "reconnect_requires_known_target")
        self.assertEqual(native.calls, [])

    async def test_known_label_is_requested_identity_not_verification(self):
        target = ConnectorTarget(TARGET.slot, TARGET.issuer, TARGET.client, TARGET.resource,
            TARGET.scopes, TARGET.strategy_id, Known("known-subject"))
        native, host, binding = self.make(target=target)

        async def known_admission(context, operation, control, actual):
            self.assertEqual(actual, target)
            self.assertEqual(context, "authenticated-person-a")

        binding._revalidate = known_admission
        view = await self.project(binding, request("reconnect"))
        self.assertEqual(view["requestedAccountLabel"], "known-subject")
        self.assertEqual(view["state"], "action_required")
        self.assertEqual(native.calls, [])


if __name__ == "__main__":
    unittest.main()
