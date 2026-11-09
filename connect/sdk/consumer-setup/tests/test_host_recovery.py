"""Ordinary host recovery uses retained requests without replaying setup."""
import asyncio
import copy
import json
from pathlib import Path
import shutil
import subprocess
import sys
import unittest
from unittest.mock import patch

import test_prepare_host as preparation
import test_acquisition_recovery as acquisitions
import test_bootstrap_host as bootstraps
import test_recovery_selection as recoveries
import test_selected_setup as selected
from test_recovery_selection import manifest

host = preparation.host


class HostRecoveryTests(unittest.TestCase):
    def setUp(self):
        self.fixture = preparation.PreparationTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.args = self.fixture.args
        self.args.families = ""
        self.args.node = self.args.node_sha256 = None
        self.args.npm_cli = self.args.npm_cli_sha256 = None
        self.case = acquisitions.AcquisitionRecoveryTests()
        self.addCleanup(self.case.doCleanups)
        self.acquired, self.original, _ = self.case.publication("python")
        values = manifest(self.fixture.base / "recover.json", self.original, [self.original.job["id"]])
        for name, value in values.items():
            setattr(self.args, name, str(value))

    def test_acquisition_recovery_without_old_tools_or_selected_profile(self):
        self.acquired.profile_path.unlink()
        self.acquired.pip.unlink()
        before = acquisitions.inventory(self.original.workspace)
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no execution during preparation")):
            result = host.prepare(self.args)
        output = Path(result["directory"])
        receipt = json.loads((output / "prepared.json").read_bytes())
        self.assertEqual(receipt["recovery"]["manifest_sha256"], self.args.recover_manifest_sha256)
        argv = json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertEqual(argv[argv.index("--recover-manifest") + 1], str(output / "recover-manifest.json"))
        self.assertNotIn("--selected-install-profile", argv)
        self.assertNotIn("--python-acquisition-profile", argv)
        self.assertEqual(before, acquisitions.inventory(self.original.workspace))
        self.assertFalse(Path(self.args.workspace).exists())

    def refuse(self, code):
        with self.assertRaisesRegex(Exception, code):
            host.prepare(self.args)
        self.assertFalse(Path(self.args.output).exists())
        self.assertFalse(Path(self.args.workspace).exists())

    def test_pair_hash_and_unsupported_authority_refused(self):
        baseline = copy.copy(self.args)
        for changes, code in (({"recover_manifest_sha256": None}, "recovery_manifest_pair_required"),
                              ({"recover_manifest": None}, "recovery_manifest_pair_required"),
                              ({"recover_manifest_sha256": "0" * 64}, "recovery_manifest_changed"),
                              ({"allow_public_pypi": True}, "recovery_read_only"),
                              ({"allow_public_npm": True}, "recovery_read_only"),
                              ({"prior_request": "unused"}, "recovery_read_only"),
                              ({"compose_backend_profile": "unused"}, "recovery_read_only"),
                              ({"mcpb_acquisition_profile": "unused"}, "recovery_read_only"),
                              ({"allow_remote_recipes": True}, "recovery_read_only")):
            with self.subTest(changes=changes):
                self.args = copy.copy(baseline)
                vars(self.args).update(changes)
                self.refuse(code)

    def test_empty_families_still_refused_for_fresh_setup(self):
        self.args.recover_manifest = self.args.recover_manifest_sha256 = None
        self.refuse("invalid_library_families")

    def test_missing_historical_staging_does_not_prevent_readback_configuration(self):
        self.args.staging_root = str(self.acquired.staging)
        shutil.rmtree(self.acquired.staging)
        host.prepare(self.args)
        self.assertFalse(self.acquired.staging.exists())

    def test_staging_alias_is_not_a_new_recovery_binding(self):
        alias = self.fixture.base / "staging-alias"
        alias.symlink_to(self.fixture.staging, target_is_directory=True)
        self.args.staging_root = str(alias)
        self.refuse("invalid_recovery_staging")

    def test_original_workspace_and_manifest_are_not_output_destinations(self):
        self.args.output = str(self.original.workspace / "new-config")
        self.refuse("recovery_overlaps_host_paths")
        self.args.output = str(self.fixture.base / "new-output")
        self.args.workspace = str(self.original.workspace / "new-workspace")
        self.refuse("recovery_overlaps_host_paths")
        self.args.workspace = str(self.fixture.base / "new-workspace")
        selected_path = self.fixture.base / "reserved" / "manifest.json"
        # The alias parent is absent. Resolve neither it nor the protected file
        # into a writable publication path.
        self.args.output = str(self.fixture.base / "RESERVED")
        self.args.recover_manifest = str(selected_path)
        with self.assertRaises(Exception):
            host.prepare(self.args)
        self.assertFalse(Path(self.args.output).exists())

    def test_anchor_change_after_validation_refuses_publication(self):
        anchor = self.original.workspace / "requests" / (self.original.job["id"] + ".json")
        original_recheck = host.recheck_pins
        def change(pins):
            original_recheck(pins)
            anchor.chmod(0o600)
            anchor.write_bytes(b"{}\n")
        with patch.object(host, "recheck_pins", side_effect=change):
            self.refuse("recovery_request_changed")

    def library_pair(self):
        case = recoveries.RecoverySelectionTests()
        case.setUp()
        self.addCleanup(case.tearDown)
        original, identifiers = case.pair(families=("services",), node=None, npm_cli=None)
        for path in case.bundle.rglob("*"):
            if path.is_file():
                self.fixture.files["consumers/" + str(path.relative_to(case.bundle))] = (path.read_bytes(), 0o644)
        self.fixture.publish_release()
        self.args.families = "services"
        self.args.python = str(Path(sys.executable).resolve())
        self.args.python_sha256 = host.digest(Path(self.args.python).read_bytes())
        values = manifest(self.fixture.base / "pair.json", original, identifiers)
        vars(self.args).update({k: str(v) for k, v in values.items()})
        return original, identifiers

    def test_two_configured_requests_keep_exact_binding_without_installation(self):
        original, identifiers = self.library_pair()
        before = acquisitions.inventory(original.workspace)
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no effect")):
            result = host.prepare(self.args)
        receipt = json.loads((Path(result["directory"]) / "prepared.json").read_bytes())
        self.assertEqual(receipt["recovery"]["requests"], 2)
        self.assertEqual(before, acquisitions.inventory(original.workspace))
        self.assertEqual(len(identifiers), 2)

    def test_configured_runtime_and_frozen_configuration_mismatch(self):
        original, _ = self.library_pair()
        baseline = copy.copy(self.args)
        self.args.python = str(self.fixture.runtime)
        self.args.python_sha256 = host.digest(self.fixture.runtime.read_bytes())
        self.refuse("recovery_selection_mismatch")
        self.args = baseline
        path = next((original.workspace / "configurations").glob("*.json"))
        path.chmod(0o600)
        path.write_bytes(b"{}\n")
        self.refuse("recovery_configuration_mismatch")

    def test_selected_request_uses_current_profile_and_exact_retained_proposal(self):
        # Generate the original profile using the same ordinary host path.
        self.args.recover_manifest = self.args.recover_manifest_sha256 = None
        self.args.families = "services"
        first = host.prepare(self.args)
        profile = Path(first["directory"]) / "selected-install.json"
        fixture = selected.SelectedTests()
        fixture.setUp()
        self.addCleanup(fixture.tearDown)
        fixture.staging = self.fixture.staging
        original = fixture.controller(python=self.args.python, node=None, npm_cli=None,
            selected_install_profile=profile, selected_install_profile_sha256=host.digest(profile.read_bytes()))
        checked = original.selected_check(fixture.stage())
        job = original.selected_install(fixture.selection(checked))["job"]
        original.join()
        self.assertEqual(original.jobs[job["id"]]["status"], "succeeded")
        values = manifest(self.fixture.base / "selected.json", original, [job["id"]])
        vars(self.args).update({k: str(v) for k, v in values.items()})
        self.args.output = str(self.fixture.base / "selected-recovery")
        self.args.families = ""
        host.prepare(self.args)
        self.args.output = str(self.fixture.base / "refused-selected-recovery")
        path = original.workspace / "selections" / checked["validation_id"] / "package.json"
        path.chmod(0o600)
        path.write_bytes(b"{}\n")
        self.refuse("selected_proposal_changed")

    def test_actual_generated_stdio_is_readback_only_and_original_argv_stays_one_shot(self):
        self.fixture.files["setup/mcp_adapter.py"] = ((preparation.SOURCE / "mcp_adapter.py").read_bytes(), 0o644)
        files = {"install-library-consumer.py": {"sha256": host.digest(
            self.fixture.files["consumers/install-library-consumer.py"][0])}}
        self.fixture.files["consumers/CONTENTS.json"] = (host.encoded({"files": files}), 0o644)
        self.fixture.publish_release()
        self.args.python = str(Path(sys.executable).resolve())
        self.args.mcp_python = sys.executable
        self.args.python_sha256 = self.args.mcp_python_sha256 = host.digest(Path(self.args.python).read_bytes())
        host.prepare(self.args)
        entry = json.loads((Path(self.args.output) / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]
        async def exercise():
            from mcp import ClientSession, StdioServerParameters
            from mcp.client.stdio import stdio_client
            async with stdio_client(StdioServerParameters(**entry)) as (reader, writer):
                async with ClientSession(reader, writer) as client:
                    await client.initialize()
                    self.assertEqual({t.name for t in (await client.list_tools()).tools},
                                     {"setup_recovery_manifest", "setup_state", "setup_reconcile", "setup_job", "setup_jobs"})
                    state = await client.call_tool("setup_state", {})
                    value = json.loads(state.content[0].text)
                    self.assertEqual(value["mode"], "recovery")
                    self.assertEqual(value["job"]["status"], "unconfirmed")
                    observed = await client.call_tool("setup_reconcile", {"job_id": self.original.job["id"]})
                    self.assertEqual(json.loads(observed.content[0].text)["observation"]["status"], "matched")
                    refused = await client.call_tool("setup_selected_install", {})
                    self.assertTrue(refused.is_error)
        async def bounded():
            async with asyncio.timeout(20):
                await exercise()
        asyncio.run(bounded())
        rerun = subprocess.run([entry["command"], *entry["args"]], input=b"", capture_output=True, timeout=10)
        self.assertNotEqual(rerun.returncode, 0)
        self.assertIn(b"preserve the workspace", rerun.stderr)


class BootstrapRecoveryTests(unittest.TestCase):
    def setUp(self):
        self.fixture = bootstraps.BootstrapTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.args = self.fixture.args
        self.args.families = ""
        self.case = acquisitions.AcquisitionRecoveryTests()
        self.addCleanup(self.case.doCleanups)
        _, self.original, _ = self.case.publication("python")
        values = manifest(self.fixture.base / "recover.json", self.original, [self.original.job["id"]])
        vars(self.args).update({k: str(v) for k, v in values.items()})

    def test_bootstrap_creates_client_then_exact_recovery_configuration(self):
        before = acquisitions.inventory(self.original.workspace)
        result = bootstraps.bootstrap.bootstrap(self.args)
        receipt = json.loads((Path(result["host"]["directory"]) / "prepared.json").read_bytes())
        self.assertEqual(receipt["recovery"]["mode"], "readback_only")
        self.assertEqual(before, acquisitions.inventory(self.original.workspace))
        self.assertFalse(Path(self.args.workspace).exists())

    def test_original_workspace_is_protected_against_real_client_destination(self):
        self.args.client_directory = str(self.original.workspace / "client")
        with patch.object(bootstraps.bootstrap, "run_owner", side_effect=AssertionError("no install")):
            with self.assertRaisesRegex(Exception, "recovery_overlaps_host_paths"):
                bootstraps.bootstrap.bootstrap(self.args)

    def test_old_preparer_cannot_silently_ignore_recovery_before_install(self):
        raw = self.fixture.files["setup/prepare_host.py"][0]
        raw += (b"\n_current_prepare = prepare\ndef prepare(args):\n"
                b"    args = SimpleNamespace(**vars(args))\n"
                b"    args.recover_manifest = args.recover_manifest_sha256 = None\n"
                b"    args.families = 'services'\n"
                b"    return _current_prepare(args)\n")
        self.fixture.files["setup/prepare_host.py"] = (raw, 0o644)
        self.fixture.publish_release()
        with patch.object(bootstraps.bootstrap, "run_owner", side_effect=AssertionError("no install")):
            with self.assertRaisesRegex(Exception, "recovery_not_prepared"):
                bootstraps.bootstrap.bootstrap(self.args)
        self.assertFalse(Path(self.args.client_directory).exists())

    def test_mutation_after_preview_is_refused_before_client_install(self):
        real_recheck = bootstraps.bootstrap.host.recheck_release
        def change(*args):
            real_recheck(*args)
            Path(self.args.recover_manifest).write_bytes(b"{}\n")
        with patch.object(bootstraps.bootstrap.host, "recheck_release", side_effect=change), \
                patch.object(bootstraps.bootstrap, "run_owner", side_effect=AssertionError("no install")):
            with self.assertRaisesRegex(Exception, "recovery_manifest_changed"):
                bootstraps.bootstrap.bootstrap(self.args)
        self.assertFalse(Path(self.args.client_directory).exists())


if __name__ == "__main__":
    unittest.main()
