"""Bounded observation of real Controller jobs, without worker ownership.

The projection-refusal control exercises repeated current-owner checks. It does
not grant another caller access to a job.
"""
import asyncio
import copy
import threading
import time
import unittest
from unittest.mock import patch

from test_stop_job import StopFixture, adapter, backend


class JobObservationWaitTests(StopFixture, unittest.IsolatedAsyncioTestCase):
    async def observed(self, predicate):
        async def wait():
            while not predicate():
                await asyncio.sleep(0)
        await asyncio.wait_for(wait(), 2)

    def running(self, controller):
        entered, release = self.gate(), self.gate()

        def worker():
            entered.set()
            self.assertTrue(release.wait(5))
            return {"fixture": "completed"}

        job = self.start(controller, worker)
        self.assertTrue(entered.wait(2))
        return job, release

    async def test_schema_default_and_zero_keep_exact_immediate_snapshot(self):
        controller = self.controller()
        owner = adapter.SetupAdapter(controller)
        job, release = self.running(controller)
        try:
            definition = next(item for item in adapter.descriptions(controller.families)
                              if item["name"] == "setup_job")
            self.assertEqual(definition["inputSchema"]["required"], ["job_id"])
            self.assertFalse(definition["inputSchema"]["additionalProperties"])
            self.assertEqual(definition["inputSchema"]["properties"]["wait_ms"],
                             {"type": "integer", "minimum": 0, "maximum": 10000})
            self.assertTrue(definition["read_only"])
            self.assertTrue(definition["idempotent"])
            expected = backend.encoded(controller.job_detail({"job_id": job["id"]}))
            for arguments in ({"job_id": job["id"]}, {"job_id": job["id"], "wait_ms": 0}):
                result = await asyncio.wait_for(owner.dispatch("/api/job", arguments), 1)
                self.assertEqual(backend.encoded(result), expected)
            self.assertEqual(controller.job["status"], "running")
        finally:
            release.set()
            await owner.aclose()

    async def test_invalid_wait_arguments_refuse_before_owner_read(self):
        controller = self.controller()
        owner = adapter.SetupAdapter(controller)
        with patch.object(controller, "dispatch", wraps=controller.dispatch) as dispatch:
            for invalid in (True, False, -1, 10001, 1.0, "1", None, [], {}):
                with self.subTest(invalid=invalid), self.assertRaises(backend.SetupError):
                    await owner.dispatch("/api/job", {"job_id": "a" * 32, "wait_ms": invalid})
            for arguments in ({"wait_ms": 1}, {"job_id": "a" * 32, "wait_ms": 1, "retry": True}):
                with self.assertRaises(backend.SetupError):
                    await owner.dispatch("/api/job", arguments)
            dispatch.assert_not_called()
        await owner.aclose()

    async def test_running_job_wakes_on_terminal_without_reconcile_or_effect_replay(self):
        controller = self.controller()
        owner = adapter.SetupAdapter(controller)
        job, release = self.running(controller)
        observed = threading.Event()
        original = controller.dispatch
        routes = []

        def read(route, arguments):
            routes.append((route, copy.deepcopy(arguments)))
            value = original(route, arguments)
            observed.set()
            return value

        try:
            with patch.object(controller, "dispatch", side_effect=read):
                waiting = asyncio.create_task(owner.dispatch("/api/job", {"job_id": job["id"], "wait_ms": 10000}))
                self.assertTrue(await asyncio.to_thread(observed.wait, 2))
                self.assertFalse(waiting.done())
                release.set()
                result = await asyncio.wait_for(waiting, 2)
            self.assertEqual(result["job"]["status"], "succeeded")
            self.assertEqual(result["job"]["result"], {"fixture": "completed"})
            self.assertIsNone(result["reconciliation"]["observation"])
            self.assertTrue(all(row == ("/api/job", {"job_id": job["id"]}) for row in routes))
            self.assertEqual(len(controller.jobs), 1)
        finally:
            release.set()
            await owner.aclose()

    async def test_timeout_returns_running_snapshot_and_leaves_worker_untouched(self):
        controller = self.controller()
        owner = adapter.SetupAdapter(controller)
        job, release = self.running(controller)
        before = backend.encoded(controller.job_detail({"job_id": job["id"]}))
        try:
            start = time.monotonic()
            value = await asyncio.wait_for(owner.dispatch("/api/job", {"job_id": job["id"], "wait_ms": 40}), 2)
            self.assertGreaterEqual(time.monotonic() - start, 0.035)
            self.assertEqual(backend.encoded(value), before)
            self.assertTrue(controller._job_threads[job["id"]].is_alive())
            self.assertFalse(owner._job_reads or owner._inflight)
            self.assertNotIn("stop", controller.job)
        finally:
            release.set()
            await owner.aclose()

    async def test_each_observation_rechecks_projected_access_and_foreign_job_refuses(self):
        controller = self.controller()
        owner = adapter.SetupAdapter(controller)
        other = adapter.SetupAdapter(self.controller())
        job, release = self.running(controller)
        seen = threading.Event()
        allowed = threading.Event()
        allowed.set()
        original = controller.dispatch
        calls = []

        def projected(route, arguments):
            calls.append((route, copy.deepcopy(arguments)))
            if not allowed.is_set():
                raise backend.SetupError("unknown_job", 404)
            result = original(route, arguments)
            seen.set()
            return result

        try:
            with self.assertRaises(backend.SetupError) as foreign:
                await other.dispatch("/api/job", {"job_id": job["id"], "wait_ms": 10000})
            self.assertEqual(foreign.exception.code, "unknown_job")
            with patch.object(controller, "dispatch", side_effect=projected):
                waiting = asyncio.create_task(owner.dispatch("/api/job", {"job_id": job["id"], "wait_ms": 10000}))
                self.assertTrue(await asyncio.to_thread(seen.wait, 2))
                allowed.clear()
                with self.assertRaises(backend.SetupError) as retired:
                    await asyncio.wait_for(waiting, 2)
                self.assertEqual(retired.exception.code, "unknown_job")
            self.assertGreaterEqual(len(calls), 2)
            self.assertEqual(controller.job["status"], "running")
            self.assertNotIn("stop", controller.job)
        finally:
            release.set()
            await owner.aclose()
            await other.aclose()

    async def test_cancellation_detaches_one_observer_and_stop_remains_responsive(self):
        controller = self.controller()
        owner = adapter.SetupAdapter(controller)
        job, release = self.running(controller)
        first = asyncio.create_task(owner.dispatch("/api/job", {"job_id": job["id"], "wait_ms": 10000}))
        second = asyncio.create_task(owner.dispatch("/api/job", {"job_id": job["id"], "wait_ms": 10000}))
        try:
            await self.observed(lambda: len(owner._job_reads) == 2)
            first.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await asyncio.wait_for(first, 1)
            self.assertFalse(second.done())
            self.assertEqual(controller.job["status"], "running")
            self.assertNotIn("stop", controller.job)
            stopped = await asyncio.wait_for(owner.dispatch("/api/stop", {"job_id": job["id"]}), 1)
            self.assertEqual(stopped["job"]["stop"]["status"], "requested")
            release.set()
            value = await asyncio.wait_for(second, 2)
            self.assertEqual(value["job"]["status"], "unconfirmed")
            self.assertEqual(value["job"]["error"], "operation_stopped")
        finally:
            release.set()
            await asyncio.gather(first, second, return_exceptions=True)
            await owner.aclose()

    async def test_close_detaches_observation_before_host_worker_join(self):
        controller = self.controller()
        owner = adapter.SetupAdapter(controller)
        job, release = self.running(controller)
        waiting = asyncio.create_task(owner.dispatch("/api/job", {"job_id": job["id"], "wait_ms": 10000}))
        closing = None
        try:
            await self.observed(lambda: bool(owner._job_reads))
            closing = asyncio.create_task(owner.aclose())
            with self.assertRaises(asyncio.CancelledError):
                await asyncio.wait_for(waiting, 1)
            self.assertFalse(closing.done())
            self.assertTrue(controller._job_threads[job["id"]].is_alive())
            self.assertNotIn("stop", controller.job)
            self.assertFalse(owner._job_reads or owner._inflight)
        finally:
            release.set()
            if closing is not None:
                await asyncio.wait_for(closing, 2)
            else:
                await owner.aclose()

    async def test_cancelled_observer_joins_entered_read_without_joining_worker(self):
        controller = self.controller()
        owner = adapter.SetupAdapter(controller)
        job, worker_release = self.running(controller)
        entered, read_release = self.gate(), self.gate()
        original = controller.dispatch

        def held_read(route, arguments):
            result = original(route, arguments)
            entered.set()
            self.assertTrue(read_release.wait(3))
            return result

        waiting = None
        try:
            with patch.object(controller, "dispatch", side_effect=held_read):
                waiting = asyncio.create_task(owner.dispatch("/api/job", {"job_id": job["id"], "wait_ms": 10000}))
                self.assertTrue(await asyncio.to_thread(entered.wait, 2))
                waiting.cancel()
                await asyncio.sleep(0)
                waiting.cancel()
                await asyncio.sleep(0)
                self.assertFalse(waiting.done())
                self.assertTrue(owner._inflight)
                read_release.set()
                with self.assertRaises(asyncio.CancelledError):
                    await asyncio.wait_for(waiting, 1)
            self.assertTrue(controller._job_threads[job["id"]].is_alive())
            self.assertNotIn("stop", controller.job)
            self.assertFalse(owner._job_reads or owner._inflight)
        finally:
            read_release.set()
            worker_release.set()
            if waiting is not None:
                await asyncio.gather(waiting, return_exceptions=True)
            await owner.aclose()


if __name__ == "__main__":
    unittest.main()
