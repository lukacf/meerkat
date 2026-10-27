"""Acquisition shares setup's existing admission, stop and replay owner."""
import copy
import threading
import unittest

from test_selected_setup import SelectedFixture, adapter, backend


class Acquisition:
    def __init__(self):
        self.entered = threading.Event()
        self.release = threading.Event()
        self.calls = []
        self.fail = False

    def arguments(self, request):
        backend.keys(request, ("request_id", "distribution", "version"))
        return copy.deepcopy(request)

    def summary(self):
        return {"scope": "fixture_only"}

    def run(self, request, directory, run, stop):
        self.calls.append((request, directory))
        self.entered.set()
        while not self.release.wait(0.01):
            stop()
        stop()
        if self.fail:
            raise backend.SetupError("acquisition_fixture_refused")
        return {"scope": "retained_python_wheel_inputs", "staged_id": directory.name}


class AcquisitionJobs(SelectedFixture, unittest.TestCase):
    request = {"request_id": "b" * 32, "distribution": "example", "version": "1.0"}

    def enabled(self):
        controller = self.controller()
        controller.python_acquisition = Acquisition()
        self.addCleanup(controller.python_acquisition.release.set)
        return controller

    def test_disabled_by_default(self):
        controller = self.controller()
        with self.assertRaisesRegex(backend.SetupError, "python_acquisition_unavailable"):
            controller.acquire_python(self.request)
        self.assertEqual(controller.jobs, {})

    def test_replay_persists_single_job_and_request_before_worker(self):
        controller = self.enabled()
        (controller.workspace / "acquisitions").mkdir()
        first = controller.dispatch("/api/acquire_python", self.request)["job"]
        self.assertTrue(controller.python_acquisition.entered.wait(2))
        observed, directory = controller.python_acquisition.calls[0]
        self.assertEqual(backend.decode((directory / "request.json").read_bytes()), observed)
        self.assertEqual(controller.acquire_python(self.request)["job"]["id"], first["id"])
        with self.assertRaisesRegex(backend.SetupError, "acquisition_request_changed"):
            controller.acquire_python({**self.request, "version": "2.0"})
        controller.python_acquisition.release.set()
        controller.join()
        replay = controller.acquire_python(self.request)["job"]
        self.assertEqual(replay["status"], "succeeded")
        self.assertEqual(len(controller.python_acquisition.calls), 1)
        self.assertEqual(controller.job_detail({"job_id": first["id"]})["job"], replay)
        self.assertFalse(controller.reconciliation_available(replay))

    def test_stopped_attempt_cannot_be_replayed_into_another_download(self):
        controller = self.enabled()
        (controller.workspace / "acquisitions").mkdir()
        first = controller.acquire_python(self.request)["job"]
        self.assertTrue(controller.python_acquisition.entered.wait(2))
        controller.stop({"job_id": first["id"]})
        controller.join()
        replay = controller.acquire_python(self.request)["job"]
        self.assertEqual(replay["status"], "unconfirmed")
        self.assertEqual(replay["error"], "operation_stopped")
        self.assertEqual(replay["stop"]["status"], "settled")
        self.assertEqual(len(controller.python_acquisition.calls), 1)
        self.assertFalse(controller.operation_lock.locked())

    def test_other_request_cannot_run_concurrently_and_close_refuses_new_work(self):
        controller = self.enabled()
        (controller.workspace / "acquisitions").mkdir()
        controller.acquire_python(self.request)
        self.assertTrue(controller.python_acquisition.entered.wait(2))
        with self.assertRaisesRegex(backend.SetupError, "operation_in_progress"):
            controller.acquire_python({**self.request, "request_id": "c" * 32})
        self.assertEqual(len(controller.acquisition_requests), 1)
        controller.close_admission()
        with self.assertRaisesRegex(backend.SetupError, "host_closing"):
            controller.acquire_python({**self.request, "request_id": "d" * 32})
        controller.python_acquisition.release.set()
        controller.join()

    def test_acquisition_failure_does_not_become_install_success(self):
        controller = self.enabled()
        (controller.workspace / "acquisitions").mkdir()
        controller.python_acquisition.fail = True
        controller.python_acquisition.release.set()
        first = controller.acquire_python(self.request)["job"]
        controller.join()
        result = controller.job_detail({"job_id": first["id"]})["job"]
        self.assertEqual(result["error"], "acquisition_fixture_refused")
        self.assertEqual(result["status"], "unconfirmed")
        self.assertEqual(controller.validations, {})
        self.assertEqual(controller.install_intents, {})


if __name__ == "__main__":
    unittest.main()
