"""Two host-owned Node pairs share one Controller without runtime substitution.

Package and resolver outputs are controlled fixtures. These tests establish
dispatch, custody and recovery bindings, not real Node compatibility.
"""
import copy
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

import test_backend as library
import test_node_acquisition as acquisition
from test_selected_mcpb import McpbFixture
from test_selected_setup import SelectedFixture, SelectedRunner, adapter, backend


def argument(argv, flag):
    return argv[argv.index(flag) + 1]


class MixedRunner(SelectedRunner):
    def run(self, argv, capture=False):
        if any(flag in argv for flag in ("--package", "--selected-node", "--selected-python")):
            return super().run(argv, capture=capture)
        delegate = library.FakeRunner(self.workspace)
        delegate.calls = self.calls
        return delegate.run(argv, capture=capture)


class SplitRuntimeTests(SelectedFixture, unittest.TestCase):
    def setUp(self):
        super().setUp()
        self.pair = {}
        for name in ("selected_node", "selected_npm_cli"):
            path = self.root / name
            path.write_text("inert distinct " + name + "\n")
            path.chmod(0o700)
            self.pair[name] = str(path)

    def test_partial_empty_or_unused_pair_fails_before_workspace(self):
        for changes in ({"selected_node": self.pair["selected_node"]},
                        {"selected_npm_cli": self.pair["selected_npm_cli"]},
                        {"selected_node": "", "selected_npm_cli": self.pair["selected_npm_cli"]},
                        {"selected_node": self.pair["selected_node"], "selected_npm_cli": ""}):
            with self.subTest(changes=changes):
                workspace = self.root / "refused"
                with self.assertRaisesRegex(backend.SetupError, "invalid_runtime"):
                    self.controller(workspace=workspace, **changes)
                self.assertFalse(workspace.exists())
        self.profile["validators"] = {"python": self.tools["python"]}
        self.write_profile()
        with self.assertRaisesRegex(backend.SetupError, "invalid_runtime"):
            self.controller(workspace=self.root / "unused", **self.pair)
        self.assertFalse((self.root / "unused").exists())

    def test_omitted_override_preserves_exact_legacy_binding(self):
        c = self.controller()
        expected = str(Path(sys.executable).resolve())
        self.assertEqual(c.selected_binding["runtimes"],
                         {"python": expected, "node": expected, "npm_cli": expected})
        self.assertEqual((c.selected_node, c.selected_npm_cli), (c.node, c.npm_cli))

    def test_one_session_keeps_library_and_selected_dispatch_distinct(self):
        fixture = library.FixtureMixin()
        fixture.setUp()
        self.addCleanup(fixture.tearDown)
        c = self.controller(bundle=fixture.bundle, contents_sha256=fixture.sha,
                            families=backend.FAMILIES, runner_factory=MixedRunner, **self.pair)
        c.prepare({"family": "activepieces"})
        c.join()
        self.assertEqual(c.job["status"], "succeeded", c.job)
        request = {"family": "activepieces", "profile": "test-write", "operations": ["test_write"],
                   "values": {"token_env": "TEST_ONLY_TOKEN", "timeout_ms": 5000}}
        checked = c.check(request)
        job = c.install({**request, "validation_id": checked["validation_id"],
                         "configuration_sha256": checked["configuration_sha256"]})["job"]
        c.join()
        self.assertEqual(c.job["status"], "succeeded", c.job)
        record = backend.decode((c.workspace / "requests" / (job["id"] + ".json")).read_bytes())
        self.assertEqual(record["runtimes"], {"python": c.python, "node": c.node, "npm_cli": c.npm_cli})
        checked = c.selected_check(self.stage("node"))
        c.selected_install(self.selection(checked))
        c.join()
        self.assertEqual(c.job["status"], "succeeded", c.job)
        self.assertEqual(set(c.families), set(backend.FAMILIES))
        for argv, _ in c.runner.calls:
            if "--selected-node" in argv:
                self.assertEqual(argument(argv, "--node"), self.pair["selected_node"])
                if "--verify-existing" not in argv:
                    self.assertEqual(argument(argv, "--npm-cli"), self.pair["selected_npm_cli"])
            elif "--family" in argv:
                self.assertEqual(argument(argv, "--node"), c.node)
                self.assertEqual(argument(argv, "--npm-cli"), c.npm_cli)
            elif "profiles" in argv or "config-help" in argv or "check-config" in argv:
                self.assertEqual(argv[0], c.node)
        expected = c.expected_command("activepieces", self.root / "installed")
        self.assertEqual(expected[0], c.node)

    def test_split_selected_only_needs_no_library_node(self):
        c = self.controller(node=None, npm_cli=None, **self.pair)
        checked = c.selected_check(self.stage("node"))
        c.selected_install(self.selection(checked))
        c.join()
        self.assertEqual(c.job["status"], "succeeded", c.job)
        self.assertIsNone(c.node)
        self.assertEqual(c.selected_binding["runtimes"]["node"], self.pair["selected_node"])

    def test_selected_pair_cannot_satisfy_missing_library_runtime(self):
        with self.assertRaisesRegex(backend.SetupError, "invalid_runtime"):
            self.controller(families=("activepieces",), node=None, npm_cli=None,
                            workspace=self.root / "missing-library", **self.pair)
        self.assertFalse((self.root / "missing-library").exists())

    def test_recovery_binds_selected_pair_and_ignores_changed_library_pair(self):
        for profile in ("node", "python"):
            with self.subTest(profile=profile):
                c = self.controller(**self.pair)
                checked = c.selected_check(self.stage(profile, profile))
                job = c.selected_install(self.selection(checked))["job"]
                c.join()
                args = dict(recover_workspace=c.workspace, recover_request=job["id"],
                            recover_request_sha256=job["recovery_request"]["sha256"])
                recovered = self.controller(node=None, npm_cli=None, **args, **self.pair)
                observation = recovered.reconcile({"job_id": job["id"]})["observation"]
                self.assertEqual(observation["status"], "matched", observation)
                self.assertEqual(recovered.job["status"], "unconfirmed")
                self.assertNotIn("result", recovered.job)
                for changes in ({}, {**self.pair, "selected_node": sys.executable},
                                {**self.pair, "selected_npm_cli": sys.executable}):
                    with self.subTest(changes=changes), self.assertRaisesRegex(
                            backend.SetupError, "selected_proposal_changed"):
                        self.controller(**args, **changes)
                    self.assertFalse((self.root / ("work-" + str(self.sequence))).exists())

    def test_retry_and_replay_keep_original_selected_pair(self):
        c = self.controller(**self.pair)
        checked = c.selected_check(self.stage("node"))
        c.runner.nonzero = True
        job = c.selected_install(self.selection(checked))["job"]
        c.join()
        self.assertEqual(c.job["status"], "unconfirmed")
        calls = len(c.runner.calls)
        self.assertEqual(c.selected_install(self.selection(checked))["job"]["id"], job["id"])
        self.assertEqual(len(c.runner.calls), calls)
        original = copy.deepcopy(c.selected_binding)
        c.selected_binding["runtimes"]["node"] = c.node
        with self.assertRaisesRegex(backend.SetupError, "selected_proposal_changed"):
            c.selected_retry({"job_id": job["id"]})
        c.selected_binding = original
        retry = c.selected_retry({"job_id": job["id"]})
        self.assertEqual(c.selected_retry({"job_id": job["id"]}), retry)
        c.runner.nonzero = False
        c.selected_install(self.selection(retry))
        c.join()
        self.assertEqual(c.job["status"], "succeeded", c.job)
        for argv, _ in c.runner.calls:
            if "--selected-node" in argv:
                self.assertEqual(argument(argv, "--node"), self.pair["selected_node"])

    def test_cli_entrypoints_forward_pair_to_same_controller(self):
        argv = ["--workspace", str(self.root / "cli"), "--families", "", "--python", sys.executable,
                "--selected-node", self.pair["selected_node"], "--selected-npm-cli", self.pair["selected_npm_cli"]]
        for entry in (backend.main, adapter.main):
            with self.subTest(entry=entry), patch.object(backend, "Controller", side_effect=backend.SetupError("fixture_stop")) as create, \
                    patch("builtins.print"):
                self.assertEqual(entry(argv), 2)
                self.assertEqual(create.call_args.kwargs["selected_node"], self.pair["selected_node"])
                self.assertEqual(create.call_args.kwargs["selected_npm_cli"], self.pair["selected_npm_cli"])


