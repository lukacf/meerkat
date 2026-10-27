"""Explicit acquisition targets without narrowing the neutral Node installer."""
import copy
import json
import os
from pathlib import Path
import platform
import resource
import struct
import subprocess
import sys
import unittest
from unittest.mock import patch

import test_node_acquisition as base
from test_acquisition_owner_readback import envelope, snapshot
from test_selected_setup import backend


class NodeTargetTests(unittest.TestCase):
    def fixture(self, target=None):
        case = base.NodeAcquisitionTests()
        case.setUp()
        self.addCleanup(case.doCleanups)
        if target is not None:
            case.profile["target"] = target
        if target == "linux-x86_64":
            case.runtime = {"platform": "linux", "arch": "x64", "version": "v24.19.0"}
        return case

    def test_invalid_target_refused_before_binding(self):
        for target in ("linux-arm64", "Linux", "", None, True, {}, []):
            with self.subTest(target=target):
                case = self.fixture()
                case.profile["target"] = target
                with self.assertRaisesRegex(base.B.SetupError, "invalid_node_acquisition_profile"):
                    case.owner()
                self.assertFalse((case.workspace / "node-acquisition").exists())

    def test_agent_cannot_override_target(self):
        case = self.fixture("linux-x86_64")
        owner = case.owner()
        with self.assertRaises(base.B.SetupError):
            owner.arguments({**case.request, "target": "macos-arm64"})
        self.assertEqual(case.calls, [])

    def test_mac_omitted_and_explicit_preserve_receipt_shape(self):
        for target in (None, "macos-arm64"):
            with self.subTest(target=target):
                case = self.fixture(target)
                owner = case.owner()
                result = case.execute(owner)
                self.assertNotIn("target", result)
                self.assertEqual(owner.summary()["target"], "Node 24 macOS arm64")
                record = envelope(owner, "node", case.request, case.job)
                self.assertEqual(base.M.readback(case.io, record, case.job), result)
                # Earlier workers had neither proof and omitted target in settings.
                (case.job / "probe.json").unlink()
                (case.job / "preflight.json").unlink()
                settings = json.loads((case.job / "resolve.json").read_bytes())
                del settings["target"]
                (case.job / "resolve.json").write_bytes(base.M.wire(settings))
                self.assertEqual(base.M.readback(case.io, record, case.job), result)

    def test_linux_receipt_and_readback_bind_frozen_target_without_execution(self):
        case = self.fixture("linux-x86_64")
        owner = case.owner()
        result = case.execute(owner)
        self.assertEqual(result["target"], "linux-x86_64")
        self.assertEqual(owner.summary()["target"], "Node 24 Linux x86_64")
        self.assertEqual(result["runtime"], case.runtime)
        record = envelope(owner, "node", case.request, case.job)
        case.profile_path.unlink()
        case.node.unlink()
        case.npm.unlink()
        before = snapshot(case.workspace)
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no readback execution")):
            self.assertEqual(base.M.readback(case.io, record, case.job), result)
        self.assertEqual(snapshot(case.workspace), before)

    def test_preflight_mismatch_refuses_before_any_fetch(self):
        for runtime in ({"platform": "darwin", "arch": "arm64", "version": "v24.19.0"},
                        {"platform": "linux", "arch": "arm64", "version": "v24.19.0"},
                        {"platform": "linux", "arch": "x64", "version": "v24.14.0"}):
            with self.subTest(runtime=runtime):
                case = self.fixture("linux-x86_64")
                case.runtime = runtime
                with self.assertRaisesRegex(base.B.SetupError, "node_acquisition_runtime_unsupported"):
                    case.execute()
                self.assertEqual([call[0][-3] for call in case.calls], ["--probe"])
                case.assert_unpublished()

    def test_preflight_failure_is_fixed_without_fetch(self):
        case = self.fixture("linux-x86_64")
        with self.assertRaisesRegex(base.B.SetupError, "^node_acquisition_runtime_unsupported$"):
            case.execute(runner=lambda *args, **kwargs: (73, b"private runtime failure"))
        case.assert_unpublished()

    def test_final_runtime_must_match_preflight(self):
        case = self.fixture("linux-x86_64")
        def after(stage):
            if stage == "--probe":
                case.runtime["version"] = "v24.20.0"
        case.after_stage = after
        with self.assertRaises(base.B.SetupError):
            case.execute()
        self.assertEqual([call[0][-3] for call in case.calls], ["--probe", "--fetch", "--fetch", "--resolve"])
        case.assert_unpublished()

    def test_changed_proof_after_dependency_fetch_never_publishes(self):
        for name in ("probe.json", "resolve.json", "preflight.json", "runtime.json"):
            with self.subTest(name=name):
                case = self.fixture("linux-x86_64")
                def after(stage):
                    if len(case.calls) == 5:
                        (case.job / name).write_bytes(b"{}\n")
                case.after_stage = after
                with self.assertRaises(base.B.SetupError):
                    case.execute()
                case.assert_unpublished()

    def test_readback_rejects_missing_or_changed_target_proofs(self):
        for mutation in ("receipt-target", "missing-target", "runtime", "both-runtimes", "resolve-target",
                         "resolve-pin", "probe-path", "missing-probe", "missing-preflight"):
            with self.subTest(mutation=mutation):
                case = self.fixture("linux-x86_64")
                owner = case.owner()
                case.execute(owner)
                record = envelope(owner, "node", case.request, case.job)
                receipt_path = case.staging / case.job.name / "acquisition.json"
                receipt = json.loads(receipt_path.read_bytes())
                if mutation == "receipt-target":
                    receipt["target"] = "macos-arm64"
                elif mutation == "missing-target":
                    del receipt["target"]
                elif mutation in ("runtime", "both-runtimes"):
                    receipt["runtime"]["version"] = "v24.20.0"
                    if mutation == "both-runtimes":
                        (case.job / "runtime.json").write_bytes(base.M.wire(receipt["runtime"]))
                elif mutation.startswith("resolve") or mutation == "probe-path":
                    path = case.job / ("probe.json" if mutation == "probe-path" else "resolve.json")
                    value = json.loads(path.read_bytes())
                    key, replacement = {"resolve-target": ("target", "macos-arm64"),
                                        "resolve-pin": ("node_sha256", "0" * 64),
                                        "probe-path": ("directory", "/elsewhere")}[mutation]
                    value[key] = replacement
                    path.write_bytes(base.M.wire(value))
                else:
                    (case.job / ("probe.json" if mutation == "missing-probe" else "preflight.json")).unlink()
                receipt_path.write_bytes(base.M.wire(receipt))
                with self.assertRaisesRegex(base.B.SetupError, "node_acquisition_readback_unconfirmed"):
                    base.M.readback(case.io, record, case.job)

    def test_linux_worker_checks_native_python_before_node_or_npm(self):
        for system, machine, width in (("darwin", "arm64", 8), ("linux", "aarch64", 8), ("linux", "x86_64", 4)):
            with self.subTest(system=system, machine=machine, width=width):
                for worker in (base.M.probe_worker, base.M.resolve_worker):
                    with patch.object(sys, "platform", system), patch.object(platform, "machine", return_value=machine), \
                            patch.object(struct, "calcsize", return_value=width), patch.object(os, "execve") as execution:
                        with self.assertRaises(ValueError):
                            worker({"target": "linux-x86_64"})
                        execution.assert_not_called()

    def test_linux_workers_exec_guarded_node_with_no_ambient_options(self):
        case = self.fixture("linux-x86_64")
        owner = case.owner()
        for name in ("home", "tmp", "cache", "project"):
            (case.job / name).mkdir()
        settings = {"node": str(case.node), "node_sha256": base.M.sha(case.node.read_bytes()),
                    "npm": str(owner.assets / "npm"), "directory": str(case.job), "target": owner.target}
        for worker in (base.M.probe_worker, base.M.resolve_worker):
            with patch.object(sys, "platform", "linux"), patch.object(platform, "machine", return_value="x86_64"), \
                    patch.object(struct, "calcsize", return_value=8), patch.object(os, "chdir"), \
                    patch.object(resource, "setrlimit"), patch.object(os, "execve") as execution, \
                    patch.dict(os.environ, {"NODE_OPTIONS": "--require=/untrusted", "HTTP_PROXY": "http://private"}):
                worker(copy.deepcopy(settings))
            node, argv, env = execution.call_args.args
            self.assertEqual(Path(node), case.node)
            self.assertIn('process.platform!=="linux"', argv[3])
            self.assertIn('process.arch!=="x64"', argv[3])
            self.assertNotIn("NODE_OPTIONS", env)
            self.assertNotIn("HTTP_PROXY", env)
            if worker is base.M.probe_worker:
                self.assertNotIn(str(owner.assets / "npm/bin/npm-cli.js"), argv)
            else:
                self.assertIn("--ignore-scripts", argv)

    def test_controller_accepts_linux_acquisition_with_neutral_selected_profile_and_recovers(self):
        case = self.fixture("linux-x86_64")
        case.profile_path.write_bytes(backend.encoded(case.profile))
        validator = case.root / "validator"
        validator.write_text("inert validator; must not execute\n")
        validator.chmod(0o700)
        selected = case.root / "selected.json"
        selected.write_bytes(backend.encoded({"schema_version": 1, "scope": backend.SELECTED_SCOPE,
            "staging_root": str(case.staging),
            "installer": {"path": str(case.installer), "sha256": backend.digest(case.installer.read_bytes())},
            "validators": {"node": {"path": str(validator), "sha256": backend.digest(validator.read_bytes())}}}))
        controller = backend.Controller(None, None, case.root / "setup", (), sys.executable, str(case.node), sys.executable,
            selected_install_profile=selected, selected_install_profile_sha256=backend.digest(selected.read_bytes()),
            node_acquisition_profile=case.profile_path,
            node_acquisition_profile_sha256=backend.digest(case.profile_path.read_bytes()))
        self.addCleanup(controller.join)
        owner = controller.node_acquisition
        self.assertEqual(owner.target, "linux-x86_64")
        original_run = owner.run
        def run(request, directory, runner, check_stop):
            case.job = directory
            return original_run(request, directory, case.runner, check_stop)
        owner.run = run
        controller.acquire_node(case.request)
        controller.join()
        self.assertEqual(controller.job["status"], "succeeded", controller.job)
        result = controller.job["result"]
        anchor = controller.job["recovery_request"]
        recovered = backend.Controller(None, None, case.root / "recovered", (), sys.executable, None, None,
            recover_workspace=controller.workspace, recover_request=anchor["id"], recover_request_sha256=anchor["sha256"])
        self.addCleanup(recovered.join)
        with patch.object(subprocess, "Popen", side_effect=AssertionError("readback only")):
            observation = recovered.reconcile({"job_id": controller.job["id"]})["observation"]
        self.assertEqual(observation["status"], "matched", observation)
        self.assertEqual(observation["result"], result)
        self.assertEqual(recovered.job["status"], "unconfirmed")
        self.assertNotIn("result", recovered.job)
        with self.assertRaisesRegex(backend.SetupError, "recovery_read_only"):
            recovered.acquire_node(case.request)


if __name__ == "__main__":
    unittest.main()
