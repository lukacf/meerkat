"""Independent stop regressions; fixture bytes are not live installation proof."""
import copy
import http.client
import json
import signal
import subprocess
import sys
import threading
import time
import unittest
from unittest.mock import patch

from test_backend import FakeRunner, FixtureMixin, backend


class ReturnGateRunner(FakeRunner):
    """Hold a completed owner call before returning its observed result."""

    def __init__(self, workspace):
        super().__init__(workspace)
        self.hold_argument = None
        self.result_ready = threading.Event()
        self.release_result = threading.Event()

    def run(self, argv, capture=False):
        result = super().run(argv, capture=capture)
        if self.hold_argument in argv:
            self.result_ready.set()
            if not self.release_result.wait(5):
                raise AssertionError("fixture return gate timed out")
        return result


class StopJobAdversarialTests(FixtureMixin, unittest.TestCase):
    def tearDown(self):
        for controller in self.controllers:
            gate = getattr(controller.runner, "release_result", None)
            if gate is not None:
                gate.set()
        super().tearDown()

    def gated_prepared(self):
        controller = self.controller(runner_factory=ReturnGateRunner)
        self.addCleanup(controller.runner.release_result.set)
        controller.prepare({"family": "agno"})
        controller.join()
        self.assertEqual(controller.job["status"], "succeeded")
        return controller

    def settled(self, controller):
        controller.runner.release_result.set()
        controller.join()
        self.assertEqual(controller.job["status"], "unconfirmed")
        self.assertEqual(controller.job["error"], "operation_stopped")
        self.assertEqual(controller.job["stop"]["status"], "settled")
        self.assertNotIn("result", controller.job)
        self.assertFalse(controller.state()["operation_in_progress"])

    def test_committed_files_survive_stop_and_exact_replay_never_restarts(self):
        controller = self.gated_prepared()
        request = self.checked(controller)
        controller.runner.hold_argument = "--destination"
        job = controller.install(request)["job"]
        self.assertTrue(controller.runner.result_ready.wait(2))
        destination = controller.install_intents[job["id"]]["destination"]
        paths = [destination / "ready/installed.json", destination / "ready/mcp.json"]
        original = [path.read_bytes() for path in paths]
        stopped = controller.dispatch("/api/stop", {"job_id": job["id"]})["job"]
        self.assertEqual(stopped["status"], "running")
        self.assertEqual(stopped["stop"]["status"], "requested")
        self.assertTrue(controller.operation_lock.locked())
        calls = copy.deepcopy(controller.runner.calls)
        self.assertEqual(controller.install(request)["job"], stopped)
        self.assertEqual(controller.dispatch("/api/stop", {"job_id": job["id"]})["job"], stopped)
        self.settled(controller)
        self.assertEqual([path.read_bytes() for path in paths], original)
        self.assertEqual(controller.install(request)["job"], controller.job)
        self.assertEqual(controller.dispatch("/api/stop", {"job_id": job["id"]})["job"], controller.job)
        self.assertEqual(controller.runner.calls, calls)

    def test_old_completed_job_cannot_stop_current_job(self):
        controller = self.gated_prepared()
        old = copy.deepcopy(controller.job)
        request = self.checked(controller)
        controller.runner.hold_argument = "config-help"
        job = controller.install(request)["job"]
        self.assertTrue(controller.runner.result_ready.wait(2))
        self.assertEqual(controller.dispatch("/api/stop", {"job_id": old["id"]})["job"], old)
        self.assertNotIn("stop", controller.job)
        other = self.controller()
        self.error("unknown_job", lambda: other.dispatch("/api/stop", {"job_id": job["id"]}))
        self.assertNotIn("stop", controller.job)
        controller.dispatch("/api/stop", {"job_id": job["id"]})
        self.settled(controller)

    def test_http_stop_and_state_available_while_install_parser_is_blocked(self):
        controller = self.gated_prepared()
        request = self.checked(controller)
        controller.runner.hold_argument = "config-help"
        job = controller.install(request)["job"]
        self.assertTrue(controller.runner.result_ready.wait(2))
        calls = copy.deepcopy(controller.runner.calls)
        server = backend.Server(controller)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()

        def post(route, body):
            connection = http.client.HTTPConnection(*server.server_address, timeout=1)
            try:
                connection.request("POST", route, backend.encoded(body), headers={
                    "Origin": server.origin, "X-Toolkit-Session": server.capability,
                    "Content-Type": "application/json"})
                response = connection.getresponse()
                return response.status, json.loads(response.read())
            finally:
                connection.close()

        try:
            status, response = post("/api/stop", {"job_id": job["id"]})
            self.assertEqual(status, 200)
            self.assertEqual(response["job"]["stop"]["status"], "requested")
            status, response = post("/api/state", {})
            self.assertEqual(status, 200)
            self.assertTrue(response["operation_in_progress"])
            self.settled(controller)
            # No check-config or installer call follows the stopped help call.
            self.assertEqual(controller.runner.calls, calls)
        finally:
            controller.runner.release_result.set()
            server.shutdown()
            server.server_close()
            thread.join(2)

    def test_signal_failure_cannot_settle_or_release_a_live_child(self):
        controller = self.controller()
        runner = backend.Runner(controller.workspace)
        controller.runner = runner
        controller._job_context = runner._job_context
        # FixtureMixin cleanup expects custom runners to expose this gate.
        runner.block = None
        entered = threading.Event()
        exited = threading.Event()

        class Child:
            pid = 123456789
            returncode = None

            def wait(self, timeout=None):
                entered.set()
                if not exited.wait(timeout):
                    raise subprocess.TimeoutExpired("fixture", timeout)
                self.returncode = 0
                return 0

        child = Child()
        with patch.object(backend.subprocess, "Popen", return_value=child), patch.object(
                backend.os, "killpg", side_effect=PermissionError("fixture signal denied")):
            try:
                with controller.lock:
                    job = controller.start_job("install", "agno", lambda: controller._run(["fixture"]))
                self.assertTrue(entered.wait(2))
                controller.dispatch("/api/stop", {"job_id": job["id"]})
                controller.threads[-1].join(0.5)
                self.assertIsNone(child.returncode)
                self.assertTrue(controller.operation_lock.locked(), "live child lost operation custody")
                self.assertEqual(controller.job["status"], "running")
                self.assertEqual(controller.job["stop"]["status"], "requested")
            finally:
                exited.set()
                controller.join()

    def test_real_owned_child_stops_without_signaling_bystander_or_next_stage(self):
        controller = self.controller()
        runner = backend.Runner(controller.workspace)
        controller.runner = runner
        controller._job_context = runner._job_context
        runner.block = None
        ready = self.root / "owned-ready"
        later = self.root / "later-stage"
        owned = []
        signals = []
        real_popen = subprocess.Popen
        real_killpg = backend.os.killpg
        bystander = real_popen([sys.executable, "-I", "-c", "import time; time.sleep(30)"],
                               stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, start_new_session=True)

        def spawn(*args, **kwargs):
            child = real_popen(*args, **kwargs)
            owned.append(child)
            return child

        def kill_group(pid, sig):
            signals.append((pid, sig))
            return real_killpg(pid, sig)

        def work():
            controller._run([sys.executable, "-I", "-c",
                "import pathlib,sys,time; pathlib.Path(sys.argv[1]).write_text('ready'); time.sleep(30)", str(ready)])
            controller._run([sys.executable, "-I", "-c",
                "import pathlib,sys; pathlib.Path(sys.argv[1]).write_text('unexpected')", str(later)])
            return {}

        try:
            with patch.object(backend.subprocess, "Popen", side_effect=spawn), patch.object(
                    backend.os, "killpg", side_effect=kill_group):
                with controller.lock:
                    job = controller.start_job("install", "agno", work)
                deadline = time.monotonic() + 3
                while not ready.exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(ready.exists())
                controller.dispatch("/api/stop", {"job_id": job["id"]})
                controller.threads[-1].join(3)
                self.assertFalse(controller.threads[-1].is_alive())
                self.assertEqual(len(owned), 1)
                self.assertEqual(signals, [(owned[0].pid, signal.SIGKILL)])
                self.assertEqual(owned[0].returncode, -signal.SIGKILL)
                self.assertIsNone(bystander.poll())
                self.assertFalse(later.exists())
                self.assertEqual(controller.job["stop"]["status"], "settled")
                self.assertEqual(controller.job["status"], "unconfirmed")
        finally:
            for child in owned + [bystander]:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=3)
            controller.join()

    def test_reaped_child_is_never_signaled_when_stop_arrives_before_result(self):
        controller = self.controller()
        runner = backend.Runner(controller.workspace)
        controller.runner = runner
        controller._job_context = runner._job_context
        runner.block = None
        entered = threading.Event()
        return_result = threading.Event()

        class Child:
            pid = 123456789
            returncode = None

            def wait(self, timeout=None):
                entered.set()
                if not return_result.wait(timeout):
                    raise subprocess.TimeoutExpired("fixture", timeout)
                self.returncode = 0
                controller.dispatch("/api/stop", {"job_id": controller.job["id"]})
                return 0

        with patch.object(backend.subprocess, "Popen", return_value=Child()), patch.object(
                backend.os, "killpg") as signal_group:
            try:
                with controller.lock:
                    controller.start_job("install", "agno", lambda: controller._run(["fixture"]))
                self.assertTrue(entered.wait(2))
                return_result.set()
                controller.join()
                signal_group.assert_not_called()
                self.assertEqual(controller.job["status"], "unconfirmed")
                self.assertEqual(controller.job["error"], "operation_stopped")
            finally:
                return_result.set()
                controller.join()


if __name__ == "__main__":
    unittest.main()
