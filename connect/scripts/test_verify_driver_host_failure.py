"""Offline typed-error observer controls, not SDK/native execution proof."""
import asyncio
import copy
import dataclasses
import json
import hashlib
import sqlite3
from pathlib import Path
import tempfile
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, Mock, patch

import verify_supplied_shared_workflow as w


@dataclasses.dataclass
class Config:
    error_callback: object = None


@dataclasses.dataclass
class Retired:
    fenced: bool = True
    reservations_drained: bool = True
    uncertain: bool = False


def event(member=w.G_ID, category="host_loop_crash"):
    # The retained Meta ErrorEvent contract is category + context.member_id.
    # No installed SDK is imported and this is not wire-delivery evidence.
    return SimpleNamespace(category=category, message="unretained private detail",
        context={"member_id": member, "error": "unretained private detail"})


class SelectedHostFailureControls(unittest.IsolatedAsyncioTestCase):
    async def test_exact_selection_forwards_each_identical_event_and_bounds_observation(self):
        forwarded = []
        observer = w.s.SelectedHostLoopFailure(w.G_ID, "live", forwarded.append)
        foreign = [event("other-member"), event(category="checkpoint_failure"),
                   event(member=w.C3_ID)]
        for row in foreign:
            await observer(row)
            observer.require_alive()
            self.assertIsNone(observer.observation())
        selected = event()
        for _ in range(3):
            await observer(selected)
        self.assertEqual(len(forwarded), 6)
        self.assertTrue(all(a is b for a, b in zip(forwarded, foreign + [selected] * 3)))
        self.assertEqual(observer.observation(), {
            "category": "host_loop_crash", "member_id": w.G_ID, "phase": "live"})
        with self.assertRaisesRegex(AssertionError, "typed host_loop_crash"):
            observer.require_alive()

    async def test_previous_async_hook_is_awaited_and_its_failure_cannot_erase_observation(self):
        seen = []
        original_error = RuntimeError("original hook failed")
        async def previous(row):
            await asyncio.sleep(0)
            seen.append(row)
            raise original_error
        selected = event()
        observer = w.s.SelectedHostLoopFailure(w.G_ID, "live", previous)
        with self.assertRaises(RuntimeError) as failure:
            await observer(selected)
        self.assertIs(failure.exception, original_error)
        self.assertIs(seen[0], selected)
        with self.assertRaisesRegex(AssertionError, "typed host_loop_crash"):
            observer.require_alive()

    async def test_restored_config_has_distinct_phase_and_retired_callback_cannot_poison_it(self):
        forwarded = []
        scenario = w.ColdSharedScenario(w.v, "http://127.0.0.1:1234")
        original = Config(forwarded.append)
        scenario.configure_error_observer(original)
        prepared = original.error_callback
        restored = dataclasses.replace(original)
        scenario.phase = "restored"
        scenario.configure_error_observer(restored)
        self.assertIsNot(restored.error_callback, prepared)
        self.assertIs(original.error_callback, prepared)
        self.assertIs(restored.error_callback.previous, prepared.previous)
        old_event = event()
        await prepared(old_event)
        scenario.host_loop_failure.require_alive()
        self.assertIsNone(restored.error_callback.observation())
        current_event = event()
        await restored.error_callback(current_event)
        self.assertEqual([row is expected for row, expected in zip(forwarded, [old_event, current_event])], [True, True])
        self.assertEqual(restored.error_callback.observation()["phase"], "restored")
        self.assertEqual(prepared.observation()["phase"], "prepared")

    def scenario(self, cls):
        scenario = cls(w.v, "http://127.0.0.1:1234")
        config = Config()
        scenario.configure_error_observer(config)
        scenario.customizer = SimpleNamespace(builds=[], created=[])
        return scenario, config

    def ownership(self, order):
        enrollment = SimpleNamespace(result=lambda: order.append("login-result"),
            retire=lambda **kw: (order.append(("retire", kw)), Retired())[1])
        private = SimpleNamespace(begin_console_login=lambda *a, **kw: enrollment)
        provider = SimpleNamespace(lock=threading.RLock(), requests=[], enabled=False, errors=[],
            abort_b=threading.Event(), release_b=threading.Event(), finish_abort=threading.Event(),
            finish_release=threading.Event(), release_original_responses=lambda **kw: order.append(("original-release", kw)))
        return private, provider

    async def test_driver_observed_failure_runs_original_login_and_model_cleanup(self):
        scenario, config = self.scenario(w.s.Scenario)
        order = []
        private, provider = self.ownership(order)
        async def begin(*args):
            await config.error_callback(event())
            order.append("workflow-returned")
            return None
        scenario.begin_workflow = begin
        connection = SimpleNamespace(request=lambda *a, **kw: None,
            getresponse=lambda: SimpleNamespace(status=200, read=lambda _: b'{"review":null}'),
            close=lambda: order.append("http-closed"))
        sdk = SimpleNamespace(ConsoleLoginSelection=lambda *a: a)
        with tempfile.TemporaryDirectory() as temporary, \
                patch.object(w.v, "SDK", sdk, create=True), \
                patch.object(w.s.c, "signed_login", return_value="offline"), \
                patch.object(w.s, "HTTPConnection", return_value=connection), \
                patch.object(w.s.asyncio, "sleep", new=AsyncMock(side_effect=AssertionError("unexpected poll wait"))):
            directory = Path(temporary)
            with self.assertRaisesRegex(AssertionError, "typed host_loop_crash"):
                await scenario.run(private, provider, directory, [], SimpleNamespace(poll=lambda: None), scenario.origin)
            report = json.loads((directory / "console-journey.json").read_text())
        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["selected_host_failure"], config.error_callback.observation())
        self.assertEqual(report["login_retirement"], dataclasses.asdict(Retired()))
        self.assertTrue(provider.abort_b.is_set() and provider.release_b.is_set())
        self.assertEqual(order, ["login-result", "http-closed", "workflow-returned", ("retire", {"timeout": 30})])
        self.assertNotIn("cleanup_error", report)

    async def test_both_observed_failure_keeps_original_release_and_retirement(self):
        scenario, config = self.scenario(w.BothSelectedScenario)
        order = []
        private, provider = self.ownership(order)
        scenario.finalizer = SimpleNamespace(application_tool_policy=b"existing-policy")
        scenario.begin_workflow = AsyncMock(return_value={"children": [{"tag": "a"}]})
        scenario.workflow_context = (None, None, None, {"record": {"workflow": {"workflow": "existing-G"}}}, object())
        async def observed(member):
            await config.error_callback(event())
            order.append("observed-returned")
            return SimpleNamespace(session_id="existing-session"), None
        scenario.observe_shared = observed
        with tempfile.TemporaryDirectory() as temporary, \
                patch.object(w.v, "SDK", SimpleNamespace(ConsoleLoginSelection=lambda *a: a), create=True), \
                patch.object(w.c, "signed_login", return_value="offline"), \
                patch.object(w.s, "HTTPConnection", side_effect=AssertionError("unexpected post-crash HTTP")):
            directory = Path(temporary)
            with self.assertRaisesRegex(AssertionError, "typed host_loop_crash"):
                await scenario.run_shared(private, provider, directory, [], SimpleNamespace(poll=lambda: None), scenario.origin)
            report = json.loads((directory / "console-journey.json").read_text())
        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["selected_host_failure"], config.error_callback.observation())
        self.assertTrue(provider.finish_abort.is_set() and provider.finish_release.is_set())
        self.assertEqual(order, ["login-result", "observed-returned", ("original-release", {"abort": True}),
                                 ("retire", {"timeout": 30})])
        self.assertNotIn("cleanup_errors", report)

    async def test_warm_g_wait_fails_before_reading_any_new_completion(self):
        scenario, config = self.scenario(w.Scenario)
        scenario.observe_shared = AsyncMock(return_value=(SimpleNamespace(session_id="existing-session",
            delivery_scope=SimpleNamespace(session_id="existing-session")),
            {"canonical": {"record": {"workflow": {"workflow": "existing-workflow"}}}}))
        await config.error_callback(event())
        provider = SimpleNamespace(lock=threading.RLock(), errors=[], b_context=threading.Event(), issued=set())
        with patch.object(w, "context_delivery", side_effect=AssertionError("unexpected receipt read")):
            with self.assertRaisesRegex(AssertionError, "typed host_loop_crash"):
                await scenario.wait_finalized_a({"source_key": "selected"}, object(), provider, Path("/unused"), w.time.monotonic() + 1)

    async def test_cold_g_wait_fails_without_mutating_publication_fault_or_approving(self):
        scenario, config = self.scenario(w.ColdSharedScenario)
        scenario.cold = {}
        scenario.observe_shared = AsyncMock(return_value=(SimpleNamespace(session_id="existing-session"),
            {"canonical": {"record": {"workflow": {"workflow": "existing-workflow"}}}}))
        await config.error_callback(event())
        with patch.object(w, "selected_completion_fault", side_effect=AssertionError("unexpected fault change")), \
                patch.object(w.s, "HTTPConnection", side_effect=AssertionError("unexpected approval/read")):
            with self.assertRaisesRegex(AssertionError, "typed host_loop_crash"):
                await scenario.wait_selected_a({}, object(), None, Path("/unused"), None, w.time.monotonic() + 1, "offline")
        self.assertEqual(scenario.cold, {"selected_a_http": []})

    async def test_helperless_restored_c3_failure_preserves_listing_join_and_exact_callback_identity(self):
        scenario, config = self.scenario(w.HelperlessScenario)
        scenario.phase = "restored"
        restored = dataclasses.replace(config)
        scenario.configure_error_observer(restored)
        await restored.error_callback(event())  # G is not selected in this fixture.
        scenario.host_loop_failure.require_alive()
        selected = event(member=w.C3_ID)
        await restored.error_callback(selected)
        private = SimpleNamespace(begin_console_login=Mock(side_effect=AssertionError("unexpected fresh login")))
        callback = object()
        scenario.runtime = SimpleNamespace(host_control=private, _transport=SimpleNamespace(_callback_handler=callback))
        scenario.callback_dispatchers = [(None, None, callback)]
        scenario.probe_attempt = SimpleNamespace(join_listing=AsyncMock())
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            with self.assertRaisesRegex(AssertionError, "typed host_loop_crash"):
                await scenario.run(private, None, directory, [], SimpleNamespace(args=["gateway"], pid=2), "unused")
            report = json.loads((directory / "helperless-restored.json").read_text())
        private.begin_console_login.assert_not_called()
        scenario.probe_attempt.join_listing.assert_awaited_once()
        self.assertIs(scenario.runtime._transport._callback_handler, callback)
        self.assertEqual(report["selected_host_failure"], {
            "category": "host_loop_crash", "member_id": w.C3_ID, "phase": "restored"})
        self.assertEqual(report["status"], "failed")
        self.assertNotIn("cleanup_error", report)


    async def test_close_sqlite_poll_observes_late_selected_failure_before_completed_receipt(self):
        workflow = {"key": {"realm": "mob.fixture", "owner": "workflow-owner", "id": "workflow-id"}}
        effect = {"key": {"record": workflow["key"], "id": "close"}, "intent": {
            "id": "close", "kind": "setup_workflow_close", "request": {
                "schema": "meerkat_toolkit.setup_workflow.close", "version": 1,
                "body": {"workflow": workflow, "payload": workflow}}},
            "status": "Succeeded", "outcome": {"Succeeded": {"receipt": {
                "schema": "toolkit.setup-workflow-close", "version": 1,
                "body": {"state": "source_fenced_and_finalized", "session_id": "G-session", "input_id": "G-original"}}}}}
        for member in (w.G_ID, "another-member"):
            with self.subTest(member=member), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                path = root / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
                path.parent.mkdir(parents=True)
                with sqlite3.connect(path) as db:
                    db.execute("CREATE TABLE toolkit_effects(realm,owner,record_id,id,data)")
                    def store(value):
                        db.execute("DELETE FROM toolkit_effects")
                        db.execute("INSERT INTO toolkit_effects VALUES(?,?,?,?,?)",
                            ("mob.fixture", "workflow-owner", "workflow-id", "close", json.dumps(value)))
                        db.commit()
                    pending = {**copy.deepcopy(effect), "status": "Pending"}
                    store(pending)
                    observer = w.s.SelectedHostLoopFailure(w.G_ID, "restored")
                    observed = {}
                    async def settle(_):
                        self.assertEqual(observed["effect"], pending)
                        store(effect)
                        await observer(event(member=member))
                    with patch.object(w.asyncio, "sleep", side_effect=settle) as waited:
                        call = w.wait_finalized_g_close(root, workflow, "G-session", "G-original",
                            w.time.monotonic() + 1, observed, failure_observer=observer)
                        if member == w.G_ID:
                            with self.assertRaisesRegex(AssertionError, "typed host_loop_crash"):
                                await call
                            self.assertEqual(observed["effect"], pending)
                        else:
                            self.assertEqual(await call, effect)
                            self.assertEqual(observed["effect"], effect)
                    waited.assert_awaited_once_with(0.05)
                    self.assertEqual(json.loads(db.execute("SELECT data FROM toolkit_effects").fetchone()[0]), effect)

    async def condition_case(self, method, *, selected):
        # The real Condition worker signals immediately before its actual park.
        # The callback must release that same worker, not a cancelled surrogate.
        customizer = w.AcceptanceCustomizer()
        entered, returned = threading.Event(), threading.Event()
        class ParkObserved(threading.Condition):
            def wait(self, timeout=None):
                entered.set()
                try:
                    return super().wait(timeout)
                finally:
                    returned.set()
        customizer.created_changed = ParkObserved()
        observer = w.s.SelectedHostLoopFailure(w.G_ID, "restored")
        retired = w.s.SelectedHostLoopFailure(w.G_ID, "prepared")
        context = {"model": "fixture-model", "labels": {}, "system_prompt": "fixture prompt"}
        identity = w.C3_ID if method == "wait_created" else w.G_ID
        deadline = w.time.monotonic() + 30
        async def build():
            draft = SimpleNamespace(model=context["model"], labels={}, system_prompt=context["system_prompt"],
                external_tools=[], register_tool=Mock())
            await customizer.customize_build(SimpleNamespace(identity=identity),
                SimpleNamespace(identity=identity, profile=w.C3_CLASS if identity == w.C3_ID else w.CLASS_ID), draft)
            if identity == w.C3_ID:
                draft.register_tool.assert_called_once()
            else:
                draft.register_tool.assert_not_called()
        await build()
        if method == "wait_respawn":
            await customizer.after_create(identity, "same-session", SimpleNamespace(**context))
            before = customizer.respawn_snapshot(identity)
            await build()
            rpc_returned = customizer.respawn_snapshot(identity)
            call = customizer.wait_respawn(before, rpc_returned, identity, "same-session", deadline,
                failure_observer=observer)
        else:
            call = getattr(customizer, method)(1, "same-session", deadline, failure_observer=observer)
        task = asyncio.create_task(call)
        try:
            self.assertTrue(await asyncio.to_thread(entered.wait, 2))
            self.assertFalse(task.done())
            if selected:
                await observer(event())
                with self.assertRaisesRegex(AssertionError, "typed host_loop_crash"):
                    await asyncio.wait_for(asyncio.shield(task), 2)
            else:
                await observer(event("foreign-member"))
                await retired(event())
                observer.require_alive()
                self.assertFalse(task.done())
                await customizer.after_create(identity, "same-session", SimpleNamespace(**context))
                proof = await asyncio.wait_for(asyncio.shield(task), 2)
                if method == "wait_respawn":
                    self.assertEqual(proof["latest_build"], 2)
                    self.assertEqual(customizer.verify_respawn(proof), proof)
                else:
                    self.assertEqual(proof, [{"identity": identity, "session_id": "same-session", "context": context}])
            self.assertTrue(task.done() and returned.is_set())
            with observer._conditions_lock:
                self.assertEqual(observer._conditions, {})
        finally:
            # Even a failed test assertion keeps the same original worker owned.
            if not task.done():
                await observer(event())
                try:
                    await task
                except AssertionError:
                    pass

    async def test_selected_late_callback_wakes_and_joins_each_original_condition_waiter(self):
        for method in ("wait_respawn", "wait_shared_created", "wait_created"):
            with self.subTest(method=method):
                await self.condition_case(method, selected=True)

    async def test_foreign_and_retired_phase_callbacks_preserve_real_late_creation_waits(self):
        for method in ("wait_respawn", "wait_shared_created", "wait_created"):
            with self.subTest(method=method):
                await self.condition_case(method, selected=False)

    async def test_a_and_departure_sqlite_polls_observe_selected_failure_before_next_read(self):
        for kind in ("a", "departure"):
            for member in (w.G_ID, "foreign-member"):
                with self.subTest(kind=kind, member=member), tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    observer = w.s.SelectedHostLoopFailure(w.G_ID, "restored")
                    if kind == "a":
                        path = root / "state/runtime.sqlite"
                        path.parent.mkdir(parents=True)
                        db = sqlite3.connect(path)
                        db.execute("CREATE TABLE runtime_input_states(runtime_id,input_id,state_json)")
                        pending = {"input_id": "A-input", "terminal_outcome": None, "terminal_completion": None}
                        completed = {**pending, "terminal_outcome": "fixture-outcome", "terminal_completion": "fixture-completion"}
                        def store(value):
                            db.execute("DELETE FROM runtime_input_states")
                            db.execute("INSERT INTO runtime_input_states VALUES(?,?,?)", ("rt:session:A-session", "A-input", json.dumps(value)))
                            db.commit()
                        read_name = "input_rows"
                        call = w.s.wait_initial_a_completion(root, {"session_id": "A-session", "input_id": "A-input"},
                            w.time.monotonic() + 30, failure_observer=observer)
                    else:
                        path = root / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
                        path.parent.mkdir(parents=True)
                        db = sqlite3.connect(path)
                        db.execute("CREATE TABLE toolkit_records(realm,owner,id,data)")
                        pending = {"sealed": False, "waiters": {
                            "A": {"identity": {"source": "A"}, "departed": False},
                            "B": {"identity": {"source": "B"}, "departed": False}}}
                        completed = copy.deepcopy(pending)
                        completed["waiters"]["A"]["departed"] = True
                        def store(value):
                            db.execute("DELETE FROM toolkit_records")
                            db.execute("INSERT INTO toolkit_records VALUES(?,?,?,?)", ("mob.fixture", "owner", "workflow",
                                json.dumps({"schema": "meerkat_toolkit.setup_workflow", "body": value})))
                            db.commit()
                        read_name = "workflow_rows"
                        call = w.s.wait_departed(root, "A", w.time.monotonic() + 30,
                            remaining_source="B", failure_observer=observer)
                    try:
                        store(pending)
                        async def settle(_):
                            store(completed)
                            await observer(event(member))
                        original_read = getattr(w.s, read_name)
                        with patch.object(w.s, read_name, wraps=original_read) as reads, \
                                patch.object(w.s.asyncio, "sleep", side_effect=settle) as paused:
                            if member == w.G_ID:
                                with self.assertRaisesRegex(AssertionError, "typed host_loop_crash"):
                                    await call
                                self.assertEqual(reads.call_count, 1)
                            else:
                                self.assertEqual(await call, [completed] if kind == "a" else completed)
                                self.assertEqual(reads.call_count, 2)
                            paused.assert_awaited_once_with(0.05)
                    finally:
                        db.close()

    async def test_service_observer_checks_after_same_joined_timeout_or_reply_without_resubmit(self):
        for selected in (True, False):
            for timeout_first in (True, False):
                with self.subTest(selected=selected, timeout_first=timeout_first):
                    observer = w.s.SelectedHostLoopFailure(w.G_ID, "live")
                    loop = asyncio.get_running_loop()
                    calls, joined = [], []
                    expected = {"actual": "same retained exchange"}
                    def observe(*, timeout):
                        calls.append(timeout)
                        try:
                            if len(calls) == 1:
                                delivery = asyncio.run_coroutine_threadsafe(observer(event(w.G_ID if selected else "foreign-member")), loop)
                                delivery.result(timeout=2)
                                if timeout_first:
                                    raise TimeoutError("same retained exchange still pending")
                            return expected
                        finally:
                            joined.append(len(calls))
                    provider = SimpleNamespace(lock=threading.Lock(), errors=[])
                    result = w.v.observe_installed_service_reply(observe, provider, SimpleNamespace(poll=lambda: None),
                        w.time.monotonic() + 30, failure_observer=observer)
                    if selected:
                        with self.assertRaisesRegex(AssertionError, "typed host_loop_crash"):
                            await result
                        self.assertEqual(joined, [1])
                    else:
                        self.assertIs(await result, expected)
                        self.assertEqual(joined, [1, 2] if timeout_first else [1])
                    self.assertEqual(len(calls), len(joined))
                    self.assertTrue(all(0 < value <= 0.25 for value in calls))


if __name__ == "__main__":
    unittest.main()