class SplitMcpbTests(McpbFixture, unittest.TestCase):
    def test_mcpb_override_and_legacy_no_npm_dispatch_and_recovery(self):
        node = self.root / "selected-node"
        node.write_text("inert selected Node\n")
        node.chmod(0o700)
        for index, pair in enumerate(({}, {"selected_node": str(node), "selected_npm_cli": sys.executable})):
            with self.subTest(pair=pair):
                c = self.controller(**pair)
                expected = pair.get("selected_node", c.node)
                checked = c.selected_check(self.stage(name="mcpb-" + str(index)))
                job = c.selected_install(self.selection(checked))["job"]
                c.join()
                self.assertEqual(c.job["status"], "succeeded", c.job)
                recovered = self.controller(recover_workspace=c.workspace, recover_request=job["id"],
                    recover_request_sha256=job["recovery_request"]["sha256"], **pair)
                self.assertEqual(recovered.reconcile({"job_id": job["id"]})["observation"]["status"], "matched")
                for argv, _ in c.runner.calls + recovered.runner.calls:
                    if argv[1] != "check":
                        self.assertEqual(argument(argv, "--node"), expected)
                c.runner.proof_mutator = lambda value: value["runtime"].update(executable="/wrong/node")
                self.assertEqual(c.reconcile({"job_id": job["id"]})["observation"]["status"], "unconfirmed")


