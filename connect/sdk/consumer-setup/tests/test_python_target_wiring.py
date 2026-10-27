"""Host target binding across acquisition, installation and inert readback."""
import sys
import unittest
from pathlib import Path

from test_selected_setup import SelectedFixture, SelectedRunner, backend


class TargetRunner(SelectedRunner):
    def run(self, argv, capture=False):
        code, raw = super().run(argv, capture)
        if "--python-target" not in argv:
            return code, raw
        target = argv[argv.index("--python-target") + 1]
        if "--verify-existing" in argv:
            proof = backend.decode(raw)
            proof["python_target"] = target
            return code, backend.encoded(proof)
        path = Path(argv[argv.index("--destination") + 1]) / "ready/installed.json"
        receipt = backend.decode(path.read_bytes())
        receipt["python_target"] = target
        path.write_bytes(backend.encoded(receipt))
        return code, raw


class PythonTargetWiring(SelectedFixture, unittest.TestCase):
    def acquisition(self, target=None):
        pip = self.root / "pip.pyz"
        pip.write_bytes(b"not executed in this host binding test")
        value = {"schema_version": 1, "scope": "trusted_pypi_wheel_acquisition",
                 "python_sha256": backend.digest(Path(sys.executable).resolve().read_bytes()),
                 "pip": {"path": str(pip), "sha256": backend.digest(pip.read_bytes())}}
        if target is not None:
            value["target"] = target
        profile = self.root / "acquisition.json"
        profile.write_bytes(backend.encoded(value))
        return {"python_acquisition_profile": profile,
                "python_acquisition_profile_sha256": backend.digest(profile.read_bytes())}

    def test_target_agreement_required_before_workspace_effects(self):
        for selected, acquired in ((None, "linux-x86_64"), ("linux-x86_64", None)):
            with self.subTest(selected=selected, acquired=acquired):
                self.profile.pop("python_target", None)
                if selected is not None:
                    self.profile["python_target"] = selected
                self.write_profile()
                workspace = self.root / ("mismatch-" + str(selected))
                with self.assertRaisesRegex(backend.SetupError, "python_target_mismatch"):
                    self.controller(workspace=workspace, **self.acquisition(acquired))
                self.assertFalse(workspace.exists())

    def test_agreed_profiles_keep_target_host_owned(self):
        self.profile["python_target"] = "linux-x86_64"
        self.write_profile()
        c = self.controller(**self.acquisition("linux-x86_64"))
        self.assertEqual(c.python_acquisition.target, "linux-x86_64")
        request = {"request_id": "a" * 32, "distribution": "some-package", "version": "1.0", "target": "macos-arm64"}
        with self.assertRaises(backend.SetupError):
            c.acquire_python(request)
        self.assertEqual(c.runner.calls, [])

    def test_invalid_or_irrelevant_target_refused(self):
        for value in (None, True, [], {}, "linux-arm64", ""):
            with self.subTest(value=value):
                self.profile["python_target"] = value
                self.write_profile()
                with self.assertRaisesRegex(backend.SetupError, "invalid_selected_host_profile"):
                    self.controller()
        self.profile["python_target"] = "linux-x86_64"
        self.profile["validators"].pop("python")
        self.write_profile()
        with self.assertRaisesRegex(backend.SetupError, "invalid_selected_host_profile"):
            self.controller()

    def test_legacy_mac_omits_new_cli_argument(self):
        c, _, job = self.installed()
        self.assertNotIn("--python-target", [arg for argv, _ in c.runner.calls for arg in argv])
        self.assertEqual(c.reconcile({"job_id": job["id"]})["observation"]["status"], "matched")

    def test_mac_host_refuses_linux_readback_proof_or_receipt(self):
        for crossed in ("proof", "receipt"):
            with self.subTest(crossed=crossed):
                c = self.controller()
                checked = c.selected_check(self.stage(name=crossed))
                job = c.selected_install(self.selection(checked))["job"]
                c.join()
                if crossed == "proof":
                    c.runner.bad_proof = {"python_target": "linux-x86_64"}
                else:
                    path = Path(checked["destination"]) / "ready/installed.json"
                    receipt = backend.decode(path.read_bytes())
                    receipt["python_target"] = "linux-x86_64"
                    path.write_bytes(backend.encoded(receipt))
                with self.assertRaisesRegex(backend.SetupError, "reconciliation_unconfirmed"):
                    c.selected_readback(c.install_intents[job["id"]])

    def test_linux_target_survives_install_and_recovery(self):
        self.profile["python_target"] = "linux-x86_64"
        self.write_profile()
        c = self.controller(runner_factory=TargetRunner)
        checked = c.selected_check(self.stage())
        job = c.selected_install(self.selection(checked))["job"]
        c.join()
        self.assertEqual(c.jobs[job["id"]]["status"], "succeeded", c.jobs[job["id"]])
        for argv, _ in c.runner.calls[1:]:
            self.assertEqual(argv[argv.index("--python-target") + 1], "linux-x86_64")
        recovered = self.controller(runner_factory=TargetRunner, recover_workspace=c.workspace,
                                    recover_request=job["id"], recover_request_sha256=job["recovery_request"]["sha256"])
        result = recovered.reconcile({"job_id": job["id"]})
        self.assertEqual(result["observation"]["status"], "matched")
        self.assertTrue(all("--verify-existing" in argv for argv, _ in recovered.runner.calls))
        self.assertTrue(all(argv[argv.index("--python-target") + 1] == "linux-x86_64" for argv, _ in recovered.runner.calls))


if __name__ == "__main__":
    unittest.main()
