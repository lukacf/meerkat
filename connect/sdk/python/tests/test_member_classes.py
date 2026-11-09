"""Real pipe framing and negative controls, without claiming native execution."""
import asyncio
import dataclasses
import json
import os
from pathlib import Path
import runpy
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, Mock, patch
from concurrent.futures import ThreadPoolExecutor

from meerkat_meta.identity_first_models import DurableAgentSpec
from meerkat_toolkit.host_control import PrivateHostClient, HostControlError, HostReplyCode, HostReplyError
from meerkat_toolkit.member_classes import MemberClassClient, MemberClassAddRequest, MemberClassObservationUnknown, MemberClassListingUnknown


MEMBER = {"mob_id": "application", "member": "project_one", "role": "project"}
PRINCIPAL = {"kind": "actor", "id": "application-owner"}


def selection():
    return MemberClassAddRequest("add-project-one", {"class": "project", "member": {
        "member": MEMBER, "logical_owner": PRINCIPAL, "context_generation": 1,
        "audience": {"kind": "principal", "principal": PRINCIPAL}}},
        DurableAgentSpec(identity="project-one", profile="project", runtime_mode_override="turn_driven"),
        {"id": "project-console", "origin": {"kind": "local_console", "member": MEMBER,
         "requester": PRINCIPAL, "ingress_actor": PRINCIPAL}, "logical_executor": PRINCIPAL,
         "represented_subject": None, "source_authority": PRINCIPAL,
         "authentication_domain": "console", "event_domain": "console"},
        {"target": {"kind": "original_route", "route": "project-console"}, "issuance_id": "issue-project"})


def native(stage="minted"):
    return {"outcomes": {"project-one": {"restore": {"stage": stage}}}, "managed_edges": []}


def settled(stage="minted", *, authorization=None, cleanup=None):
    return {"state": "settled", "native": {"state": "result", "result": native(stage)},
            "authorization": authorization or {"state": "succeeded"},
            "cleanup": cleanup or {"state": "succeeded"}}


def observed(*, admission="add-project-one", revision=17, authorization=None):
    return {"state": "observed", "record": {"id": admission, "instance": selection().instance,
            "phase": {"kind": "retired"}}, "checkpoint": {"admission": admission, "revision": revision},
            "authorization": authorization or {"state": "refused", "error": "denied"}}


