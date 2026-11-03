"""Explicit local-job stop with real owned processes and existing owner fixtures.

Fixtures exercise package receipts and replay, not a live provider or a native
acquisition. Real subprocess cases establish direct-child ownership only.
"""
from __future__ import annotations

import asyncio
import copy
import importlib.util
from pathlib import Path
import subprocess
import sys
import threading
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


adapter = load("stop_job_adapter", ROOT / "mcp_adapter.py")
backend = adapter.backend
fixtures = load("stop_job_fixtures", ROOT / "tests/test_backend.py")
fixtures.backend = backend
selected = load("stop_job_selected", ROOT / "tests/test_selected_setup.py")
selected.backend = backend
selected.adapter = adapter


def wait_for(predicate, timeout=5):
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= deadline:
            raise AssertionError("Timed out waiting for test synchronization")
        time.sleep(0.005)


class StopFixture(fixtures.FixtureMixin):
    def setUp(self):
        super().setUp()
        self.gates = []

    def gate(self):
        gate = threading.Event()
        self.gates.append(gate)
        return gate

    def tearDown(self):
        for gate in self.gates:
            gate.set()
        for controller in self.controllers:
            for job in list(controller.jobs.values()):
                if job["status"] == "running":
                    controller.stop({"job_id": job["id"]})
            block = getattr(controller.runner, "block", None)
            if block:
                block.set()
            for thread in controller.threads:
                thread.join(5)
                self.assertFalse(thread.is_alive(), "owned worker did not settle")
        self.temp.cleanup()

    def real_controller(self):
        return self.controller(runner_factory=backend.Runner)

    def start(self, controller, worker, **kwargs):
        with controller.lock:
            return copy.deepcopy(controller.start_job("install", "agno", worker, **kwargs))

    def settle(self, controller, job):
        for thread in controller.threads:
            thread.join(5)
            self.assertFalse(thread.is_alive(), "owned worker did not settle")
        result = controller.job_detail({"job_id": job["id"]})["job"]
        self.assertEqual(result["status"], "unconfirmed")
        self.assertEqual(result["error"], "operation_stopped")
        self.assertEqual(result["stop"], {"status": "settled", "scope": "local_job_only"})
        self.assertFalse(controller.operation_lock.locked())
        return result

    def sleeper(self, controller, name, *, capture=False, close_stdout=False):
        ready = self.root / name
        code = "import os,sys,time;from pathlib import Path;Path(sys.argv[1]).write_text(str(os.getpid()));"
        if close_stdout:
            code += "os.close(1);"
        code += "time.sleep(300)"
        job = self.start(controller, lambda: controller._run([sys.executable, "-I", "-c", code, str(ready)], capture=capture))
        wait_for(ready.exists)
        return job, ready


