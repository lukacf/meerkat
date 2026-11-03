"""Selected-install completion stays in its original MCP call.

The real Controller, job, persistence and readback paths use inert package owners.
These controls establish delivery/custody, not real package or native authority.
"""
import asyncio
from contextlib import asynccontextmanager
from concurrent.futures import ThreadPoolExecutor
import copy
from pathlib import Path
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from mcp import Client

from test_selected_setup import SelectedFixture, adapter, backend
from test_selected_mcpb import McpbFixture


class SelectedInstallContinuation(SelectedFixture, unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        super().setUp()
        self.releases = []

    def tearDown(self):
        for release in self.releases:
            release.set()
        super().tearDown()

    async def event(self, event):
        self.assertTrue(await asyncio.to_thread(event.wait, 5), "fixture gate timed out")

    def gated_readback(self, controller):
        entered, release = threading.Event(), threading.Event()
        self.releases.append(release)
        run = controller.runner.run
        def gated(argv, **kwargs):
            if "--verify-existing" in argv or len(argv) > 1 and argv[1] == "verify":
                entered.set()
                if not release.wait(5):
                    raise AssertionError("fixture readback gate timed out")
            return run(argv, **kwargs)
        controller.runner.run = gated
        return entered, release

    @asynccontextmanager
    async def connection(self, controller):
        owner = adapter.SetupAdapter(controller)
        try:
            async with Client(adapter.create_server(owner)) as client:
                yield client, owner
        finally:
            for release in self.releases:
                release.set()
            await owner.aclose()

    def effect_calls(self, controller):
        return [argv for argv, _ in controller.runner.calls
                if "--destination" in argv and "--verify-existing" not in argv and "--package" not in argv]

    async def test_node_ready_receipt_does_not_end_original_mcp_call_before_verified_completion(self):
        c = self.controller()
        request = self.software_stage("node")
        entered, release = self.gated_readback(c)
        async with self.connection(c) as (client, _):
            checked = (await client.call_tool("setup_selected_check", request)).structured_content
            other_checked = (await client.call_tool("setup_selected_check", self.software_stage("node", "other"))).structured_content
            selected = self.selection(checked)
            call = asyncio.create_task(client.call_tool("setup_selected_install", selected))
            replay = None
            try:
                await self.event(entered)
                job = copy.deepcopy(c.job)
                destination = Path(c.install_intents[job["id"]]["destination"])
                self.assertTrue((destination / "ready/installed.json").is_file())
                self.assertEqual(job["status"], "running")
                # This counterexample is independent of the new wait method:
                # the old adapter returns running while actual readback is held.
                done, _ = await asyncio.wait({call}, timeout=0.05)
                if done:
                    self.assertNotEqual((await call).structured_content["job"]["status"], "running")
                self.assertFalse(call.done())
                replay = asyncio.create_task(client.call_tool("setup_selected_install", selected))
                observed = await client.call_tool("setup_job", {"job_id": job["id"]})
                self.assertEqual(observed.structured_content["job"], job)
                state = await client.call_tool("setup_state", {})
                self.assertEqual(state.structured_content["job"], job)
                self.assertFalse(replay.done())
                refused = await client.call_tool("setup_selected_install", self.selection(other_checked))
                self.assertTrue(refused.is_error)
                self.assertEqual(refused.structured_content["error"], "operation_in_progress")
                self.assertEqual(len(c.jobs), 1)
                release.set()
                first, repeated = await asyncio.wait_for(asyncio.gather(call, replay), 5)
                self.assertFalse(first.is_error, first)
                self.assertEqual(first.structured_content, repeated.structured_content)
                final = first.structured_content["job"]
                self.assertEqual(final["id"], job["id"])
                self.assertEqual(final["status"], "succeeded")
                self.assertEqual(final["result"]["scope"], "observed_software_installation_only")
                self.assertEqual(final["result"]["effect_sha256"], selected["effect_sha256"])
                self.assertEqual(final["result"]["receipt_sha256"], backend.digest((destination / "ready/installed.json").read_bytes()))
                self.assertFalse(c.operation_lock.locked())
                self.assertFalse(c._job_threads[job["id"]].is_alive())
                self.assertEqual(len(self.effect_calls(c)), 1)
                self.assertEqual(len(c.jobs), 1)
                calls = copy.deepcopy(c.runner.calls)
                again = await client.call_tool("setup_selected_install", selected)
                self.assertEqual(again.structured_content["job"], final)
                self.assertEqual(c.runner.calls, calls)
            finally:
                release.set()
                await asyncio.gather(*[t for t in (call, replay) if t is not None], return_exceptions=True)

    async def test_many_exact_replays_share_join_and_do_not_starve_explicit_stop(self):
        # The shipped asyncio default pool has at least five slots. Two slots
        # make a blocking wait per replay exhaust the pool with only two calls.
        loop = asyncio.get_running_loop()
        loop.set_default_executor(ThreadPoolExecutor(max_workers=2))
        c = self.controller()
        checked = c.selected_check(self.stage("node"))
        entered, release = self.gated_readback(c)
        admitted, count = asyncio.Event(), 0
        dispatch = c.dispatch
        def counted(route, arguments):
            nonlocal count
            result = dispatch(route, arguments)
            if route == "/api/selected_install":
                count += 1
                if count == 6:
                    loop.call_soon_threadsafe(admitted.set)
            return result
        c.dispatch = counted
        calls = []
        async with self.connection(c) as (client, owner):
            try:
                calls.append(asyncio.create_task(client.call_tool("setup_selected_install", self.selection(checked))))
                await self.event(entered)
                calls.extend(asyncio.create_task(client.call_tool("setup_selected_install", self.selection(checked)))
                             for _ in range(5))
                await asyncio.wait_for(admitted.wait(), 3)
                self.assertEqual(len(owner._selected_install_waits), 1)
                stopped = await asyncio.wait_for(client.call_tool("setup_stop", {"job_id": c.job["id"]}), 3)
                self.assertEqual(stopped.structured_content["job"]["stop"]["status"], "requested")
                self.assertTrue(all(not call.done() for call in calls))
                release.set()
                results = await asyncio.wait_for(asyncio.gather(*calls), 5)
                self.assertTrue(all(result.structured_content == results[0].structured_content for result in results))
                self.assertEqual(results[0].structured_content["job"]["error"], "operation_stopped")
                self.assertEqual(len(self.effect_calls(c)), 1)
                self.assertFalse(c.operation_lock.locked())
            finally:
                release.set()
                await asyncio.gather(*calls, return_exceptions=True)

    async def test_python_and_mcpb_share_original_call_settlement(self):
        for profile in ("python", "mcpb"):
            with self.subTest(profile=profile):
                case = self
                if profile == "mcpb":
                    case = McpbFixture()
                    case.setUp()
                    self.addCleanup(case.tearDown)
                c = case.controller()
                checked = c.selected_check(case.stage(profile))
                entered, release = self.gated_readback(c)
                async with self.connection(c) as (client, _):
                    call = asyncio.create_task(client.call_tool("setup_selected_install", case.selection(checked)))
                    try:
                        await self.event(entered)
                        done, _ = await asyncio.wait({call}, timeout=0.05)
                        self.assertFalse(done)
                        release.set()
                        result = await asyncio.wait_for(call, 5)
                        self.assertFalse(result.is_error, result)
                        self.assertEqual(result.structured_content["job"]["status"], "succeeded")
                        self.assertFalse(c.operation_lock.locked())
                    finally:
                        release.set()
                        await asyncio.gather(call, return_exceptions=True)

    async def test_terminal_row_waits_for_worker_cleanup(self):
        c = self.controller()
        checked = c.selected_check(self.stage("node"))
        entered, finish = threading.Event(), threading.Event()
        self.releases.append(finish)
        lock = c.operation_lock
        def release():
            entered.set()
            if not finish.wait(5):
                raise AssertionError("fixture cleanup gate timed out")
            lock.release()
        c.operation_lock = SimpleNamespace(acquire=lock.acquire, release=release, locked=lock.locked)
        async with self.connection(c) as (client, _):
            call = asyncio.create_task(client.call_tool("setup_selected_install", self.selection(checked)))
            try:
                await self.event(entered)
                self.assertEqual(c.job["status"], "succeeded")
                self.assertTrue(lock.locked())
                self.assertFalse(call.done())
                observed = await client.call_tool("setup_job", {"job_id": c.job["id"]})
                self.assertEqual(observed.structured_content["job"]["status"], "succeeded")
                finish.set()
                self.assertEqual((await call).structured_content["job"]["status"], "succeeded")
                self.assertFalse(lock.locked())
            finally:
                finish.set()
                await asyncio.gather(call, return_exceptions=True)

    async def test_install_and_readback_failure_remain_unconfirmed_and_do_not_retry(self):
        for kind in ("installer", "readback"):
            with self.subTest(kind=kind):
                c = self.controller()
                checked = c.selected_check(self.stage("node", kind))
                if kind == "installer":
                    c.runner.nonzero = True
                else:
                    c.runner.bad_proof = {"verified": False}
                async with self.connection(c) as (client, _):
                    selected = self.selection(checked)
                    job = (await client.call_tool("setup_selected_install", selected)).structured_content["job"]
                    self.assertEqual(job["status"], "unconfirmed")
                    self.assertEqual(job["error"], "operation_unconfirmed")
                    self.assertNotIn("result", job)
                    self.assertFalse(c.operation_lock.locked())
                    replay = await client.call_tool("setup_selected_install", selected)
                    self.assertEqual(replay.structured_content["job"], job)
                    self.assertEqual(len(self.effect_calls(c)), 1)

    async def test_explicit_stop_keeps_reads_responsive_and_returns_settled_uncertainty(self):
        c = self.controller()
        checked = c.selected_check(self.stage("node"))
        entered, release = self.gated_readback(c)
        async with self.connection(c) as (client, _):
            call = asyncio.create_task(client.call_tool("setup_selected_install", self.selection(checked)))
            try:
                await self.event(entered)
                stopped = await client.call_tool("setup_stop", {"job_id": c.job["id"]})
                self.assertEqual(stopped.structured_content["job"]["stop"]["status"], "requested")
                self.assertFalse(call.done())
                release.set()
                job = (await call).structured_content["job"]
                self.assertEqual(job["status"], "unconfirmed")
                self.assertEqual(job["error"], "operation_stopped")
                self.assertEqual(job["stop"]["status"], "settled")
                self.assertNotIn("result", job)
                self.assertFalse(c.operation_lock.locked())
                self.assertEqual(len(self.effect_calls(c)), 1)
            finally:
                release.set()
                await asyncio.gather(call, return_exceptions=True)

    async def test_cancelled_caller_joins_but_never_receives_late_success(self):
        c = self.controller()
        checked = c.selected_check(self.stage("node"))
        entered, release = self.gated_readback(c)
        owner = adapter.SetupAdapter(c)
        call = asyncio.create_task(owner.dispatch("/api/selected_install", self.selection(checked)))
        try:
            await self.event(entered)
            call.cancel()
            await asyncio.sleep(0)
            self.assertFalse(call.done())
            call.cancel()
            self.assertNotIn("stop", c.job)
            release.set()
            with self.assertRaises(asyncio.CancelledError):
                await call
            self.assertEqual(c.job["status"], "succeeded")
            self.assertFalse(c.operation_lock.locked())
            replay = await owner.dispatch("/api/selected_install", self.selection(checked))
            self.assertEqual(replay["job"], c.job)
            self.assertEqual(len(self.effect_calls(c)), 1)
        finally:
            release.set()
            await asyncio.gather(call, return_exceptions=True)
            await owner.aclose()

    async def test_caller_deadline_stays_timeout_after_joined_settlement(self):
        c = self.controller()
        checked = c.selected_check(self.stage("node"))
        entered, release = self.gated_readback(c)
        owner = adapter.SetupAdapter(c)
        deadline = asyncio.timeout(None)
        async def bounded_call():
            async with deadline:
                return await owner.dispatch("/api/selected_install", self.selection(checked))
        call = asyncio.create_task(bounded_call())
        try:
            await self.event(entered)
            deadline.reschedule(asyncio.get_running_loop().time())
            await asyncio.sleep(0)
            await asyncio.sleep(0)
            self.assertTrue(deadline.expired())
            self.assertFalse(call.done())
            self.assertNotIn("stop", c.job)
            release.set()
            with self.assertRaises(TimeoutError):
                await call
            self.assertEqual(c.job["status"], "succeeded")
            self.assertFalse(c.operation_lock.locked())
            self.assertEqual(len(self.effect_calls(c)), 1)
        finally:
            release.set()
            await asyncio.gather(call, return_exceptions=True)
            await owner.aclose()

    async def test_close_joins_admitted_effect_and_refuses_new_dispatch(self):
        c = self.controller()
        checked = c.selected_check(self.stage("node"))
        entered, release = self.gated_readback(c)
        owner = adapter.SetupAdapter(c)
        call = asyncio.create_task(owner.dispatch("/api/selected_install", self.selection(checked)))
        closing = None
        try:
            await self.event(entered)
            closing = asyncio.create_task(owner.aclose())
            await asyncio.sleep(0)
            self.assertTrue(c.closing)
            self.assertFalse(closing.done())
            with self.assertRaisesRegex(backend.SetupError, "host_closing"):
                await owner.dispatch("/api/state", {})
            release.set()
            self.assertEqual((await call)["job"]["status"], "succeeded")
            await closing
            self.assertFalse(c.operation_lock.locked())
            self.assertTrue(all(not thread.is_alive() for thread in c.threads))
        finally:
            release.set()
            await asyncio.gather(*[t for t in (call, closing) if t is not None], return_exceptions=True)
            await owner.aclose()

    async def test_start_failures_preserve_one_unconfirmed_job_without_effect(self):
        start = threading.Thread.start
        for after_start in (False, True):
            with self.subTest(after_start=after_start):
                c = self.controller()
                checked = c.selected_check(self.stage("node", str(after_start)))
                def fail(thread):
                    if thread.name != "consumer-setup-operation":
                        return start(thread)
                    if after_start:
                        start(thread)
                    raise RuntimeError("private fixture start failure")
                async with self.connection(c) as (client, _):
                    with patch.object(threading.Thread, "start", fail):
                        job = (await client.call_tool("setup_selected_install", self.selection(checked))).structured_content["job"]
                    self.assertEqual(job["status"], "unconfirmed")
                    self.assertEqual(job["error"], "operation_unconfirmed")
                    self.assertEqual(self.effect_calls(c), [])
                    self.assertFalse(c.operation_lock.locked())
                    self.assertTrue(all(not thread.is_alive() for thread in c.threads))

    async def test_failed_checkpoint_never_starts_worker_or_fabricates_result(self):
        c = self.controller()
        checked = c.selected_check(self.stage("node"))
        def refuse(_):
            raise OSError("private fixture retention failure")
        c.recovery_selection_writer = refuse
        owner = adapter.SetupAdapter(c)
        try:
            job = (await owner.dispatch("/api/selected_install", self.selection(checked)))["job"]
            self.assertEqual(job["status"], "unconfirmed")
            self.assertEqual(job["error"], "recovery_selection_unavailable")
            self.assertEqual(c._job_threads, {})
            self.assertEqual(self.effect_calls(c), [])
            self.assertFalse(c.operation_lock.locked())
        finally:
            await owner.aclose()

    async def test_direct_admission_stays_async_and_wait_is_exact_owner_and_selected_kind(self):
        c, other = self.controller(), self.controller()
        checked = c.selected_check(self.stage("node"))
        entered, release = self.gated_readback(c)
        job = c.dispatch("/api/selected_install", self.selection(checked))["job"]
        self.assertEqual(job["status"], "running")
        await self.event(entered)
        with self.assertRaisesRegex(backend.SetupError, "unknown_selected_install_job"):
            other.wait_selected_install(job["id"])
        self.assertEqual(other.jobs, {})
        with self.assertRaisesRegex(backend.SetupError, "selected_install_worker_unavailable"):
            thread = c._job_threads.pop(job["id"])
            try:
                c.wait_selected_install(job["id"])
            finally:
                c._job_threads[job["id"]] = thread
        release.set()
        final = await asyncio.to_thread(c.wait_selected_install, job["id"])
        self.assertEqual(final["job"]["status"], "succeeded")
        intent = c.install_intents.pop(job["id"])
        try:
            with self.assertRaisesRegex(backend.SetupError, "unknown_selected_install_job"):
                c.wait_selected_install(job["id"])
        finally:
            c.install_intents[job["id"]] = intent
        self.assertEqual(len(self.effect_calls(c)), 1)

    async def test_recovered_row_stays_historical_even_after_matching_readback(self):
        c, _, job = self.installed("node")
        recovered = self.recover(c, job)
        before = recovered.job_detail({"job_id": job["id"]})["job"]
        self.assertEqual(before["status"], "unconfirmed")
        self.assertEqual(before["error"], "historical_outcome_unknown")
        self.assertEqual(recovered.wait_selected_install(job["id"])["job"], before)
        self.assertEqual(recovered.reconcile({"job_id": job["id"]})["observation"]["status"], "matched")
        self.assertEqual(recovered.wait_selected_install(job["id"])["job"], before)
        self.assertEqual(recovered._job_threads, {})


if __name__ == "__main__":
    unittest.main()