class MemberClassPipeTests(unittest.TestCase):
    def setUp(self):
        child_read, parent_write = os.pipe()
        parent_read, child_write = os.pipe()
        self.host = PrivateHostClient(parent_write, parent_read, timeout=2)
        self.client = MemberClassClient(self.host)
        self.reader = os.fdopen(child_read, "rb")
        self.writer = os.fdopen(child_write, "wb", buffering=0)
        self.pool = ThreadPoolExecutor(max_workers=2)
        self.loop = asyncio.new_event_loop()
        self.loop_thread = threading.Thread(target=self.loop.run_forever)
        self.loop_thread.start()

    def tearDown(self):
        self.host.close()
        self.pool.shutdown(wait=True)
        self.reader.close()
        self.writer.close()
        self.loop.call_soon_threadsafe(self.loop.stop)
        self.loop_thread.join()
        self.loop.close()

    def begin_add(self, request, **kwargs):
        async def begin():
            return self.client.begin_add(request, publish=AsyncMock(), **kwargs)
        return asyncio.run_coroutine_threadsafe(begin(), self.loop).result(2)

    def add(self, request, **kwargs):
        return self.begin_add(request, **kwargs).result()

    def receive(self):
        return json.loads(self.reader.readline())

    def respond(self, request, observation):
        self.writer.write(json.dumps({"version": 1, "id": request["id"], "result": {
            "kind": "member_class", "observation": observation}}).encode() + b"\n")

    def test_add_uses_existing_pipe_and_preserves_native_result_independently_of_authorization(self):
        request = selection()
        pending = self.pool.submit(self.add, request)
        wire = self.receive()
        self.assertEqual(wire["command"], {"kind": "member_class", "operation": request.wire()})
        self.assertNotIn("factory", wire["command"]["operation"])
        self.respond(wire, settled(authorization={"state": "refused", "error": "denied"}))
        result = pending.result(2)
        self.assertEqual(result["native"]["result"]["outcomes"]["project-one"]["restore"]["stage"], "minted")
        self.assertEqual(result["authorization"], {"state": "refused", "error": "denied"})

    def test_unsettled_is_returned_before_explicit_native_observer_join(self):
        pending = self.pool.submit(self.add, selection())
        wire = self.receive()
        self.respond(wire, {"state": "unsettled", "settlement_id": wire["id"]})
        result = pending.result(2)
        self.assertEqual(result["state"], "unsettled")
        joined = self.pool.submit(self.client.join, result["settlement_id"])
        join = self.receive()
        self.assertEqual(join["command"]["operation"], {"kind": "join", "settlement_id": wire["id"]})
        self.respond(join, {"state": "native_refused", "message": "held completion refused", "data": {"kind": "held_bootstrap_refused"}})
        self.assertEqual(joined.result(2)["state"], "native_refused")

    def test_timeout_never_retries_or_removes_desired_member(self):
        pending = self.pool.submit(self.add, selection(), timeout=0.03)
        wire = self.receive()
        with self.assertRaises(TimeoutError):
            pending.result(2)
        self.respond(wire, settled())
        observation = self.pool.submit(self.client.observe, "add-project-one", MEMBER)
        next_request = self.receive()
        self.assertEqual(next_request["id"], wire["id"] + 1)
        self.assertEqual(next_request["command"]["operation"]["kind"], "observe")
        self.respond(next_request, observed())
        self.assertEqual(observation.result(2), observed())

    def test_retired_observation_keeps_record_checkpoint_and_denied_authorization(self):
        pending = self.pool.submit(self.client.observe, "add-project-one", MEMBER)
        wire = self.receive()
        self.respond(wire, observed())
        result = pending.result(2)
        self.assertEqual(result["record"]["phase"], {"kind": "retired"})
        self.assertEqual(result["checkpoint"], {"admission": "add-project-one", "revision": 17})
        self.assertEqual(result["authorization"], {"state": "refused", "error": "denied"})

    def test_checkpoint_for_another_admission_is_not_combined_with_current_record(self):
        pending = self.pool.submit(self.client.observe, "add-project-one", MEMBER)
        wire = self.receive()
        response = observed()
        response["checkpoint"]["admission"] = "another-admission"
        self.respond(wire, response)
        with self.assertRaises(HostControlError):
            pending.result(2)
        self.assertEqual(self.reader.read(), b"")

    def test_missing_record_refusal_is_not_synthesized_as_authorization_absence(self):
        pending = self.pool.submit(self.client.observe, "missing", MEMBER)
        wire = self.receive()
        self.writer.write(json.dumps({"version": 1, "id": wire["id"],
            "result": {"kind": "error", "error": "denied"}}).encode() + b"\n")
        with self.assertRaises(HostReplyError) as refused:
            pending.result(2)
        self.assertEqual(refused.exception.code, HostReplyCode.DENIED)

    def test_explicit_predecessor_is_forwarded_without_lookup_or_retry(self):
        prior = {"admission": "prior-project-one", "revision": 17}
        request = dataclasses.replace(selection(), expected_predecessor=prior)
        pending = self.pool.submit(self.add, request)
        wire = self.receive()
        self.assertEqual(wire["id"], 1)
        self.assertEqual(wire["command"]["operation"]["expected_predecessor"], prior)
        self.respond(wire, settled())
        self.assertEqual(pending.result(2), settled())
        self.assertNotIn("expected_predecessor", selection().wire())

    def test_malformed_predecessor_refuses_before_any_bytes(self):
        for prior in ({"admission": "old", "revision": True},
                      {"admission": "old", "revision": -1},
                      {"admission": "old", "revision": 2**64},
                      {"admission": "old"},
                      {"admission": "old", "revision": 3, "auto_retry": True}):
            with self.assertRaises(HostControlError):
                self.begin_add(dataclasses.replace(selection(), expected_predecessor=prior))
        self.host.close()
        self.assertEqual(self.reader.read(), b"")

    def test_callback_implementation_grants_parents_and_initial_input_refused_locally(self):
        original = selection()
        for request in [dataclasses.replace(original, instance={**original.instance, "factory": "evil"}),
                        dataclasses.replace(original, route={**original.route, "controller_parent": ["fake"]}),
                        dataclasses.replace(original, instance={"class": "project", "member": {**original.instance["member"], "setup_capability": {}}}),
                        dataclasses.replace(original, spec=DurableAgentSpec(identity="project-one", profile="project", initial_message="run now"))]:
            with self.assertRaises(HostControlError):
                self.add(request)
        self.host.close()
        self.assertEqual(self.reader.read(), b"")

    def test_foreign_settlement_reference_poisoned_without_join(self):
        pending = self.pool.submit(self.add, selection())
        wire = self.receive()
        self.respond(wire, {"state": "unsettled", "settlement_id": wire["id"] + 1})
        with self.assertRaises(HostControlError):
            pending.result(2)
        self.assertEqual(self.reader.read(), b"")

    def test_attempt_keeps_exact_operation_id_after_timeout_without_replay(self):
        attempt = self.begin_add(selection(), timeout=0.03)
        wire = self.receive()
        self.assertEqual(attempt.operation_id, wire["id"])
        with self.assertRaises(MemberClassObservationUnknown) as first:
            attempt.result()
        with self.assertRaises(MemberClassObservationUnknown) as second:
            attempt.result()
        self.assertIs(first.exception, second.exception)
        self.assertIs(first.exception.attempt, attempt)
        self.respond(wire, {"state": "unsettled", "settlement_id": wire["id"]})
        joined = self.pool.submit(self.client.join, first.exception.operation_id)
        join = self.receive()
        self.assertEqual(join["id"], wire["id"] + 1)
        self.assertEqual(join["command"]["operation"], {"kind": "join", "settlement_id": wire["id"]})
        self.respond(join, settled("refused"))
        self.assertEqual(joined.result(2)["state"], "settled")

    def test_authorization_reply_cannot_substitute_another_member(self):
        pending = self.pool.submit(self.client.observe, "add-project-one", MEMBER)
        wire = self.receive()
        self.respond(wire, observed(admission="other"))
        with self.assertRaises(HostControlError):
            pending.result(2)
        self.assertEqual(self.reader.read(), b"")

    def test_partial_write_failure_retains_published_operation_without_retry(self):
        write = os.write
        calls = []

        def interrupted(fd, value):
            if fd != self.host._command_fd:
                return write(fd, value)
            calls.append(value)
            if len(calls) == 1:
                return write(fd, value[:8])
            raise BrokenPipeError("injected partial write")

        with patch("meerkat_toolkit.host_control.os.write", interrupted):
            with self.assertRaises(MemberClassObservationUnknown) as caught:
                self.begin_add(selection())
        self.assertEqual(caught.exception.operation_id, 1)
        with self.assertRaises(MemberClassObservationUnknown) as repeated:
            caught.exception.attempt.result()
        self.assertIs(caught.exception, repeated.exception)
        self.assertEqual(len(calls), 2)
        self.assertEqual(self.reader.read(), calls[0][:8])


class MemberClassListingPipeTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        child_read, parent_write = os.pipe()
        parent_read, child_write = os.pipe()
        self.host = PrivateHostClient(parent_write, parent_read, timeout=1)
        self.client = MemberClassClient(self.host)
        self.reader = os.fdopen(child_read, "rb")
        self.writer = os.fdopen(child_write, "wb", buffering=0)

    async def asyncTearDown(self):
        await asyncio.to_thread(self.host.close)
        await asyncio.sleep(0)
        self.reader.close()
        self.writer.close()

    async def receive(self):
        return json.loads(await asyncio.wait_for(asyncio.to_thread(self.reader.readline), 2))

    def send(self, identifier, result):
        self.writer.write(json.dumps({"version": 1, "id": identifier, "result": result}).encode() + b"\n")

    def ready(self, identifier, **overrides):
        request = selection().wire()
        fields = {"admission_id": request["id"], "identity": request["spec"]["identity"], "class": request["instance"]["class"]}
        fields.update(overrides)
        self.send(identifier, {"kind": "member_class_listing_required", "operation_id": identifier, **fields})

    def ack(self, request, operation_id, **overrides):
        fields = {key: request["command"]["operation"][key] for key in ("admission_id", "identity", "class")}
        fields.update(overrides)
        self.send(request["id"], {"kind": "member_class_listing_acknowledged", "operation_id": operation_id, **fields})

    async def test_pre_native_terminal_states_preserve_ownership_and_cleanup_without_native_join(self):
        example = runpy.run_path(str(Path(__file__).resolve().parents[1] / "examples" / "forge_member_classes.py"))
        replies = [
            {"state": "reservation_not_accepted", "error": "unavailable"},
            {"state": "preflight_refused", "error": "denied", "cleanup": {"state": "succeeded"}},
            {"state": "preflight_refused", "error": "wrong_owner",
             "cleanup": {"state": "refused", "error": "unavailable"}},
        ]
        for value in replies:
            publish = AsyncMock()
            attempt = self.client.begin_add(selection(), publish=publish)
            request = await self.receive()
            self.send(request["id"], {"kind": "member_class", "observation": value})
            result = await example["observe_admitting_add"](SimpleNamespace(member_classes=self.client), attempt)
            self.assertEqual(result, {"initial": value, "terminal": value, "listing_error": None})
            self.assertNotIn("native", result["terminal"])
            self.assertEqual(self.host._pending, {}, "terminal preflight does not request Join or retry")
            self.assertEqual(self.host._member_listings, {})
            publish.assert_not_called()

    async def test_duplicate_reservation_refusal_does_not_release_original_callers_listing(self):
        entered, release = asyncio.Event(), asyncio.Event()
        async def original_publish():
            entered.set()
            await release.wait()
        original = self.client.begin_add(selection(), publish=original_publish)
        first = await self.receive()
        self.ready(first["id"])
        await entered.wait()
        rejected_publish = AsyncMock()
        rejected = self.client.begin_add(selection(), publish=rejected_publish)
        second = await self.receive()
        refusal = {"state": "reservation_not_accepted", "error": "unavailable"}
        self.send(second["id"], {"kind": "member_class", "observation": refusal})
        self.assertEqual(await asyncio.to_thread(rejected.result), refusal)
        await rejected.join_listing()
        self.assertFalse(original._listing_done.done())
        self.assertIs(self.host._member_listings[first["id"]], original)
        rejected_publish.assert_not_called()
        release.set()
        ack = await self.receive()
        self.ack(ack, first["id"])
        self.send(first["id"], {"kind": "member_class", "observation": settled()})
        await original.join_listing()
        self.assertEqual(await asyncio.to_thread(original.result), settled())

    async def _reject_preflight_shape(self, value):
        publish = AsyncMock()
        attempt = self.client.begin_add(selection(), publish=publish)
        request = await self.receive()
        self.send(request["id"], {"kind": "member_class", "observation": value})
        with self.assertRaises(MemberClassObservationUnknown):
            await asyncio.to_thread(attempt.result)
        publish.assert_not_called()

    async def test_reservation_reply_cannot_fabricate_native_result(self):
        await self._reject_preflight_shape({"state": "reservation_not_accepted", "error": "denied", "native": native()})

    async def test_preflight_reply_requires_cleanup_fact(self):
        await self._reject_preflight_shape({"state": "preflight_refused", "error": "denied"})

    async def test_preflight_reply_rejects_unknown_error(self):
        await self._reject_preflight_shape({"state": "preflight_refused", "error": "made_up",
                                           "cleanup": {"state": "succeeded"}})

    async def test_preflight_reply_rejects_ambiguous_cleanup(self):
        await self._reject_preflight_shape({"state": "preflight_refused", "error": "denied",
                                           "cleanup": {"state": "succeeded", "error": "denied"}})

    async def test_native_join_cannot_claim_a_new_preflight_attempt(self):
        joined = asyncio.create_task(asyncio.to_thread(self.client.join, 91))
        request = await self.receive()
        self.send(request["id"], {"kind": "member_class", "observation": {
            "state": "preflight_refused", "error": "denied", "cleanup": {"state": "succeeded"}}})
        with self.assertRaises(HostControlError):
            await joined

    async def test_callback_is_on_existing_loop_and_observer_cancel_does_not_cancel_it(self):
        entered, release = asyncio.Event(), asyncio.Event()
        caller_thread = threading.get_ident()
        async def publish():
            self.assertEqual(threading.get_ident(), caller_thread)
            entered.set()
            await release.wait()
        attempt = self.client.begin_add(selection(), publish=publish)
        request = await self.receive()
        self.ready(request["id"])
        await asyncio.wait_for(entered.wait(), 1)
        observer = asyncio.create_task(attempt.join_listing())
        await asyncio.sleep(0)
        observer.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await observer
        self.assertFalse(attempt._listing_done.done())
        # Native can return Unsettled while its entered completion still waits.
        self.send(request["id"], {"kind": "member_class", "observation": {
            "state": "unsettled", "settlement_id": request["id"]}})
        self.assertEqual((await asyncio.to_thread(attempt.result))["state"], "unsettled")
        self.assertFalse(attempt._listing_done.done())
        release.set()
        ack = await self.receive()
        self.assertEqual(ack["command"]["operation"], {"kind": "listing_result", "operation_id": request["id"], "accepted": True,
            "admission_id": request["command"]["operation"]["id"],
            "identity": request["command"]["operation"]["spec"]["identity"],
            "class": request["command"]["operation"]["instance"]["class"]})
        self.ack(ack, request["id"])
        await asyncio.wait_for(attempt.join_listing(), 1)
        joined = asyncio.create_task(asyncio.to_thread(self.client.join, request["id"]))
        join = await self.receive()
        self.send(join["id"], {"kind": "member_class", "observation": settled("refused")})
        await joined
        await asyncio.sleep(0)
        self.assertEqual(self.host._member_listings, {})

    async def test_unsettled_before_interim_retains_callback_and_refuses_late_listing(self):
        publish = AsyncMock()
        attempt = self.client.begin_add(selection(), publish=publish)
        request = await self.receive()
        self.send(request["id"], {"kind": "member_class", "observation": {
            "state": "unsettled", "settlement_id": request["id"]}})
        await asyncio.to_thread(attempt.result)
        self.assertIs(self.host._member_listings[request["id"]], attempt)
        self.ready(request["id"])
        ack = await self.receive()
        self.assertFalse(ack["command"]["operation"]["accepted"])
        self.ack(ack, request["id"])
        await attempt.join_listing()
        publish.assert_not_called()
        joined = asyncio.create_task(asyncio.to_thread(self.client.join, request["id"]))
        join = await self.receive()
        self.send(join["id"], {"kind": "member_class", "observation": settled("refused")})
        await joined

    async def test_actual_join_releases_listing_that_native_never_requested(self):
        publish = AsyncMock()
        attempt = self.client.begin_add(selection(), publish=publish)
        request = await self.receive()
        self.send(request["id"], {"kind": "member_class", "observation": {
            "state": "unsettled", "settlement_id": request["id"]}})
        await asyncio.to_thread(attempt.result)
        self.assertFalse(attempt._listing_done.done())
        joined = asyncio.create_task(asyncio.to_thread(self.client.join, request["id"]))
        join = await self.receive()
        self.send(join["id"], {"kind": "member_class", "observation": settled("refused")})
        await joined
        await asyncio.wait_for(attempt.join_listing(), 1)
        publish.assert_not_called()

    async def test_late_join_response_still_releases_retained_callback_after_observer_timeout(self):
        publish = AsyncMock()
        attempt = self.client.begin_add(selection(), publish=publish)
        request = await self.receive()
        self.send(request["id"], {"kind": "member_class", "observation": {
            "state": "unsettled", "settlement_id": request["id"]}})
        await asyncio.to_thread(attempt.result)
        joined = asyncio.create_task(asyncio.to_thread(self.client.join, request["id"], timeout=0.02))
        join = await self.receive()
        with self.assertRaises(TimeoutError):
            await joined
        self.assertFalse(attempt._listing_done.done())
        self.send(join["id"], {"kind": "member_class", "observation": settled("refused")})
        await asyncio.wait_for(attempt.join_listing(), 1)
        publish.assert_not_called()

    async def test_closed_application_loop_refuses_interim_without_entering_callback(self):
        loop = asyncio.new_event_loop()
        thread = threading.Thread(target=loop.run_forever)
        thread.start()
        publish = AsyncMock()
        async def begin():
            return self.client.begin_add(selection(), publish=publish)
        attempt = await asyncio.wrap_future(asyncio.run_coroutine_threadsafe(begin(), loop))
        request = await self.receive()
        loop.call_soon_threadsafe(loop.stop)
        await asyncio.to_thread(thread.join)
        loop.close()
        self.ready(request["id"])
        with self.assertRaises(MemberClassObservationUnknown):
            await asyncio.to_thread(attempt.result)
        publish.assert_not_called()

    async def test_foreign_ack_poisons_pipe_without_implying_native_activation(self):
        attempt = self.client.begin_add(selection(), publish=AsyncMock())
        request = await self.receive()
        self.ready(request["id"])
        ack = await self.receive()
        self.ack(ack, request["id"] + 1)
        with self.assertRaises(MemberClassListingUnknown):
            await attempt.join_listing()
        with self.assertRaises(MemberClassObservationUnknown):
            await asyncio.to_thread(attempt.result)

    async def test_eof_leaves_entered_callback_owned_until_application_write_settles(self):
        entered, release, finished = asyncio.Event(), asyncio.Event(), asyncio.Event()
        async def publish():
            entered.set()
            await release.wait()
            finished.set()
        attempt = self.client.begin_add(selection(), publish=publish)
        request = await self.receive()
        self.ready(request["id"])
        await entered.wait()
        self.writer.close()
        with self.assertRaises(MemberClassObservationUnknown):
            await asyncio.to_thread(attempt.result)
        self.assertFalse(finished.is_set())
        release.set()
        with self.assertRaises(MemberClassListingUnknown):
            await attempt.join_listing()
        self.assertTrue(finished.is_set())

    async def test_ack_has_reserved_capacity_when_ordinary_observations_are_full(self):
        attempt = self.client.begin_add(selection(), publish=AsyncMock())
        request = await self.receive()
        for _ in range(31):
            self.host._start_call({"kind": "member_class", "operation": {"kind": "observe", "id": "other", "member": MEMBER}})
            await self.receive()
        self.assertEqual(len(self.host._pending), 32)
        self.ready(request["id"])
        ack = await self.receive()
        self.assertEqual(len(self.host._pending), 33)
        self.assertEqual(ack["command"]["operation"]["kind"], "listing_result")
        self.ack(ack, request["id"])
        self.send(request["id"], {"kind": "member_class", "observation": settled()})
        await asyncio.wait_for(attempt.join_listing(), 1)
        await asyncio.to_thread(attempt.result)

    async def test_duplicate_interim_poison_does_not_cancel_entered_application_mutation(self):
        entered, release, finished = asyncio.Event(), asyncio.Event(), asyncio.Event()
        async def publish():
            entered.set()
            await release.wait()
            finished.set()
        attempt = self.client.begin_add(selection(), publish=publish)
        request = await self.receive()
        self.ready(request["id"])
        await entered.wait()
        self.ready(request["id"])
        with self.assertRaises(MemberClassObservationUnknown):
            await asyncio.to_thread(attempt.result)
        self.assertFalse(finished.is_set())
        release.set()
        with self.assertRaises(MemberClassListingUnknown):
            await attempt.join_listing()
        self.assertTrue(finished.is_set())

    async def test_foreign_interim_correlation_poisoned_before_callback(self):
        publish = AsyncMock()
        attempt = self.client.begin_add(selection(), publish=publish)
        request = await self.receive()
        self.ready(request["id"], operation_id=request["id"] + 1)
        with self.assertRaises(MemberClassObservationUnknown):
            await asyncio.to_thread(attempt.result)
        with self.assertRaises(MemberClassListingUnknown):
            await attempt.join_listing()
        publish.assert_not_called()

    async def _foreign_ready(self, field):
        publish = AsyncMock()
        attempt = self.client.begin_add(selection(), publish=publish)
        request = await self.receive()
        self.ready(request["id"], **{field: "another-owner"})
        with self.assertRaises(MemberClassObservationUnknown):
            await asyncio.to_thread(attempt.result)
        with self.assertRaises(MemberClassListingUnknown):
            await attempt.join_listing()
        publish.assert_not_called()

    async def test_foreign_admission_interim_refuses_before_callback(self):
        await self._foreign_ready("admission_id")

    async def test_foreign_identity_interim_refuses_before_callback(self):
        await self._foreign_ready("identity")

    async def test_foreign_class_interim_refuses_before_callback(self):
        await self._foreign_ready("class")

    async def test_foreign_ack_selection_poisoned_after_actual_callback_settles(self):
        publish = AsyncMock()
        attempt = self.client.begin_add(selection(), publish=publish)
        request = await self.receive()
        self.ready(request["id"])
        ack = await self.receive()
        self.ack(ack, request["id"], admission_id="another-admission")
        with self.assertRaises(MemberClassListingUnknown):
            await attempt.join_listing()
        with self.assertRaises(MemberClassObservationUnknown):
            await asyncio.to_thread(attempt.result)
        publish.assert_awaited_once()

    async def test_late_ack_denied_keeps_native_result_separate_from_listing(self):
        publish = AsyncMock()
        attempt = self.client.begin_add(selection(), publish=publish)
        request = await self.receive()
        self.ready(request["id"])
        ack = await self.receive()
        self.send(ack["id"], {"kind": "error", "error": "denied"})
        self.send(request["id"], {"kind": "member_class", "observation": settled("refused")})
        observed = await asyncio.to_thread(attempt.result)
        self.assertEqual(observed["native"]["result"], native("refused"))
        with self.assertRaises(MemberClassListingUnknown):
            await attempt.join_listing()
        publish.assert_awaited_once()

    async def _consumer_keeps_terminal_with_failed_listing(self, *, callback_fails):
        example = runpy.run_path(str(Path(__file__).resolve().parents[1] / "examples" / "forge_member_classes.py"))
        publish = AsyncMock(side_effect=RuntimeError("durable write unknown") if callback_fails else None)
        attempt = self.client.begin_add(selection(), publish=publish)
        request = await self.receive()
        runtime = SimpleNamespace(member_classes=self.client)
        observation = asyncio.create_task(example["observe_admitting_add"](runtime, attempt))
        self.ready(request["id"])
        ack = await self.receive()
        self.assertEqual(ack["command"]["operation"]["accepted"], not callback_fails)
        initial = {"state": "unsettled", "settlement_id": request["id"]}
        self.send(request["id"], {"kind": "member_class", "observation": initial})
        join = await self.receive()
        self.assertEqual(join["command"]["operation"], {"kind": "join", "settlement_id": request["id"]})
        terminal = settled("refused", authorization={"state": "refused", "error": "denied"},
                           cleanup={"state": "refused", "error": "unavailable"})
        self.send(join["id"], {"kind": "member_class", "observation": terminal})
        if callback_fails:
            self.ack(ack, request["id"])
        else:
            self.send(ack["id"], {"kind": "error", "error": "denied"})
        result = await asyncio.wait_for(observation, 1)
        self.assertEqual(result["initial"], initial)
        self.assertEqual(result["terminal"], terminal)
        self.assertIsInstance(result["listing_error"], MemberClassListingUnknown)
        self.assertEqual(self.host._pending, {}, "no retry or additional native Join")
        publish.assert_awaited_once()

    async def test_consumer_retains_native_join_after_late_positive_ack_denied(self):
        await self._consumer_keeps_terminal_with_failed_listing(callback_fails=False)

    async def test_consumer_retains_native_join_after_failed_application_listing(self):
        await self._consumer_keeps_terminal_with_failed_listing(callback_fails=True)

    async def test_missing_callback_refuses_before_any_bytes(self):
        with self.assertRaises(HostControlError):
            self.client.begin_add(selection(), publish=None)
        await asyncio.to_thread(self.host.close)
        self.assertEqual(self.reader.read(), b"")

    async def test_callback_failure_sends_negative_ack_without_claiming_no_mutation(self):
        attempt = self.client.begin_add(selection(), publish=AsyncMock(side_effect=RuntimeError("partial durable write")))
        request = await self.receive()
        self.ready(request["id"])
        ack = await self.receive()
        self.assertFalse(ack["command"]["operation"]["accepted"])
        self.ack(ack, request["id"])
        with self.assertRaises(MemberClassListingUnknown):
            await attempt.join_listing()
        self.send(request["id"], {"kind": "member_class", "observation": {
            "state": "native_refused", "message": "completion refused", "data": None}})
        self.assertEqual((await asyncio.to_thread(attempt.result))["state"], "native_refused")


