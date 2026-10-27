"""Host-selected native Python targets, using inert wheels and trusted pip fixtures."""
import copy
import json
import os
import platform
import sys
import unittest
from unittest.mock import patch
import zipfile

import test_python_acquisition as fixtures

M, B = fixtures.M, fixtures.B


class PythonAcquisitionTargets(unittest.TestCase):
    def setUp(self):
        self.case = fixtures.AcquisitionTests()
        self.case.setUp()
        self.addCleanup(self.case.doCleanups)

    def owner(self, target=None):
        if target is not None:
            self.case.profile["target"] = target
        return self.case.owner()

    def execute(self, owner, observed=None):
        def runner(*args, **kwargs):
            result = self.case.runner(*args, **kwargs)
            runtime = {"target": observed or ["cpython", 3, 11, *M.TARGETS[owner.target]],
                       "stdlib": sorted(sys.stdlib_module_names),
                       **({"pointer_bits": 64} if owner.target == "linux-x86_64" else {})}
            (self.case.job / "runtime.json").write_bytes(M.wire(runtime))
            return result
        return self.case.execute(owner, runner=runner)

    def envelope(self, owner):
        return {"schema_version": 1, "kind": "toolkit.consumer_setup.acquisition_request",
                "job_id": self.case.job.name, "family": "selected-python", "workspace": str(owner.workspace),
                "staging_root": str(owner.staging), "arguments": copy.deepcopy(self.case.request),
                "host": {"profile_sha256": owner.profile_sha, "selected_profile_sha256": "d" * 64,
                         "assets": {name: M.sha(raw) for name, raw in owner.frozen.items()}}}

    def readback(self, owner):
        with patch("subprocess.Popen", side_effect=AssertionError("readback must not execute")):
            return M.readback(self.case.io, self.envelope(owner), self.case.job)

    def test_omitted_target_preserves_mac_default_and_is_bound_in_new_outputs(self):
        owner = self.owner()
        self.assertEqual(owner.target, "macos-arm64")
        self.assertNotIn("target", owner.profile)
        result = self.execute(owner)
        self.assertEqual(result["target"], "macos-arm64")
        self.assertEqual(json.loads((self.case.job / "worker.json").read_bytes())["target"], "macos-arm64")
        self.assertEqual(owner.summary()["target"], "CPython 3.11 macOS arm64")
        self.assertEqual(self.readback(owner), result)

    def test_linux_choice_uses_same_wheel_owner_and_remains_readable_without_external_tools(self):
        owner = self.owner("linux-x86_64")
        result = self.execute(owner)
        self.assertEqual(result["target"], "linux-x86_64")
        self.assertEqual(owner.summary()["target"], "CPython 3.11 Linux x86_64")
        self.assertEqual(json.loads((self.case.job / "worker.json").read_bytes())["target"], "linux-x86_64")
        self.case.profile_path.unlink()
        self.case.pip.unlink()
        self.assertEqual(self.readback(owner), result)
        self.assertEqual({row["name"] for row in result["artifacts"]}, {"outside-server", "dependency"})

    def test_invalid_targets_are_rejected_before_binding(self):
        for value in (None, "", "linux-arm64", "Linux-x86_64", "darwin", ["linux-x86_64"], True):
            with self.subTest(value=value):
                self.case.profile["target"] = value
                with self.assertRaisesRegex(B.SetupError, "invalid_python_acquisition_profile"):
                    self.case.owner()
                self.assertFalse((self.case.workspace / "python-acquisition").exists())

    def test_agent_cannot_override_host_target(self):
        owner = self.owner("linux-x86_64")
        with self.assertRaisesRegex(B.SetupError, "invalid_python_acquisition_arguments"):
            owner.arguments({**self.case.request, "target": "macos-arm64"})
        self.assertEqual(self.case.calls, [])

    def test_mismatched_runtime_is_rejected_before_downloading_any_wheel(self):
        owner = self.owner("linux-x86_64")
        with self.assertRaisesRegex(B.SetupError, "python_acquisition_unconfirmed"):
            self.execute(owner, observed=["cpython", 3, 11, "darwin", "arm64"])
        self.assertEqual(self.case.fetches, [])
        self.assertFalse((self.case.staging / self.case.job.name).exists())

    def test_target_proof_cannot_be_changed_or_omitted_on_linux_readback(self):
        owner = self.owner("linux-x86_64")
        self.execute(owner)
        receipt = self.case.staging / self.case.job.name / "acquisition.json"
        worker, runtime = self.case.job / "worker.json", self.case.job / "runtime.json"
        for path, key, value, remove in ((receipt, "target", "macos-arm64", False),
                (receipt, "target", None, True), (worker, "target", "macos-arm64", False),
                (worker, "target", None, True),
                (runtime, "target", ["cpython", 3, 11, "darwin", "arm64"], False),
                (runtime, "target", ["cpython", 3.0, 11, "linux", "x86_64"], False),
                (runtime, "pointer_bits", None, True), (runtime, "pointer_bits", 32, False),
                (runtime, "pointer_bits", 64.0, False)):
            with self.subTest(path=path.name, value=value, remove=remove):
                raw = path.read_bytes()
                value_json = json.loads(raw)
                if remove:
                    value_json.pop(key)
                else:
                    value_json[key] = value
                path.write_bytes(M.wire(value_json))
                try:
                    with self.assertRaisesRegex(B.SetupError, "python_acquisition_readback_unconfirmed"):
                        self.readback(owner)
                finally:
                    path.write_bytes(raw)

    def test_legacy_mac_worker_and_receipt_remain_compatible_without_target_fields(self):
        owner = self.owner()
        self.execute(owner)
        receipt = self.case.staging / self.case.job.name / "acquisition.json"
        for path in (receipt, self.case.job / "worker.json"):
            value = json.loads(path.read_bytes())
            value.pop("target")
            path.write_bytes(M.wire(value))
        observed = self.readback(owner)
        self.assertNotIn("target", observed)
        self.assertEqual(observed["receipt_sha256"], M.sha(receipt.read_bytes()))
        runtime = self.case.job / "runtime.json"
        value = json.loads(runtime.read_bytes())
        value["target"] = ["cpython", 3, 11, "linux", "x86_64"]
        runtime.write_bytes(M.wire(value))
        with self.assertRaisesRegex(B.SetupError, "python_acquisition_readback_unconfirmed"):
            self.readback(owner)

    def test_explicit_mac_profile_cannot_drop_target_evidence(self):
        owner = self.owner("macos-arm64")
        self.execute(owner)
        receipt = self.case.staging / self.case.job.name / "acquisition.json"
        value = json.loads(receipt.read_bytes())
        value.pop("target")
        receipt.write_bytes(M.wire(value))
        with self.assertRaisesRegex(B.SetupError, "python_acquisition_readback_unconfirmed"):
            self.readback(owner)

    def test_runtime_change_during_readback_is_unconfirmed(self):
        owner = self.owner("linux-x86_64")
        self.execute(owner)
        read_file = self.case.io.read_file
        runtime = self.case.job / "runtime.json"

        def changed(root, name, *args):
            value = read_file(root, name, *args)
            if name == "artifact.whl":
                runtime.write_bytes(M.wire({"target": ["cpython", 3, 11, "darwin", "arm64"], "stdlib": []}))
            return value

        with patch.object(self.case.io, "read_file", changed):
            with self.assertRaisesRegex(B.SetupError, "python_acquisition_readback_unconfirmed"):
                self.readback(owner)

    def test_linux_rss_is_compared_in_bytes_at_existing_monitor_boundary(self):
        limit = M.PIP_RSS_LIMIT
        self.assertEqual(M._rss_bytes(limit // 1024, "linux"), limit)
        self.assertGreater(M._rss_bytes(limit // 1024 + 1, "linux"), limit)
        self.assertLess(M._rss_bytes(limit // 1024, "darwin"), limit)
        self.assertEqual(M._rss_bytes(limit, "darwin"), limit)

    def test_real_worker_refuses_wrong_native_target_before_opening_pip(self):
        actual = (sys.platform, platform.machine())
        other = "linux-x86_64" if actual == ("darwin", "arm64") else "macos-arm64"
        owner = self.owner(other)
        runner = B.Runner(self.case.workspace)
        with self.assertRaisesRegex(B.SetupError, "python_acquisition_runtime_unsupported"):
            self.case.execute(owner, runner=runner.run)
        self.assertFalse((self.case.job / "runtime.json").exists())
        self.assertEqual(self.case.fetches, [])

    def test_linux_worker_refuses_32_bit_process_before_opening_pip(self):
        settings = {"target": "linux-x86_64"}
        with patch.object(M, "_worker_settings", return_value=settings), \
                patch.object(M.sys, "platform", "linux"), \
                patch("platform.machine", return_value="x86_64"), \
                patch("struct.calcsize", return_value=4), \
                self.assertRaises(SystemExit) as stopped:
            M._worker("not opened", "unused")
        self.assertEqual(stopped.exception.code, 73)

    @unittest.skipUnless(sys.implementation.name == "cpython" and sys.version_info[:2] == (3, 11)
                         and (sys.platform, platform.machine()) in M.TARGETS.values(), "requires a supported native CPython 3.11")
    def test_real_worker_binds_current_native_target_and_private_environment(self):
        target = next(name for name, actual in M.TARGETS.items() if actual == (sys.platform, platform.machine()))
        sentinel = self.case.root / "trusted-pip-entered"
        script = ("import os,pathlib,sys\n"
                  "assert '--dry-run' in sys.argv and '--only-binary=:all:' in sys.argv\n"
                  "assert os.environ['PIP_CONFIG_FILE']=='/dev/null'\n"
                  "assert 'PRIVATE_TEST_SECRET' not in os.environ\n"
                  "pathlib.Path(" + repr(str(sentinel)) + ").write_text('entered')\n"
                  "pathlib.Path(sys.argv[sys.argv.index('--report')+1]).write_text(" + repr(json.dumps(self.case.report)) + ")\n")
        with zipfile.ZipFile(self.case.pip, "w") as archive:
            for name in ("pip/__init__.py", "pip/_vendor/__init__.py", "pip/_vendor/packaging/__init__.py"):
                archive.writestr(name, "")
            archive.writestr("pip/_vendor/packaging/version.py", "class Version(str): pass\n")
            archive.writestr("__main__.py", script)
        self.case.profile["pip"]["sha256"] = M.sha(self.case.pip.read_bytes())
        owner = self.owner(target)
        with patch.dict(os.environ, {"PRIVATE_TEST_SECRET": "never inherited"}):
            result = self.case.execute(owner, runner=B.Runner(self.case.workspace).run)
        self.assertTrue(sentinel.is_file())
        self.assertEqual(result["target"], target)
        self.assertEqual(self.readback(owner), result)


if __name__ == "__main__":
    unittest.main()