class StopJobTests(StopFixture, unittest.TestCase):
    def test_stop_before_first_spawn_and_before_success_callback(self):
        controller = self.real_controller()
        entered, release = self.gate(), self.gate()
        marker = self.root / "must-not-exist"
        finalized = []
        def work():
            entered.set()
            self.assertTrue(release.wait(5))
            return controller._run([sys.executable, "-I", "-c", "from pathlib import Path;import sys;Path(sys.argv[1]).touch()", str(marker)])
        job = self.start(controller, work, on_success=lambda: finalized.append(True))
        self.assertTrue(entered.wait(5))
        requested = controller.dispatch("/api/stop", {"job_id": job["id"]})["job"]
        self.assertEqual(requested["status"], "running")
        self.assertEqual(requested["stop"]["status"], "requested")
        self.assertEqual(controller.stop({"job_id": job["id"]})["job"], requested)
        release.set()
        self.settle(controller, job)
        self.assertFalse(marker.exists())
        self.assertEqual(finalized, [])

    def test_real_process_stop_during_wait_or_capture_joins_before_settlement(self):
        for capture, closed in ((False, False), (True, False), (True, True)):
            with self.subTest(capture=capture, closed=closed):
                controller = self.real_controller()
                processes = []
                original = subprocess.Popen
                def spawn(*args, **kwargs):
                    child = original(*args, **kwargs)
                    processes.append(child)
                    return child
                with patch.object(backend.subprocess, "Popen", side_effect=spawn):
                    job, marker = self.sleeper(controller, "ready-" + str(self.attempts), capture=capture, close_stdout=closed)
                    started = time.monotonic()
                    requested = controller.stop({"job_id": job["id"]})["job"]
                    self.assertLess(time.monotonic() - started, 1)
                    self.assertEqual(requested["stop"]["status"], "requested")
                    self.settle(controller, job)
                self.assertEqual(len(processes), 1)
                self.assertIsNotNone(processes[0].returncode)
                self.assertTrue(marker.exists(), "stop must retain local effects")

    def test_stop_between_real_stages_never_launches_successor(self):
        controller = self.real_controller()
        entered, release = self.gate(), self.gate()
        first, second = self.root / "first", self.root / "second"
        code = "from pathlib import Path;import sys;Path(sys.argv[1]).touch()"
        def work():
            controller._run([sys.executable, "-I", "-c", code, str(first)])
            entered.set()
            self.assertTrue(release.wait(5))
            return controller._run([sys.executable, "-I", "-c", code, str(second)])
        job = self.start(controller, work)
        self.assertTrue(entered.wait(5))
        controller.stop({"job_id": job["id"]})
        release.set()
        self.settle(controller, job)
        self.assertTrue(first.exists())
        self.assertFalse(second.exists())

    def test_foreign_id_and_old_job_cannot_stop_current_job(self):
        controller = self.real_controller()
        first, _ = self.sleeper(controller, "first-running")
        for identifier in ("f" * 32, "not-an-id", None):
            self.error("unknown_job", lambda: controller.stop({"job_id": identifier}))
        self.assertNotIn("stop", controller.job_detail({"job_id": first["id"]})["job"])
        controller.stop({"job_id": first["id"]})
        settled = self.settle(controller, first)
        second, _ = self.sleeper(controller, "second-running")
        self.assertEqual(controller.stop({"job_id": first["id"]})["job"], settled)
        current = controller.job_detail({"job_id": second["id"]})["job"]
        self.assertEqual(current["status"], "running")
        self.assertNotIn("stop", current)
        controller.stop({"job_id": second["id"]})
        self.settle(controller, second)

    def test_completed_job_stop_is_unchanged_and_does_not_signal(self):
        controller = self.real_controller()
        job = self.start(controller, lambda: {"retained": True})
        controller.join()
        before = controller.job_detail({"job_id": job["id"]})["job"]
        with patch.object(backend.os, "killpg") as signal_group:
            after = controller.stop({"job_id": job["id"]})["job"]
        self.assertEqual(before, after)
        self.assertEqual(after["status"], "succeeded")
        self.assertNotIn("stop", after)
        signal_group.assert_not_called()

    def test_stop_wins_final_publication_and_prepare_does_not_become_prepared(self):
        controller = self.controller()
        entered, release = self.gate(), self.gate()
        original = controller._run
        def run(argv, **kwargs):
            result = original(argv, **kwargs)
            if argv[-1:] == ["profiles"]:
                entered.set()
                self.assertTrue(release.wait(5))
            return result
        with patch.object(controller, "_run", side_effect=run):
            job = controller.prepare({"family": "agno"})["job"]
            self.assertTrue(entered.wait(5))
            controller.stop({"job_id": job["id"]})
            self.assertEqual(controller.family("agno")["status"], "preparing")
            release.set()
            self.settle(controller, job)
        self.assertEqual(controller.family("agno")["status"], "unconfirmed")
        self.assertEqual(controller.family("agno")["profiles"], [])
        self.assertNotIn("agno", controller.commands)

    def test_stop_after_effect_preserves_request_files_and_exact_replay(self):
        controller = self.prepared()
        request = self.checked(controller)
        entered, release = self.gate(), self.gate()
        original = controller.runner.run
        def run(argv, **kwargs):
            result = original(argv, **kwargs)
            if "--destination" in argv and "--config" in argv:
                entered.set()
                self.assertTrue(release.wait(5))
            return result
        with patch.object(controller.runner, "run", side_effect=run):
            job = controller.install(request)["job"]
            self.assertTrue(entered.wait(5))
            destination = controller.install_intents[job["id"]]["destination"]
            receipt = destination / "ready/installed.json"
            retained = controller.workspace / "requests" / (job["id"] + ".json")
            receipt_bytes, request_bytes = receipt.read_bytes(), retained.read_bytes()
            controller.stop({"job_id": job["id"]})
            release.set()
            settled = self.settle(controller, job)
        count = len(controller.runner.calls)
        self.assertEqual(controller.install(request)["job"], settled)
        self.assertEqual(len(controller.runner.calls), count)
        self.assertEqual(receipt.read_bytes(), receipt_bytes)
        self.assertEqual(retained.read_bytes(), request_bytes)

    def test_signal_failure_keeps_ownership_until_real_child_exit(self):
        controller = self.real_controller()
        ready, release = self.root / "signal-ready", self.root / "signal-release"
        code = "from pathlib import Path;import sys,time;Path(sys.argv[1]).touch();\nwhile not Path(sys.argv[2]).exists(): time.sleep(.01)"
        job = self.start(controller, lambda: controller._run([sys.executable, "-I", "-c", code, str(ready), str(release)]))
        wait_for(ready.exists)
        attempted = self.gate()
        def denied(*_):
            attempted.set()
            raise PermissionError("injected signal failure")
        try:
            with patch.object(backend.os, "killpg", side_effect=denied):
                controller.stop({"job_id": job["id"]})
                self.assertTrue(attempted.wait(5))
                self.assertTrue(controller.operation_lock.locked())
                self.assertEqual(controller.job_detail({"job_id": job["id"]})["job"]["status"], "running")
                self.error("operation_in_progress", lambda: self.start(controller, lambda: {}))
                release.touch()
                self.settle(controller, job)
        finally:
            release.touch()

    def test_captured_descendant_pipe_is_interrupted_even_after_leader_exit(self):
        controller = self.real_controller()
        ready = self.root / "descendant-ready"
        code = "import subprocess,sys;from pathlib import Path;subprocess.Popen([sys.executable,'-I','-c','import time;time.sleep(300)']);Path(sys.argv[1]).touch()"
        job = self.start(controller, lambda: controller._run([sys.executable, "-I", "-c", code, str(ready)], capture=True))
        wait_for(ready.exists)
        controller.stop({"job_id": job["id"]})
        self.settle(controller, job)