class ForgeRefusalExampleTests(unittest.IsolatedAsyncioTestCase):
    async def test_begin_admitting_add_does_not_publish_or_refresh_desired_projection(self):
        example = runpy.run_path(str(Path(__file__).resolve().parents[1] / "examples" / "forge_member_classes.py"))
        begin = example["begin_admitting_add"]
        application = SimpleNamespace(
            deployment={"applicationProfiles": {"project": "project"}, "agents": [{"profile": "seed-coordinator"}]},
            _snapshot=({}, []), _prepare_snapshot=AsyncMock())
        request, publish, runtime = selection(), AsyncMock(), Mock()
        with patch.dict(begin.__globals__, {"_same_runtime": Mock()}):
            attempt = begin(application, runtime, request, publish=publish)
        self.assertIs(attempt, runtime.member_classes.begin_add.return_value)
        runtime.member_classes.begin_add.assert_called_once_with(request, publish=publish)
        application._prepare_snapshot.assert_not_called()
        publish.assert_not_called()
        runtime.member_classes.begin_add.reset_mock()
        application._snapshot = ({}, [request.spec])
        with patch.dict(begin.__globals__, {"_same_runtime": Mock()}):
            with self.assertRaisesRegex(ValueError, "excluded from the desired projection"):
                begin(application, runtime, request, publish=publish)
        runtime.member_classes.begin_add.assert_not_called()

    async def test_consumer_joins_native_then_listing_before_returning_for_application_cleanup(self):
        example = runpy.run_path(str(Path(__file__).resolve().parents[1] / "examples" / "forge_member_classes.py"))
        calls = []
        initial = {"state": "unsettled", "settlement_id": 7}
        terminal = settled(authorization={"state": "refused", "error": "denied"})
        async def join_listing():
            calls.append("listing")
        def join_native(identifier):
            self.assertEqual(identifier, 7)
            calls.append("native")
            return terminal
        attempt = SimpleNamespace(result=Mock(return_value=initial), join_listing=join_listing)
        runtime = SimpleNamespace(member_classes=SimpleNamespace(join=join_native))
        result = await example["observe_admitting_add"](runtime, attempt)
        self.assertEqual(calls, ["native", "listing"])
        self.assertEqual(result, {"initial": initial, "terminal": terminal, "listing_error": None})

    async def test_consumer_does_not_swallow_cancellation_or_unexpected_listing_error(self):
        example = runpy.run_path(str(Path(__file__).resolve().parents[1] / "examples" / "forge_member_classes.py"))
        for error in (asyncio.CancelledError(), RuntimeError("unexpected error")):
            attempt = SimpleNamespace(result=Mock(return_value=settled()),
                                      join_listing=AsyncMock(side_effect=error))
            runtime = SimpleNamespace(member_classes=SimpleNamespace(join=Mock()))
            with self.assertRaises(type(error)) as raised:
                await example["observe_admitting_add"](runtime, attempt)
            self.assertIs(raised.exception, error)
            runtime.member_classes.join.assert_not_called()

    async def test_existing_restored_observation_does_not_prepare_add(self):
        example = runpy.run_path(str(Path(__file__).resolve().parents[1] / "examples" / "forge_member_classes.py"))
        observe = example["observe_restored"]
        request = selection()
        application = SimpleNamespace(
            deployment={"applicationProfiles": {"project": "project"},
                        "agents": [{"profile": "seed-coordinator"}]},
            _reconcile_lock=asyncio.Lock(), _prepare_snapshot=AsyncMock(),
            _snapshot=({}, [request.spec]))
        runtime = SimpleNamespace(
            member_classes=SimpleNamespace(observe=Mock(return_value=observed()),
                                           begin_add=Mock(), retire=Mock()),
            status=AsyncMock(return_value={"status": "existing"}),
            identity_resolved_tools_detail=AsyncMock(return_value={"tools": []}))
        with patch.dict(observe.__globals__, {
                "_same_runtime": Mock(),
                "begin_admitting_add": Mock(side_effect=AssertionError("must not prepare Add"))}):
            result = await observe(application, runtime, request)
        self.assertEqual(result["authorization"], observed())
        self.assertEqual(result["native_status"], {"status": "existing"})
        application._prepare_snapshot.assert_awaited_once_with(runtime=runtime)
        runtime.member_classes.observe.assert_called_once_with(request.id, MEMBER)
        runtime.member_classes.begin_add.assert_not_called()
        runtime.member_classes.retire.assert_not_called()

        application._snapshot = ({}, [])
        runtime.member_classes.observe.reset_mock()
        with patch.dict(observe.__globals__, {"_same_runtime": Mock()}):
            with self.assertRaisesRegex(ValueError, "existing desired projection"):
                await observe(application, runtime, request)
        runtime.member_classes.observe.assert_not_called()

    async def test_only_denied_counts_as_unknown_class_refusal(self):
        exercise = runpy.run_path(str(Path(__file__).resolve().parents[1] / "examples" / "forge_member_classes.py"))[
            "observe_unknown_class_refusal"]
        for code in HostReplyCode:
            error = HostReplyError(code)
            runtime = SimpleNamespace(member_classes=SimpleNamespace(begin_add=Mock(side_effect=error)))
            if code == HostReplyCode.DENIED:
                self.assertEqual(await exercise(runtime, selection(), "absent-class"), {"native_reply": "denied"})
            else:
                with self.assertRaises(HostReplyError) as caught:
                    await exercise(runtime, selection(), "absent-class")
                self.assertIs(caught.exception, error)
            runtime.member_classes.begin_add.assert_called_once()

    async def test_unknown_observation_is_not_a_refusal(self):
        exercise = runpy.run_path(str(Path(__file__).resolve().parents[1] / "examples" / "forge_member_classes.py"))[
            "observe_unknown_class_refusal"]
        error = MemberClassObservationUnknown(SimpleNamespace(operation_id=19))
        runtime = SimpleNamespace(member_classes=SimpleNamespace(begin_add=Mock(side_effect=error)))
        with self.assertRaises(MemberClassObservationUnknown) as caught:
            await exercise(runtime, selection(), "absent-class")
        self.assertIs(caught.exception, error)


