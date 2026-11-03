"""Selected-job TERM handoff with real inert installer and direct-child owners.

The metadata fixture is controlled. These cases run no package manager, build,
provider or MCP transport, and do not attest arbitrary descendant settlement.
"""
from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import signal
import sys
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("selected_stop_fixtures", ROOT / "tests/test_selected_setup.py")
fixtures = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fixtures)
backend = fixtures.backend
INSTALLER = ROOT.parents[1] / "scripts/install-library-consumer.py"
BOUND = 20


def wait_for(predicate, timeout=BOUND):
    end = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= end:
            raise AssertionError("owned process synchronization timed out")
        time.sleep(0.005)


class SelectedProcessRunner(backend.Runner):
    """Keep the existing custom run contract and actual selected argv intact."""
    def __init__(self, workspace):
        super().__init__(workspace)
        self.metadata = fixtures.SelectedRunner(workspace)
        self.block = None
        self.install_graces = []

    def run(self, argv, capture=False):
        if ("--selected-python" in argv or "--selected-node" in argv) and "--verify-existing" not in argv:
            self.install_graces.append(getattr(self._job_context, "termination_grace", None))
            return super().run(argv, capture=capture)
        return self.metadata.run(argv, capture=capture)


class SelectedInstallerStopTests(fixtures.SelectedFixture, unittest.TestCase):
    def setUp(self):
        super().setUp()
        self.releases = []
        self.owned = []

    def tearDown(self):
        # Release actual held children before any assertion can abandon a join.
        for release in self.releases:
            release.touch()
        late_workers = []
        for controller in self.controllers:
            for job in list(controller.jobs.values()):
                if job["status"] == "running":
                    controller.stop({"job_id": job["id"]})
            for thread in controller.threads:
                thread.join(BOUND)
                if thread.is_alive():
                    late_workers.append(thread.name)
                    # Keep the same worker and process handles in cleanup. A
                    # bounded failure cannot turn into a successful settlement.
                    for process in self.owned:
                        if process.returncode is None:
                            process.kill()
                    thread.join(BOUND)
                self.assertFalse(thread.is_alive(), "original selected worker remained live")
        for process in self.owned:
            if process.returncode is None:
                process.kill()
            process.wait(timeout=BOUND)
        self.temp.cleanup()
        self.assertEqual(late_workers, [], "cleanup needed outer escalation after its join bound")

    def install_program(self, source):
        path = Path(self.tools["installer"]["path"])
        path.write_text(source)
        self.tools["installer"]["sha256"] = backend.digest(path.read_bytes())
        self.profile["installer"] = self.tools["installer"]
        self.write_profile()

    def admit(self, profile):
        controller = self.controller(runner_factory=SelectedProcessRunner)
        checked = controller.selected_check(self.stage(profile=profile))
        request = self.selection(checked)
        return controller, checked, request

    def start(self, controller, request):
        actual = backend.subprocess.Popen
        def spawn(*args, **kwargs):
            child = actual(*args, **kwargs)
            self.owned.append(child)
            return child
        # Only the outer process is a child of this test. The installed program
        # retains and records its own inner wait; no PID probe replaces that.
        with patch.object(backend.subprocess, "Popen", side_effect=spawn):
            job = controller.selected_install(request)["job"]
            wait_for(lambda: bool(self.owned))
        return job

    def settled(self, controller, job):
        controller._job_threads[job["id"]].join(BOUND)
        self.assertFalse(controller._job_threads[job["id"]].is_alive())
        detail = controller.job_detail({"job_id": job["id"]})["job"]
        self.assertEqual(detail["status"], "unconfirmed")
        self.assertEqual(detail["error"], "operation_stopped")
        self.assertEqual(detail["stop"], {"status": "settled", "scope": "local_job_only"})
        self.assertNotIn("result", detail)
        self.assertFalse(controller.operation_lock.locked())
        return detail

    def test_selected_node_stop_joins_held_inner_child_before_worker_settles(self):
        ready, term, release, joined, stopped, next_stage = [self.root / name for name in
            ("inner-ready", "inner-term", "release", "inner-joined.json", "installer-stopped", "next-stage")]
        self.releases.append(release)
        inner = self.root / "inner.py"
        inner.write_text(
            "import os,signal,time\nfrom pathlib import Path\n"
            f"release=Path({str(release)!r})\n"
            "def stop(sig,frame):\n"
            f" Path({str(term)!r}).write_text('term')\n"
            " while not release.exists(): time.sleep(.005)\n"
            " raise SystemExit(0)\n"
            "signal.signal(signal.SIGTERM,stop)\n"
            f"Path({str(ready)!r}).write_text(str(os.getpid()))\n"
            "while not release.exists(): time.sleep(.01)\n")
        self.install_program(
            "import importlib.util,json,pathlib,signal,subprocess,sys\n"
            f"spec=importlib.util.spec_from_file_location('installer',{str(INSTALLER)!r})\n"
            "m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)\n"
            "def cancel(sig,frame): raise KeyboardInterrupt\n"
            "signal.signal(signal.SIGTERM,cancel)\n"
            "actual=m.subprocess.Popen\n"
            "def spawn(*args,**kwargs):\n"
            " p=actual(*args,**kwargs);wait=p.wait\n"
            " def joined_wait(*args,**kwargs):\n"
            "  code=wait(*args,**kwargs)\n"
            f"  pathlib.Path({str(joined)!r}).write_text(json.dumps({{'pid':p.pid,'returncode':code,'wait_returned':True}}))\n"
            "  return code\n"
            " p.wait=joined_wait\n return p\n"
            "m.subprocess.Popen=spawn\n"
            "destination=pathlib.Path(sys.argv[sys.argv.index('--destination')+1]);destination.mkdir()\n"
            "(destination/'partial-install').write_text('retained')\n"
            "try:\n"
            f" m.run({[sys.executable, '-I', str(inner)]!r},pathlib.Path({str(self.root)!r}),{{'PATH':'/usr/bin:/bin'}},'nested')\n"
            f" pathlib.Path({str(next_stage)!r}).write_text('unexpected')\n"
            "except KeyboardInterrupt:\n"
            f" pathlib.Path({str(stopped)!r}).write_text('unwound after inner join')\n"
            " raise SystemExit(130)\n")
        controller, checked, request = self.admit("node")
        job = self.start(controller, request)
        wait_for(ready.exists)
        controller.stop({"job_id": job["id"]})
        try:
            wait_for(term.exists)
            self.assertTrue(controller._job_threads[job["id"]].is_alive())
            self.assertTrue(controller.operation_lock.locked())
            self.assertEqual(controller.jobs[job["id"]]["status"], "running")
            self.assertFalse(joined.exists())
        finally:
            release.touch()
        detail = self.settled(controller, job)
        observation = json.loads(joined.read_text())
        self.assertEqual(observation, {"pid": int(ready.read_text()), "returncode": 0, "wait_returned": True})
        self.assertTrue(stopped.exists())
        self.assertEqual(self.owned[0].returncode, 130)
        self.assertEqual(controller.runner.install_graces, [10])
        self.assertFalse(next_stage.exists())
        destination = controller.install_intents[job["id"]]["destination"]
        self.assertEqual((destination / "partial-install").read_text(), "retained")
        self.assertFalse((destination / "ready").exists())
        self.assertEqual(controller.selected_install(request)["job"]["id"], detail["id"])
        self.assertEqual(len(self.owned), 1)
        # Worker finally removed the scoped setting before release; custom run
        # API and the exact already-stopped request do not authorize a retry.

    def test_selected_python_grace_exhaustion_joins_only_outer_and_stays_unconfirmed(self):
        ready = self.root / "outer-ready"
        self.install_program(
            "import os,pathlib,signal,time\n"
            "signal.signal(signal.SIGTERM,signal.SIG_IGN)\n"
            f"pathlib.Path({str(ready)!r}).write_text(str(os.getpid()))\n"
            "while True: time.sleep(.01)\n")
        controller, _, request = self.admit("python")
        job = self.start(controller, request)
        wait_for(ready.exists)
        controller.stop({"job_id": job["id"]})
        self.settled(controller, job)
        self.assertEqual(len(self.owned), 1)
        self.assertEqual(self.owned[0].pid, int(ready.read_text()))
        self.assertEqual(self.owned[0].returncode, -signal.SIGKILL)
        self.assertEqual(controller.runner.install_graces, [10])
        self.assertEqual(controller.selected_install(request)["job"]["id"], job["id"])
        # This branch deliberately creates no inner process. Outer escalation
        # proves no inner join receipt and must not be presented as such.

    def test_custom_run_override_keeps_signature_and_grace_is_scoped(self):
        controller = self.controller()
        checked = controller.selected_check(self.stage(profile="node"))
        request = self.selection(checked)
        actual = controller.runner.run
        seen = []
        def run(argv, capture=False):
            seen.append(("--verify-existing" in argv,
                         getattr(controller._job_context, "termination_grace", None)))
            return actual(argv, capture=capture)
        controller.runner.run = run
        readback = controller.selected_readback
        outside = []
        def observe_readback(intent):
            outside.append(getattr(controller._job_context, "termination_grace", None))
            result = readback(intent)
            outside.append(getattr(controller._job_context, "termination_grace", None))
            return result
        controller.selected_readback = observe_readback
        job = controller.selected_install(request)["job"]
        controller.join()
        self.assertEqual(controller.jobs[job["id"]]["status"], "succeeded")
        self.assertEqual(seen, [(False, 10), (True, 10)])
        self.assertEqual(outside, [None, None])
        self.assertEqual(controller.selected_install(request)["job"]["id"], job["id"])
        self.assertEqual(len(seen), 2)

    def test_scoped_grace_restores_prior_value_on_error_and_leaves_mcpb_generic(self):
        controller = self.controller()
        seen = []
        def refused(argv, capture=False):
            seen.append(getattr(controller._job_context, "termination_grace", None))
            raise backend.SetupError("operation_unconfirmed", 500)
        controller.runner.run = refused
        controller._job_context.termination_grace = 3
        with self.assertRaises(backend.SetupError):
            controller._run_selected_installer(["inert"], "node")
        self.assertEqual(controller._job_context.termination_grace, 3)
        del controller._job_context.termination_grace
        with self.assertRaises(backend.SetupError):
            controller._run_selected_installer(["inert"], "mcpb")
        self.assertEqual(seen, [10, None])
        self.assertFalse(hasattr(controller._job_context, "termination_grace"))


if __name__ == "__main__":
    unittest.main()
