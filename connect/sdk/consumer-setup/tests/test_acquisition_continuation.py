"""Acquisition completion stays in the original MCP call, with inert I/O only."""
import asyncio
from contextlib import asynccontextmanager
import copy
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from mcp import Client

import test_acquisition_recovery as recovery_tests
from test_node_acquisition_wiring import NodeFixture
from test_selected_setup import adapter, backend


class AcquisitionContinuation(NodeFixture, unittest.IsolatedAsyncioTestCase):
    async def event(self, event):
        self.assertTrue(await asyncio.to_thread(event.wait, 5), "fixture gate timed out")

    @asynccontextmanager
    async def call(self, controller, name="setup_acquire_node", request=None):
        owner = adapter.SetupAdapter(controller)
        call = None
        try:
            async with Client(adapter.create_server(owner)) as client:
                call = asyncio.create_task(client.call_tool(name, request or self.request))
                try:
                    yield client, owner, call
                finally:
                    for kind in ("node", "python", "mcpb", "compose"):
                        release = getattr(getattr(controller, kind + "_acquisition"), "release", None)
                        if release is not None:
                            release.set()
                    if call is not None:
                        await asyncio.gather(call, return_exceptions=True)
        finally:
            await owner.aclose()

    def joining(self, controller):
        entered = threading.Event()
        wait = controller.wait_acquisition
        def joined(identifier):
            entered.set()
            return wait(identifier)
        controller.wait_acquisition = joined
        return entered

    async def test_node_actual_owner_31_second_completion_returns_in_original_mcp_call(self):
        # The real Node owner checks a virtual monotonic clock. Inert I/O is
        # held at its first probe; completion at t=31 exceeds the observed
        # helper's former t=24 polling budget without spending 31 real seconds.
        case, controller, acquisition = recovery_tests.AcquisitionRecoveryTests.package_controller(self, "node")
        entered, release = threading.Event(), threading.Event()
        acquisition.release = release
        clock = SimpleNamespace(now=0)
        clock.monotonic = lambda: clock.now
        runner = case.runner
        def delayed_runner(argv, **kwargs):
            if not entered.is_set():
                entered.set()
                if not release.wait(5):
                    raise AssertionError("fixture gate timed out")
                clock.now = 31
            return runner(argv, **kwargs)
        case.runner = delayed_runner
        with patch.dict(type(acquisition).run.__globals__, {"time": clock}):
            async with self.call(controller, request=case.request) as (client, _, call):
                await self.event(entered)
                # Let protocol dispatch finish if it is still the old early
                # running-job reply. This bound observes only the test call;
                # it neither cancels it nor changes the acquisition deadline.
                done, _ = await asyncio.wait({call}, timeout=0.05)
                if done:
                    self.assertNotEqual((await call).structured_content["job"]["status"], "running")
                self.assertFalse(call.done())
                self.assertTrue(controller.operation_lock.locked())
                self.assertEqual(controller.job["status"], "running")
                release.set()
                result = await asyncio.wait_for(call, 5)
                self.assertFalse(result.is_error, result)
                job = result.structured_content["job"]
                self.assertEqual(job["status"], "succeeded")
                self.assertEqual(clock.now, 31)
                self.assertEqual(job["result"]["request"], case.request)
                self.assertEqual(job["result"]["staged_id"], job["id"])
                self.assertFalse(controller.operation_lock.locked())
                self.assertFalse(controller._job_threads[job["id"]].is_alive())
                calls = copy.deepcopy(case.calls)
                replay = await client.call_tool("setup_acquire_node", case.request)
                self.assertEqual(replay.structured_content["job"], job)
                self.assertEqual(case.calls, calls)
                self.assertEqual(len(controller.jobs), 1)
                checked = await client.call_tool("setup_reconcile", {"job_id": job["id"]})
                self.assertEqual(checked.structured_content["observation"]["status"], "matched")
                self.assertEqual(case.calls, calls, "readback must reuse retained files")

    async def test_held_call_replay_and_reads_keep_exact_owner_and_job(self):
        controller = self.enabled()
        other = self.enabled()
        joining = self.joining(controller)
        async with self.call(controller) as (client, _, call):
            await self.event(joining)
            await self.event(controller.node_acquisition.entered)
            job = copy.deepcopy(controller.job)
            replay = asyncio.create_task(client.call_tool("setup_acquire_node", self.request))
            try:
                for _ in range(11):
                    observed = await client.call_tool("setup_job", {"job_id": job["id"]})
                    self.assertEqual(observed.structured_content["job"], job)
                state = await client.call_tool("setup_state", {})
                self.assertEqual(state.structured_content["job"], job)
                self.assertFalse(call.done())
                self.assertFalse(replay.done())
                self.assertEqual(len(controller.node_acquisition.calls), 1)
                for request in (dict(self.request, version="2.0.0"), dict(self.request, request_id="c" * 32)):
                    denied = await client.call_tool("setup_acquire_node", request)
                    self.assertTrue(denied.is_error)
                denied = await client.call_tool("setup_acquire_python", self.python_request)
                self.assertEqual(denied.structured_content["error"], "acquisition_request_changed")
                with self.assertRaisesRegex(backend.SetupError, "unknown_acquisition_job"):
                    other.wait_acquisition(job["id"])
                self.assertEqual(other.jobs, {})
                controller.node_acquisition.release.set()
                first, repeated = await asyncio.gather(call, replay)
                self.assertEqual(first.structured_content, repeated.structured_content)
                self.assertEqual(first.structured_content["job"]["status"], "succeeded")
                self.assertEqual(len(controller.node_acquisition.calls), 1)
            finally:
                controller.node_acquisition.release.set()
                await asyncio.gather(replay, return_exceptions=True)

    async def test_explicit_stop_is_responsive_and_settles_original_call(self):
        controller = self.enabled()
        joining = self.joining(controller)
        async with self.call(controller) as (client, _, call):
            await self.event(joining)
            await self.event(controller.node_acquisition.entered)
            identifier = controller.job["id"]
            stopped = await client.call_tool("setup_stop", {"job_id": identifier})
            self.assertEqual(stopped.structured_content["job"]["stop"]["status"], "requested")
            result = await asyncio.wait_for(call, 5)
            job = result.structured_content["job"]
            self.assertEqual(job["id"], identifier)
            self.assertEqual(job["status"], "unconfirmed")
            self.assertEqual(job["error"], "operation_stopped")
            self.assertEqual(job["stop"]["status"], "settled")
            self.assertNotIn("result", job)
            self.assertFalse(controller.operation_lock.locked())
            replay = await client.call_tool("setup_acquire_node", self.request)
            self.assertEqual(replay.structured_content["job"], job)
            self.assertEqual(len(controller.node_acquisition.calls), 1)

    async def test_terminal_owner_failure_returns_without_fabricated_success(self):
        controller = self.enabled()
        controller.node_acquisition.fail = True
        joining = self.joining(controller)
        async with self.call(controller) as (_, _, call):
            await self.event(joining)
            await self.event(controller.node_acquisition.entered)
            self.assertFalse(call.done())
            controller.node_acquisition.release.set()
            job = (await call).structured_content["job"]
            self.assertEqual(job["status"], "unconfirmed")
            self.assertEqual(job["error"], "acquisition_fixture_refused")
            self.assertNotIn("result", job)
            self.assertFalse(controller.operation_lock.locked())

    async def test_actual_node_owner_deadline_returns_unconfirmed_in_original_call(self):
        case, controller, acquisition = recovery_tests.AcquisitionRecoveryTests.package_controller(self, "node")
        clock = SimpleNamespace(now=0)
        clock.monotonic = lambda: clock.now
        runner = case.runner
        def timed_out(argv, **kwargs):
            result = runner(argv, **kwargs)
            clock.now = 121
            return result
        case.runner = timed_out
        with patch.dict(type(acquisition).run.__globals__, {"time": clock}):
            async with self.call(controller, request=case.request) as (client, _, call):
                job = (await asyncio.wait_for(call, 5)).structured_content["job"]
                self.assertEqual(job["status"], "unconfirmed")
                self.assertEqual(job["error"], "node_acquisition_timeout")
                self.assertNotIn("result", job)
                self.assertFalse((case.staging / job["id"] / "acquisition.json").exists())
                calls = copy.deepcopy(case.calls)
                replay = await client.call_tool("setup_acquire_node", case.request)
                self.assertEqual(replay.structured_content["job"], job)
                self.assertEqual(case.calls, calls)

    async def test_cancelled_adapter_call_joins_worker_then_rethrows_cancellation(self):
        controller = self.enabled()
        joining = self.joining(controller)
        owner = adapter.SetupAdapter(controller)
        call = asyncio.create_task(owner.dispatch("/api/acquire_node", self.request))
        try:
            await self.event(joining)
            await self.event(controller.node_acquisition.entered)
            call.cancel()
            await asyncio.sleep(0)
            self.assertFalse(call.done())
            call.cancel()
            self.assertNotIn("stop", controller.job)
            controller.node_acquisition.release.set()
            with self.assertRaises(asyncio.CancelledError):
                await call
            self.assertFalse(controller.operation_lock.locked())
            self.assertEqual(controller.job["status"], "succeeded")
            replay = await owner.dispatch("/api/acquire_node", self.request)
            self.assertEqual(replay["job"], controller.job)
            self.assertEqual(len(controller.node_acquisition.calls), 1)
        finally:
            controller.node_acquisition.release.set()
            await asyncio.gather(call, return_exceptions=True)
            await owner.aclose()

    async def test_sdk_cancellation_keeps_same_job_readable_without_reacquisition(self):
        controller = self.enabled()
        joining = self.joining(controller)
        async with self.call(controller) as (client, owner, call):
            await self.event(joining)
            await self.event(controller.node_acquisition.entered)
            call.cancel()
            await asyncio.sleep(0)
            self.assertNotIn("stop", controller.job)
            controller.node_acquisition.release.set()
            with self.assertRaises(asyncio.CancelledError):
                await call
            replay = await client.call_tool("setup_acquire_node", self.request)
            self.assertEqual(replay.structured_content["job"]["status"], "succeeded")
            self.assertEqual(len(controller.node_acquisition.calls), 1)

    async def test_close_joins_entered_acquisition_and_refuses_new_dispatch(self):
        controller = self.enabled()
        joining = self.joining(controller)
        owner = adapter.SetupAdapter(controller)
        call = asyncio.create_task(owner.dispatch("/api/acquire_node", self.request))
        closing = None
        try:
            await self.event(joining)
            closing = asyncio.create_task(owner.aclose())
            await asyncio.sleep(0)
            self.assertFalse(closing.done())
            self.assertTrue(controller.closing)
            with self.assertRaisesRegex(backend.SetupError, "host_closing"):
                await owner.dispatch("/api/acquire_node", self.request)
            controller.node_acquisition.release.set()
            self.assertEqual((await call)["job"]["status"], "succeeded")
            await closing
            self.assertFalse(controller.operation_lock.locked())
            self.assertTrue(all(not worker.is_alive() for worker in controller.threads))
        finally:
            controller.node_acquisition.release.set()
            await asyncio.gather(call, return_exceptions=True)
            await owner.aclose()

    async def test_thread_start_failure_before_and_after_actual_start_never_waits_forever(self):
        start = threading.Thread.start
        for after in (False, True):
            with self.subTest(after_actual_start=after):
                controller = self.enabled()
                def failed_start(thread):
                    if thread.name != "consumer-setup-operation":
                        return start(thread)
                    if after:
                        start(thread)
                    raise RuntimeError("private thread-start diagnostic")
                with patch.object(threading.Thread, "start", failed_start):
                    async with self.call(controller) as (_, _, call):
                        job = (await asyncio.wait_for(call, 5)).structured_content["job"]
                        self.assertEqual(job["status"], "unconfirmed")
                        self.assertEqual(job["error"], "operation_unconfirmed")
                        self.assertEqual(controller.node_acquisition.calls, [])
                        self.assertFalse(controller.operation_lock.locked())
                        self.assertEqual(job["id"] in controller._job_threads, after)
                        self.assertTrue(all(not thread.is_alive() for thread in controller.threads))

    async def test_failed_recovery_checkpoint_returns_unconfirmed_without_a_worker(self):
        case, controller, _ = recovery_tests.AcquisitionRecoveryTests.package_controller(self, "node")
        def refuse(_):
            raise OSError("private retention failure")
        controller.recovery_selection_writer = refuse
        async with self.call(controller, request=case.request) as (_, _, call):
            job = (await asyncio.wait_for(call, 5)).structured_content["job"]
            self.assertEqual(job["status"], "unconfirmed")
            self.assertEqual(job["error"], "recovery_selection_unavailable")
            self.assertEqual(controller._job_threads, {})
            self.assertEqual(case.calls, [])
            self.assertFalse(controller.operation_lock.locked())
            self.assertTrue(controller.closing)

    async def test_direct_controller_admission_stays_asynchronous(self):
        controller = self.enabled()
        job = controller.dispatch("/api/acquire_node", self.request)["job"]
        self.assertEqual(job["status"], "running")
        await self.event(controller.node_acquisition.entered)
        self.assertFalse(controller.node_acquisition.release.is_set())
        self.assertTrue(controller.operation_lock.locked())
        controller.node_acquisition.release.set()
        await asyncio.to_thread(controller.join)
        self.assertEqual(controller.job_detail({"job_id": job["id"]})["job"]["status"], "succeeded")

    async def test_recovered_row_without_worker_stays_historical_after_matched_readback(self):
        case, controller, _ = recovery_tests.AcquisitionRecoveryTests.package_controller(self, "node")
        owner = adapter.SetupAdapter(controller)
        try:
            original = (await owner.dispatch("/api/acquire_node", case.request))["job"]
        finally:
            await owner.aclose()
        recovered = recovery_tests.AcquisitionRecoveryTests.recovered(self, controller)
        self.assertEqual(recovered._job_threads, {})
        before = recovered.job_detail({"job_id": original["id"]})["job"]
        self.assertEqual(before["status"], "unconfirmed")
        self.assertEqual(before["error"], "historical_outcome_unknown")
        self.assertEqual(recovered.wait_acquisition(original["id"])["job"], before)
        observation = recovered.reconcile({"job_id": original["id"]})["observation"]
        self.assertEqual(observation["status"], "matched")
        self.assertEqual(recovered.wait_acquisition(original["id"])["job"], before)
        with self.assertRaisesRegex(backend.SetupError, "recovery_read_only"):
            recovered.acquire_node(case.request)

    async def test_terminal_row_does_not_return_before_worker_releases_operation_custody(self):
        controller = self.enabled()
        release_entered, release_finish = threading.Event(), threading.Event()
        operation_lock = controller.operation_lock
        def release():
            release_entered.set()
            if not release_finish.wait(5):
                raise AssertionError("fixture release gate timed out")
            operation_lock.release()
        controller.operation_lock = SimpleNamespace(acquire=operation_lock.acquire,
            release=release, locked=operation_lock.locked)
        try:
            async with self.call(controller) as (client, _, call):
                await self.event(controller.node_acquisition.entered)
                controller.node_acquisition.release.set()
                await self.event(release_entered)
                self.assertEqual(controller.job["status"], "succeeded")
                self.assertTrue(operation_lock.locked())
                self.assertFalse(call.done())
                observed = await client.call_tool("setup_job", {"job_id": controller.job["id"]})
                self.assertEqual(observed.structured_content["job"]["status"], "succeeded")
                release_finish.set()
                self.assertEqual((await call).structured_content["job"]["status"], "succeeded")
                self.assertFalse(operation_lock.locked())
        finally:
            release_finish.set()

    async def test_python_and_mcpb_share_wait_but_compose_keeps_long_job_contract(self):
        for kind in ("python", "mcpb", "compose"):
            with self.subTest(kind=kind):
                controller = self.enabled()
                controller.runner.run_compose = controller.runner.run
                acquisition = controller.node_acquisition
                setattr(controller, kind + "_acquisition", acquisition)
                acquisition.run = lambda *args: (acquisition.entered.set(), acquisition.release.wait(5), {"fixture": True})[-1]
                joining = self.joining(controller)
                owner = adapter.SetupAdapter(controller)
                call = asyncio.create_task(owner.dispatch("/api/acquire_" + kind, self.request))
                try:
                    await self.event(acquisition.entered)
                    if kind == "compose":
                        job = (await asyncio.wait_for(call, 5))["job"]
                        self.assertEqual(job["status"], "running")
                        self.assertFalse(joining.is_set())
                    else:
                        await self.event(joining)
                        self.assertFalse(call.done())
                        acquisition.release.set()
                        self.assertEqual((await call)["job"]["status"], "succeeded")
                finally:
                    acquisition.release.set()
                    await asyncio.gather(call, return_exceptions=True)
                    await owner.aclose()


if __name__ == "__main__":
    unittest.main()