if __name__ == "__main__":
    unittest.main()


class MemberMcpCallbackTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        from meerkat_toolkit.member_classes import MemberMcpApplication, MemberMcpAdmissionSelection
        self.selection = MemberMcpAdmissionSelection("add-project-one", "project-one", "project")
        self.rows = {}
        self.entries = []
        async def retain(value):
            self.entries.append(value)
            prior = self.rows.get(value.selection.admission_id)
            if prior is not None and prior != value:
                raise HostControlError("application CAS mismatch")
            self.rows[value.selection.admission_id] = value
        async def lookup(key):
            value = self.rows[key.admission_id]
            if value.selection != key:
                raise HostControlError("application row mismatch")
            return value
        self.participant = MemberMcpApplication(retain, lookup)
        child_read, parent_write = os.pipe()
        parent_read, child_write = os.pipe()
        self.host = PrivateHostClient(parent_write, parent_read, timeout=1, member_mcp=self.participant)
        self.reader = os.fdopen(child_read, "rb")
        self.writer = os.fdopen(child_write, "wb", buffering=0)

    async def asyncTearDown(self):
        self.host.close()
        self.reader.close()
        self.writer.close()
        try:
            await self.participant.join()
        except Exception:
            pass

    def request(self, exchange, bindings, **extra):
        value = {"kind": "member_class_mcp_required", "exchange_id": exchange,
                 "selection": self.selection.wire(), "bindings": bindings, **extra}
        self.writer.write(json.dumps({"version": 1, "id": 0, "result": value}).encode() + b"\n")
        return value

    async def ack(self):
        value = json.loads(await asyncio.to_thread(self.reader.readline))
        operation = value["command"]["operation"]
        self.writer.write(json.dumps({"version": 1, "id": value["id"], "result": {
            "kind": "member_class_mcp_acknowledged", "exchange_id": operation["exchange_id"]}}).encode() + b"\n")
        return operation

    async def settled(self):
        while self.participant._pending:
            await asyncio.sleep(0.001)

    def requirement(self):
        from meerkat_toolkit.host_control import PrivateSourceEvidence
        from meerkat_toolkit.member_classes import ObservedMemberMcpRequirement, MemberMcpServiceIntent
        lifetime = {"bound": {"bound": "window", "window": {"not_before_ms": 10, "expires_at_ms": 20}}, "unresolved": []}
        # Preserve service order separately from each native operation set.
        return ObservedMemberMcpRequirement(self.selection,
            PrivateSourceEvidence("requirements/project-one", "revision-7", "a" * 64),
            (MemberMcpServiceIntent("zeta", ("read",), lifetime),
             MemberMcpServiceIntent("alpha", ("inspect",), lifetime)))

    async def observe_with(self, callback):
        from meerkat_toolkit.member_classes import MemberMcpApplication
        prior = self.participant
        await prior.join()  # No work has entered the original pipe participant.
        self.participant = MemberMcpApplication(prior._retain, prior._lookup, observed_requirement=callback)
        self.host._member_mcp = self.participant

    async def test_observed_requirement_pipe_preserves_full_retention_and_cleanup_lookup(self):
        from meerkat_toolkit.member_classes import RetainedMemberMcpBindings
        requirement = self.requirement()
        current = requirement
        observations = []
        async def observe(key, source):
            observations.append((key, source))
            return current
        await self.observe_with(observe)
        self.assertEqual(self.participant.requirement_contract, "observed_requirement")
        self.request(1, None, requirement_source=dataclasses.asdict(requirement.source))
        first = await self.ack()
        self.assertIsNone(first["retained"])
        self.assertEqual(first["requirement"], requirement.wire())
        self.assertEqual([row["server"] for row in first["requirement"]["services"]], ["zeta", "alpha"])
        await self.settled()
        self.assertEqual(observations, [(self.selection, requirement.source)])
        self.assertEqual(self.entries, [])  # Observation is not retention or publication.
        self.assertEqual(self.rows, {})

        self.request(2, ["pending-zeta", "pending-alpha"], accepted_requirement=first["requirement"])
        retained = (await self.ack())["retained"]
        await self.settled()
        self.assertEqual(retained, {"selection": self.selection.wire(),
            "bindings": ["pending-zeta", "pending-alpha"], "accepted_requirement": requirement.wire()})
        self.assertEqual(self.entries, [RetainedMemberMcpBindings.from_wire(retained)])
        current = None  # The owner now reports a void/missing requirement.
        self.request(3, None, requirement_source=dataclasses.asdict(requirement.source))
        refused = await self.ack()
        await self.settled()
        self.assertIsNone(refused["retained"])
        self.assertNotIn("requirement", refused)
        self.request(4, None)
        self.assertEqual((await self.ack())["retained"], retained)
        await self.settled()
        self.assertEqual(len(self.entries), 1, "cleanup lookup never re-retains or publishes")
        self.assertEqual(self.rows[self.selection.admission_id].wire(), retained)

    async def test_missing_observation_capability_refuses_without_breaking_operator_self(self):
        requirement = self.requirement()
        self.assertIsNone(self.participant.requirement_contract)
        self.request(1, None, requirement_source=dataclasses.asdict(requirement.source))
        refused = await self.ack()
        await self.settled()
        self.assertIsNone(refused["retained"])
        self.assertNotIn("requirement", refused)
        self.assertEqual(self.entries, [])
        self.assertEqual(self.rows, {})
        self.assertIsNone(self.host._failure)
        self.request(2, [])
        ordinary = await self.ack()
        await self.settled()
        self.assertEqual(ordinary, {"kind": "mcp_result", "exchange_id": 2,
            "selection": self.selection.wire(), "retained": {"selection": self.selection.wire(), "bindings": []}})

    async def test_observation_rejects_foreign_selection_source_and_untyped_value(self):
        requirement = self.requirement()
        changed = [dataclasses.replace(requirement, selection=dataclasses.replace(self.selection, identity="other")),
                   dataclasses.replace(requirement, source=dataclasses.replace(requirement.source, revision="revision-8")),
                   requirement.wire()]
        async def observe(key, source):
            return changed.pop(0)
        await self.observe_with(observe)
        for exchange in range(1, 4):
            self.request(exchange, None, requirement_source=dataclasses.asdict(requirement.source))
            refused = await self.ack()
            await self.settled()
            self.assertIsNone(refused["retained"])
            self.assertNotIn("requirement", refused)
        self.assertEqual(changed, [])
        self.assertEqual(self.entries, [])
        self.assertEqual(self.rows, {})
        self.assertIsNone(self.host._failure)

    async def test_observed_requirement_malformed_frames_never_enter_callbacks_or_write(self):
        requirement = self.requirement().wire()
        base = {"kind": "member_class_mcp_required", "exchange_id": 1,
                "selection": self.selection.wire(), "bindings": ["pending-zeta", "pending-alpha"],
                "accepted_requirement": requirement}
        malformed = [dict(base, requirement_source=requirement["source"]),
                     dict(base, bindings=None), dict(base, bindings=["pending-zeta"]),
                     dict(base, accepted_requirement=None)]
        for field, value in [("contract", "current_owner"), ("source", None), ("services", None),
                             ("selection", dict(self.selection.wire(), identity="other"))]:
            malformed.append(dict(base, accepted_requirement=dict(requirement, **{field: value})))
        malformed.append({"kind": "member_class_mcp_required", "exchange_id": 1,
            "selection": self.selection.wire(), "bindings": None, "requirement_source": None})
        with patch.object(self.participant._loop, "call_soon_threadsafe") as schedule, patch.object(self.host, "_start_call") as write:
            for frame in malformed:
                with self.subTest(frame=frame), self.assertRaises(HostControlError):
                    self.participant._request(self.host, frame)
            schedule.assert_not_called()
            write.assert_not_called()
        self.assertFalse(self.participant._pending)
        self.assertEqual(self.entries, [])
        self.assertEqual(self.rows, {})

    async def test_observation_pipe_loss_joins_entered_callback_without_automatic_retry(self):
        entered, release = asyncio.Event(), asyncio.Event()
        calls = []
        requirement = self.requirement()
        async def observe(key, source):
            calls.append((key, source))
            entered.set()
            await release.wait()
            return requirement
        await self.observe_with(observe)
        self.request(1, None, requirement_source=dataclasses.asdict(requirement.source))
        try:
            await asyncio.wait_for(entered.wait(), 1)
            self.host._fail(HostControlError("synthetic observation pipe loss"))
            joining = asyncio.create_task(self.participant.join())
            await asyncio.sleep(0)
            self.assertFalse(joining.done())
        finally:
            release.set()  # Assertion failure must also release the entered callback.
        with self.assertRaises(Exception):
            await joining
        self.assertEqual(calls, [(self.selection, requirement.source)])
        self.assertEqual(self.entries, [])
        self.assertEqual(self.rows, {})

    async def test_accepted_add_origin_and_observed_codec_preserve_exact_source(self):
        from meerkat_toolkit.member_classes import MemberMcpAddIntent, ObservedMemberMcpRequirement
        requirement = self.requirement()
        origin = {"accepted_application_requirement": {"source": dataclasses.asdict(requirement.source)}}
        intent = MemberMcpAddIntent(origin, 7, requirement.services)
        self.assertEqual(dataclasses.replace(selection(), mcp=intent).wire()["mcp"], {
            "origin": origin, "console_enrollment_id": 7, "services": requirement.wire()["services"]})
        self.assertEqual(ObservedMemberMcpRequirement.from_wire(requirement.wire()), requirement)
        empty = dataclasses.replace(requirement, services=())
        self.assertEqual(ObservedMemberMcpRequirement.from_wire(empty.wire()), empty)
        self.assertEqual(dataclasses.replace(intent, services=()).wire()["services"], [])
        with self.assertRaises(HostControlError):
            dataclasses.replace(intent, origin="operator_self", services=()).wire()
        for source in [None, {}, dict(dataclasses.asdict(requirement.source), sha256="A" * 64)]:
            with self.subTest(source=source), self.assertRaises(HostControlError):
                dataclasses.replace(intent, origin={"accepted_application_requirement": {"source": source}}).wire()

    async def test_exact_replay_and_empty_are_explicit_and_changed_refs_refuse(self):
        self.request(1, [])
        first = await self.ack()
        self.assertEqual(first["retained"], {"selection": self.selection.wire(), "bindings": []})
        await self.settled()
        self.request(2, [])
        self.assertEqual((await self.ack())["retained"], first["retained"])
        await self.settled()
        self.request(3, ["changed-binding"])
        self.assertIsNone((await self.ack())["retained"])
        await self.settled()
        self.assertEqual(self.rows[self.selection.admission_id].bindings, ())
        self.request(4, None)
        self.assertEqual((await self.ack())["retained"], first["retained"])
        await self.settled()

    async def test_missing_lookup_refuses_and_late_duplicate_never_reenters_callback(self):
        frame = self.request(1, None)
        self.assertIsNone((await self.ack())["retained"])
        await self.settled()
        with self.assertRaises(HostControlError):
            self.participant._request(self.host, frame)
        self.assertEqual(self.entries, [])
        self.assertEqual(self.rows, {})

    async def test_shutdown_fences_queued_unentered_callback(self):
        scheduled = []
        frame = {"kind": "member_class_mcp_required", "exchange_id": 1,
                 "selection": self.selection.wire(), "bindings": []}
        with patch.object(self.participant._loop, "call_soon_threadsafe", side_effect=lambda *args: scheduled.append(args)):
            self.participant._request(self.host, frame)
        # Simulate a start queued before the fence but dispatched afterwards.
        with self.participant._lock:
            self.participant._closed = True
        function, *args = scheduled[0]
        function(*args)
        self.assertIsNone((await self.ack())["retained"])
        await self.settled()
        self.assertEqual(self.entries, [])
        self.assertEqual(self.rows, {})

    async def test_pipe_loss_does_not_cancel_entered_durable_write(self):
        entered, release = asyncio.Event(), asyncio.Event()
        async def retain(value):
            entered.set()
            await release.wait()
            self.rows[value.selection.admission_id] = value
        self.participant._retain = retain
        self.request(1, [])
        await asyncio.wait_for(entered.wait(), 1)
        self.host._fail(HostControlError("synthetic transport loss"))
        joining = asyncio.create_task(self.participant.join())
        await asyncio.sleep(0)
        self.assertFalse(joining.done())
        release.set()
        with self.assertRaises(Exception):
            await joining
        self.assertEqual(self.rows[self.selection.admission_id].bindings, ())

    async def test_ack_refusal_before_write_closes_pipe_and_preserves_committed_row(self):
        with patch.object(self.host, "_start_call", side_effect=HostControlError("reserved ACK capacity exhausted")):
            self.request(1, [])
            self.assertEqual(await asyncio.to_thread(self.reader.readline), b"")
            await self.settled()
        self.assertIsNotNone(self.host._failure)
        self.assertIsNone(self.host._command_fd)
        self.assertEqual(self.rows[self.selection.admission_id].bindings, ())

    async def test_typed_late_ack_refusal_keeps_shared_pipe_open(self):
        self.request(1, [])
        value = json.loads(await asyncio.to_thread(self.reader.readline))
        self.writer.write(json.dumps({"version": 1, "id": value["id"], "result": {
            "kind": "error", "error": "denied"}}).encode() + b"\n")
        await self.settled()
        self.assertIsNone(self.host._failure)
        self.assertIsNotNone(self.host._command_fd)
        self.assertEqual(self.rows[self.selection.admission_id].bindings, ())
        # A different cold lookup still completes over this same client.
        self.request(2, None)
        self.assertEqual((await self.ack())["retained"]["bindings"], [])
        await self.settled()
        with self.assertRaisesRegex(Exception, "requires reconciliation"):
            await self.participant.join()

    async def test_explicit_runtime_shutdown_joins_entered_write_and_preserves_native_error(self):
        from meerkat_toolkit.host_control import MetaRuntime, ToolkitMobKitRuntime
        runtime = object.__new__(ToolkitMobKitRuntime)
        runtime._member_mcp = self.participant
        entered, release = asyncio.Event(), asyncio.Event()
        async def retain(value):
            entered.set()
            await release.wait()
            self.rows[value.selection.admission_id] = value
        self.participant._retain = retain
        self.request(1, [])
        await asyncio.wait_for(entered.wait(), 1)
        primary = RuntimeError("native shutdown did not settle")
        with patch.object(MetaRuntime, "shutdown", AsyncMock(side_effect=primary)):
            shutting = asyncio.create_task(runtime.shutdown())
            await asyncio.sleep(0)
            self.assertFalse(shutting.done())
            release.set()
            await self.ack()
            with self.assertRaises(RuntimeError) as caught:
                await shutting
        self.assertIs(caught.exception, primary)
        self.assertEqual(self.rows[self.selection.admission_id].bindings, ())
        await self.participant.join()  # Explicit app close and runtime close share one drain.

    async def test_failed_bootstrap_fences_unentered_callback_and_retains_cleanup_error(self):
        from meerkat_toolkit.host_control import MetaRuntime, ToolkitMobKitRuntime
        runtime = object.__new__(ToolkitMobKitRuntime)
        runtime._member_mcp = self.participant
        frame = {"kind": "member_class_mcp_required", "exchange_id": 1,
                 "selection": self.selection.wire(), "bindings": []}
        # Queue callback start, then enter actual failed-bootstrap cleanup before
        # the event loop dispatches it. No application transaction may start.
        self.participant._request(self.host, frame)
        primary = RuntimeError("bootstrap failed")
        with patch.object(MetaRuntime, "_cleanup_failed_bootstrap", AsyncMock()):
            acknowledgment = asyncio.create_task(self.ack())
            # Enter the real cleanup override before yielding to queued _start.
            await runtime._cleanup_failed_bootstrap(None, primary)
            self.assertIsNone((await acknowledgment)["retained"])
        self.assertEqual(self.entries, [])
        self.assertTrue(hasattr(primary, "member_mcp_cleanup_error"))
        self.assertEqual(self.rows, {})

    async def test_add_intent_is_explicit_bounded_and_preserves_native_lifetime_shape(self):
        from meerkat_toolkit.member_classes import MemberMcpAddIntent, MemberMcpServiceIntent
        lifetime = {"bound": {"bound": "window", "window": {"not_before_ms": 10, "expires_at_ms": 20}}, "unresolved": []}
        service = MemberMcpServiceIntent("hn", ("read",), lifetime)
        request = dataclasses.replace(selection(), mcp=MemberMcpAddIntent("operator_self", 7, (service,)))
        self.assertEqual(request.wire()["mcp"], {"origin": "operator_self", "console_enrollment_id": 7,
            "services": [{"server": "hn", "operations": ["read"], "lifetime": lifetime}]})
        for intent in (MemberMcpAddIntent("", 7, (service,)), MemberMcpAddIntent("agent", 7, (service,)),
                       MemberMcpAddIntent("operator_self", 0, (service,)), MemberMcpAddIntent("operator_self", 7, (service, service))):
            with self.assertRaises(HostControlError):
                dataclasses.replace(request, mcp=intent).wire()
        self.assertNotIn("mcp", selection().wire())