class SelectedStopTests(selected.SelectedFixture, unittest.TestCase):
    def test_selected_stop_retains_exact_replay_and_reconciliation(self):
        controller = self.controller()
        checked = controller.selected_check(self.stage())
        request = self.selection(checked)
        entered, release = threading.Event(), threading.Event()
        original = controller.runner.run
        def run(argv, **kwargs):
            result = original(argv, **kwargs)
            if "--destination" in argv and "--verify-existing" not in argv:
                entered.set()
                if not release.wait(5):
                    raise AssertionError("missing stop test release")
            return result
        try:
            with patch.object(controller.runner, "run", side_effect=run):
                job = controller.selected_install(request)["job"]
                self.assertTrue(entered.wait(5))
                controller.stop({"job_id": job["id"]})
                release.set()
                controller.join()
            detail = controller.job_detail({"job_id": job["id"]})["job"]
            self.assertEqual(detail["status"], "unconfirmed")
            self.assertEqual(detail["stop"]["status"], "settled")
            count = len(controller.runner.calls)
            self.assertEqual(controller.selected_install(request)["job"], detail)
            self.assertEqual(len(controller.runner.calls), count)
            observation = controller.reconcile({"job_id": job["id"]})["observation"]
            self.assertEqual(observation["status"], "matched")
            self.assertEqual(controller.job_detail({"job_id": job["id"]})["job"], detail)
        finally:
            release.set()