class SplitAcquisitionTests(unittest.TestCase):
    def fixture(self):
        case = acquisition.NodeAcquisitionTests()
        case.setUp()
        self.addCleanup(case.doCleanups)
        case.profile["target"] = "linux-x86_64"
        case.runtime = {"platform": "linux", "arch": "x64", "version": "v24.19.0"}
        case.profile_path.write_bytes(backend.encoded(case.profile))
        validator = case.root / "validator"
        validator.write_text("inert Node planner\n")
        validator.chmod(0o700)
        selected = case.root / "selected.json"
        selected.write_bytes(backend.encoded({"schema_version": 1, "scope": backend.SELECTED_SCOPE,
            "staging_root": str(case.staging),
            "installer": {"path": str(case.installer), "sha256": backend.digest(case.installer.read_bytes())},
            "validators": {"node": {"path": str(validator), "sha256": backend.digest(validator.read_bytes())}}}))
        args = dict(selected_install_profile=selected, selected_install_profile_sha256=backend.digest(selected.read_bytes()),
                    node_acquisition_profile=case.profile_path,
                    node_acquisition_profile_sha256=backend.digest(case.profile_path.read_bytes()),
                    selected_node=str(case.node), selected_npm_cli=sys.executable)
        return case, args

    def test_acquisition_uses_selected_pin_and_inert_recovery_needs_no_runtime(self):
        case, args = self.fixture()
        c = backend.Controller(None, None, case.root / "setup", (), sys.executable, sys.executable, sys.executable, **args)
        self.addCleanup(c.join)
        self.assertEqual(c.node_acquisition.node, case.node)
        run = c.node_acquisition.run
        def controlled(request, directory, runner, check_stop):
            case.job = directory
            return run(request, directory, case.runner, check_stop)
        c.node_acquisition.run = controlled
        job = c.acquire_node(case.request)["job"]
        c.join()
        self.assertEqual(c.job["status"], "succeeded", c.job)
        self.assertEqual(c.acquire_node(case.request)["job"]["id"], job["id"])
        self.assertEqual(len(case.calls), 5)
        anchor = job["recovery_request"]
        case.node.unlink()
        case.npm.unlink()
        case.profile_path.unlink()
        with patch.object(subprocess, "Popen", side_effect=AssertionError("inert recovery")):
            recovered = backend.Controller(None, None, case.root / "recovered", (), sys.executable,
                "/absent/library-node", "/absent/library-npm", selected_node="/absent/selected-node",
                selected_npm_cli="/absent/selected-npm", recover_workspace=c.workspace,
                recover_request=anchor["id"], recover_request_sha256=anchor["sha256"])
            self.addCleanup(recovered.join)
            observation = recovered.reconcile({"job_id": job["id"]})["observation"]
        self.assertEqual(observation["status"], "matched", observation)
        self.assertEqual(observation["result"], c.job["result"])
        self.assertEqual(recovered.job["status"], "unconfirmed")
        self.assertNotIn("result", recovered.job)

    def test_acquisition_cannot_use_library_pin_instead_of_selected_pin(self):
        case, args = self.fixture()
        args["selected_node"] = sys.executable
        with self.assertRaisesRegex(backend.SetupError, "invalid_node_acquisition_profile"):
            backend.Controller(None, None, case.root / "refused", (), sys.executable,
                               str(case.node), sys.executable, **args)
        self.assertFalse((case.root / "refused").exists())
        self.assertEqual(case.calls, [])

    def test_stop_after_selected_runtime_probe_prevents_network_and_replay(self):
        case, args = self.fixture()
        c = backend.Controller(None, None, case.root / "setup", (), sys.executable, sys.executable, sys.executable, **args)
        self.addCleanup(c.join)
        run = c.node_acquisition.run
        def controlled(request, directory, runner, check_stop):
            case.job = directory
            return run(request, directory, case.runner, check_stop)
        c.node_acquisition.run = controlled
        def stop(stage):
            self.assertEqual(stage, "--probe")
            c.stop({"job_id": c.job["id"]})
        case.after_stage = stop
        job = c.acquire_node(case.request)["job"]
        c.join()
        self.assertEqual(c.job["status"], "unconfirmed", c.job)
        self.assertEqual(c.job["error"], "operation_stopped")
        self.assertEqual(c.job["stop"]["status"], "settled")
        self.assertEqual([argv[-3] for argv, _ in case.calls], ["--probe"])
        self.assertEqual(c.acquire_node(case.request)["job"]["id"], job["id"])
        self.assertEqual(len(case.calls), 1)
        self.assertFalse((case.staging / job["id"] / "acquisition.json").exists())


if __name__ == "__main__":
    unittest.main()