@unittest.skipUnless(importlib.util.find_spec("mcp") is not None, "MCP SDK required")
class StopMcpTests(StopFixture, unittest.IsolatedAsyncioTestCase):
    async def test_real_mcp_stop_schema_and_exact_job_dispatch(self):
        from mcp import Client
        controller = self.real_controller()
        owner = adapter.SetupAdapter(controller)
        try:
            async with Client(adapter.create_server(owner)) as client:
                stop = next(tool for tool in (await client.list_tools()).tools if tool.name == "setup_stop")
                self.assertEqual(stop.input_schema["required"], ["job_id"])
                self.assertFalse(stop.input_schema["additionalProperties"])
                self.assertFalse(stop.annotations.read_only_hint)
                self.assertTrue(stop.annotations.idempotent_hint)
                self.assertTrue(stop.annotations.destructive_hint)
                job, _ = await asyncio.to_thread(self.sleeper, controller, "mcp-ready")
                rejected = await client.call_tool("setup_stop", {"job_id": job["id"], "pid": 1})
                self.assertTrue(rejected.is_error)
                self.assertNotIn("stop", controller.job_detail({"job_id": job["id"]})["job"])
                response = await client.call_tool("setup_stop", {"job_id": job["id"]})
                self.assertFalse(response.is_error)
                self.assertEqual(response.structured_content["job"]["id"], job["id"])
                self.assertEqual(response.structured_content["job"]["status"], "running")
                self.assertEqual(response.structured_content["job"]["stop"]["status"], "requested")
                await asyncio.to_thread(self.settle, controller, job)
                again = await client.call_tool("setup_stop", {"job_id": job["id"]})
                self.assertFalse(again.is_error)
                self.assertEqual(again.structured_content["job"]["stop"]["status"], "settled")
        finally:
            for job in list(controller.jobs.values()):
                if job["status"] == "running":
                    controller.stop({"job_id": job["id"]})
            await owner.aclose()

    async def test_real_mcp_cancelled_read_is_not_stop_and_lost_stop_reply_is_observable(self):
        from mcp import Client
        controller = self.real_controller()
        owner = adapter.SetupAdapter(controller)
        entered, release = self.gate(), self.gate()
        original = controller.dispatch
        blocked_route = ["/api/job"]
        def delayed_reply(route, arguments):
            result = original(route, arguments)
            if route == blocked_route[0]:
                entered.set()
                if not release.wait(5):
                    raise AssertionError("missing MCP reply release")
            return result
        try:
            async with Client(adapter.create_server(owner)) as client:
                job, _ = await asyncio.to_thread(self.sleeper, controller, "cancel-ready")
                with patch.object(controller, "dispatch", side_effect=delayed_reply):
                    read = asyncio.create_task(client.call_tool("setup_job", {"job_id": job["id"]}))
                    self.assertTrue(await asyncio.to_thread(entered.wait, 5))
                    read.cancel()
                    release.set()
                    with self.assertRaises(asyncio.CancelledError):
                        await read
                    await asyncio.to_thread(wait_for, lambda: not owner._inflight)
                    current = controller.job_detail({"job_id": job["id"]})["job"]
                    self.assertEqual(current["status"], "running")
                    self.assertNotIn("stop", current)
                    entered.clear()
                    release.clear()
                    blocked_route[0] = "/api/stop"
                    stop = asyncio.create_task(client.call_tool("setup_stop", {"job_id": job["id"]}))
                    self.assertTrue(await asyncio.to_thread(entered.wait, 5))
                    stop.cancel()
                    release.set()
                    with self.assertRaises(asyncio.CancelledError):
                        await stop
                    await asyncio.to_thread(wait_for, lambda: not owner._inflight)
                await asyncio.to_thread(self.settle, controller, job)
                observed = await client.call_tool("setup_job", {"job_id": job["id"]})
                self.assertFalse(observed.is_error)
                self.assertEqual(observed.structured_content["job"]["stop"]["status"], "settled")
        finally:
            release.set()
            for job in list(controller.jobs.values()):
                if job["status"] == "running":
                    controller.stop({"job_id": job["id"]})
            await owner.aclose()

    async def test_recovery_omits_stop_and_refuses_direct_route(self):
        from mcp import Client
        controller = self.controller()
        # The mode check is independent of package recovery parsing, exercised
        # in test_recovery.py. No process is started in this mode fixture.
        controller.recovery_only = True
        self.assertFalse(controller.state()["stop"]["available"])
        self.error("recovery_read_only", lambda: controller.dispatch("/api/stop", {"job_id": "a" * 32}))
        owner = adapter.SetupAdapter(controller)
        try:
            async with Client(adapter.create_server(owner)) as client:
                self.assertNotIn("setup_stop", {tool.name for tool in (await client.list_tools()).tools})
                result = await client.call_tool("setup_stop", {"job_id": "a" * 32})
                self.assertTrue(result.is_error)
        finally:
            await owner.aclose()


if __name__ == "__main__":
    unittest.main(verbosity=2)
