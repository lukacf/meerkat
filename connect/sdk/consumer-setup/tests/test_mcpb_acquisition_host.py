"""MCPB network opt-in is bound before host configuration or bootstrap effects."""
import asyncio
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

import test_prepare_host as preparation_tests
import test_bootstrap_host as bootstrap_tests


host, sha, SOURCE = preparation_tests.host, preparation_tests.sha, preparation_tests.SOURCE
bootstrap = bootstrap_tests.bootstrap


def enable_acquisition(case):
    case.mcpb(inspection=True)
    case.files[host.MCPB_ACQUISITION] = ((SOURCE / "mcpb_acquisition.py").read_bytes(), 0o644)
    case.publish_release()
    profile = {"schema_version": 1, "scope": "trusted_public_mcpb_acquisition",
               "python": {"path": str(Path(case.args.python).resolve()), "sha256": case.args.python_sha256},
               "allowed_origins": ["https://github.com"],
               "allowed_redirect_origins": ["https://release-assets.githubusercontent.com"]}
    path = case.base / "public acquisition.json"
    path.write_text(json.dumps(profile, indent=2) + "\n")
    case.args.mcpb_acquisition_profile = str(path)
    case.args.mcpb_acquisition_profile_sha256 = sha(path.read_bytes())
    return path, profile


def cli_arguments(args):
    result = []
    for key, value in vars(args).items():
        if value is not None and value is not False:
            result.append("--" + key.replace("_", "-"))
            if value is not True:
                result.append(str(value))
    return result


class McpbAcquisitionHostTests(unittest.TestCase):
    setUp = preparation_tests.PreparationTests.setUp
    publish_release = preparation_tests.PreparationTests.publish_release
    mcpb = preparation_tests.PreparationTests.mcpb
    refuse = preparation_tests.PreparationTests.refuse

    def test_profile_pair_required_before_any_preparation(self):
        for index, (profile, digest) in enumerate(((str(self.base / "profile.json"), None),
                                                 (None, "a" * 64), ("", "a" * 64),
                                                 (False, False))):
            with self.subTest(profile=profile, digest=digest):
                self.args.output = str(self.base / ("output-" + str(index)))
                self.args.mcpb_acquisition_profile = profile
                self.args.mcpb_acquisition_profile_sha256 = digest
                self.refuse("mcpb_acquisition_profile_pair_required")

    def test_requires_complete_inspection_route(self):
        enable_acquisition(self)
        self.args.enable_mcpb_inspection = False
        self.refuse("mcpb_acquisition_requires_inspection")
        self.args.enable_mcpb_inspection = True
        self.args.allow_mcpb = False
        self.refuse("mcpb_inspection_requires_mcpb")
        self.args.allow_mcpb = True
        self.args.selected_node = self.args.selected_node_sha256 = None
        self.refuse("mcpb_runtime_required")

    def test_older_archive_remains_usable_without_network_opt_in(self):
        enable_acquisition(self)
        del self.files[host.MCPB_ACQUISITION]
        (self.release / host.MCPB_ACQUISITION).unlink()
        self.publish_release()
        self.refuse("mcpb_acquisition_not_in_release")
        self.args.mcpb_acquisition_profile = self.args.mcpb_acquisition_profile_sha256 = None
        host.prepare(self.args)
        output = Path(self.args.output)
        self.assertFalse((output / "mcpb-acquisition.json").exists())
        self.assertNotIn("--mcpb-acquisition-profile", (output / "mcp.json").read_text())

    def test_cli_retains_exact_profile_and_binds_all_arguments_without_execution(self):
        path, _ = enable_acquisition(self)
        original = path.read_bytes(), path.stat().st_mtime_ns
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no process allowed")), redirect_stdout(io.StringIO()):
            self.assertEqual(host.main(cli_arguments(self.args)), 0)
        output = Path(self.args.output)
        frozen = output / "mcpb-acquisition.json"
        self.assertEqual(frozen.read_bytes(), original[0])
        self.assertEqual(frozen.stat().st_mode & 0o777, 0o600)
        self.assertEqual((path.read_bytes(), path.stat().st_mtime_ns), original)
        receipt = json.loads((output / "prepared.json").read_bytes())
        self.assertEqual(receipt["files"][frozen.name], sha(original[0]))
        self.assertEqual(receipt["runtimes"]["mcpb_acquisition_profile"]["path"], str(path))
        argv = json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertEqual(argv.count("--mcpb-acquisition-profile"), 1)
        self.assertEqual(argv[argv.index("--mcpb-acquisition-profile") + 1], str(frozen))
        self.assertEqual(argv[argv.index("--mcpb-acquisition-profile-sha256") + 1], sha(original[0]))
        self.assertEqual(argv[argv.index("--python") + 1], str(Path(self.args.python).resolve()))
        selected = json.loads((output / "selected-install.json").read_bytes())
        self.assertTrue(selected["mcpb_inspection"])
        self.assertEqual(selected["staging_root"], str(self.staging))
        self.assertFalse(Path(self.args.workspace).exists())
        self.assertEqual(list(self.staging.iterdir()), [])

    def test_canonical_owner_refuses_bad_profile_and_origin_before_publication(self):
        path, valid = enable_acquisition(self)
        for change in ({"schema_version": True}, {"scope": "download_anything"},
                       {"allowed_origins": []}, {"allowed_origins": ["http://github.com"]},
                       {"allowed_origins": ["https://github.com/path"]},
                       {"allowed_origins": ["https://github.com:443"]},
                       {"allowed_redirect_origins": ["https://example.com", "https://example.com"]},
                       {"unknown": "not allowed"}):
            with self.subTest(change=change):
                path.write_text(json.dumps({**valid, **change}))
                self.args.mcpb_acquisition_profile_sha256 = sha(path.read_bytes())
                with self.assertRaisesRegex(Exception, "invalid_mcpb_acquisition_profile"):
                    host.prepare(self.args)
                self.assertFalse(Path(self.args.output).exists())
        path.write_text(json.dumps(valid))
        self.args.mcpb_acquisition_profile_sha256 = "0" * 64
        self.refuse("mcpb_acquisition_profile_changed")

    def test_selected_python_path_must_match_even_when_bytes_match(self):
        path, profile = enable_acquisition(self)
        other = self.base / "other-python"
        other.write_bytes(self.runtime.read_bytes())
        other.chmod(0o755)
        profile["python"]["path"] = str(other)
        path.write_text(json.dumps(profile))
        self.args.mcpb_acquisition_profile_sha256 = sha(path.read_bytes())
        self.refuse("mcpb_acquisition_python_mismatch")

    def test_original_profile_path_is_canonical_and_outside_mutable_host_roots(self):
        path, _ = enable_acquisition(self)
        alias = self.base / "alias-profile"
        alias.symlink_to(path)
        self.args.mcpb_acquisition_profile = str(alias)
        with self.assertRaises(OSError):
            host.prepare(self.args)
        self.assertFalse(Path(self.args.output).exists())
        for selected in (self.staging / "profile.json", Path(self.args.output), Path(self.args.workspace)):
            self.args.mcpb_acquisition_profile = str(selected)
            self.refuse("mcpb_acquisition_profile_overlaps_host_paths")

    def test_original_profile_change_before_publication_refuses(self):
        path, _ = enable_acquisition(self)
        actual = host.recheck_release
        calls = 0
        def changing(*args):
            nonlocal calls
            calls += 1
            actual(*args)
            if calls == 2:
                path.write_text("changed after owner validation")
        with patch.object(host, "recheck_release", side_effect=changing):
            self.refuse("host_input_changed")

    def test_generated_mcp_exposes_acquisition_without_running_it(self):
        self.args.python = str(Path(sys.executable).resolve())
        self.args.mcp_python = sys.executable
        self.args.python_sha256 = self.args.mcp_python_sha256 = sha(Path(sys.executable).resolve().read_bytes())
        enable_acquisition(self)
        self.args.enable_acquired_inspection = True
        self.args.families = "services"
        self.args.node = self.args.node_sha256 = self.args.npm_cli = self.args.npm_cli_sha256 = None
        self.files["setup/mcp_adapter.py"] = ((SOURCE / "mcp_adapter.py").read_bytes(), 0o644)
        self.files["consumers/services/PROFILE-GUIDE.md"] = (b"Inert services guide\n", 0o644)
        files = {name.removeprefix("consumers/"): {"sha256": sha(raw)}
                 for name, (raw, _) in self.files.items()
                 if name.startswith("consumers/") and name != "consumers/CONTENTS.json"}
        self.files["consumers/CONTENTS.json"] = (json.dumps({"files": files}).encode(), 0o644)
        self.publish_release()
        host.prepare(self.args)
        entry = json.loads((Path(self.args.output) / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]
        async def exercise():
            from mcp import ClientSession, StdioServerParameters
            from mcp.client.stdio import stdio_client
            async with asyncio.timeout(20):
                async with stdio_client(StdioServerParameters(**entry)) as (reader, writer):
                    async with ClientSession(reader, writer) as client:
                        await client.initialize()
                        tools = {item.name: item for item in (await client.list_tools()).tools}
                        names = set(tools)
                        self.assertIn("setup_acquire_mcpb", names)
                        self.assertIn("setup_inspect_mcpb", names)
                        self.assertNotIn("setup_inspect_acquired", names)
                        inspection = tools["setup_inspect_mcpb"].input_schema
                        self.assertIn("job_id", inspection["properties"])
                        self.assertIn("receipt_sha256", inspection["properties"])
                        self.assertNotIn("oneOf", inspection)
                        refused = await client.call_tool("setup_inspect_mcpb", {"files": []})
                        self.assertTrue(refused.is_error)
                        self.assertEqual(refused.structured_content, {"error": "invalid_arguments"})
                        self.assertNotIn("setup_acquire_python", names)
                        self.assertNotIn("setup_acquire_node", names)
                        result = await client.call_tool("setup_state", {})
                        self.assertFalse(result.is_error, result)
                        self.assertIsNone(result.structured_content["job"])
        asyncio.run(exercise())
        self.assertEqual(list(self.staging.iterdir()), [])


class McpbAcquisitionBootstrapTests(unittest.TestCase):
    setUp = bootstrap_tests.BootstrapTests.setUp
    publish_release = preparation_tests.PreparationTests.publish_release
    installer = bootstrap_tests.BootstrapTests.installer
    mcpb = preparation_tests.PreparationTests.mcpb
    refused = bootstrap_tests.BootstrapTests.refused
    client = bootstrap_tests.BootstrapTests.client

    def test_cli_forwards_profile_to_both_preview_and_final(self):
        path, _ = enable_acquisition(self)
        with redirect_stdout(io.StringIO()):
            self.assertEqual(bootstrap.main(cli_arguments(self.args)), 0)
        output = Path(self.args.output)
        self.assertEqual((output / "mcpb-acquisition.json").read_bytes(), path.read_bytes())
        argv = json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertEqual(argv[argv.index("--mcpb-acquisition-profile") + 1], str(output / "mcpb-acquisition.json"))
        self.assertIn(str(self.client / "runtime/bin/python"), argv)
        self.assertTrue((self.client / "verify-entered").is_file())

    def test_bad_profile_refuses_before_client_installation(self):
        path, profile = enable_acquisition(self)
        profile["allowed_origins"] = ["https://example.com/path"]
        path.write_text(json.dumps(profile))
        self.args.mcpb_acquisition_profile_sha256 = sha(path.read_bytes())
        self.refused("invalid_mcpb_acquisition_profile")
        self.assertFalse(self.client.exists())

    def test_profile_client_alias_overlap_refuses_before_installation(self):
        enable_acquisition(self)
        self.args.mcpb_acquisition_profile = str(self.client).upper()
        self.refused("mcpb_acquisition_profile_overlaps_host_paths")
        self.assertFalse(self.client.exists())

    def test_profile_changed_after_preview_refuses_before_installer(self):
        path, _ = enable_acquisition(self)
        actual = bootstrap.host.recheck_release
        calls = 0
        def changing(*args):
            nonlocal calls
            calls += 1
            actual(*args)
            if calls == 2:
                path.write_text("changed after preview")
        with patch.object(bootstrap.host, "recheck_release", side_effect=changing):
            self.refused("host_input_changed")
        self.assertFalse(self.client.exists())

    def test_older_preparer_cannot_silently_ignore_requested_acquisition(self):
        enable_acquisition(self)
        raw = self.files["setup/prepare_host.py"][0]
        raw += b"\n_original_prepare = prepare\ndef prepare(args):\n    args.mcpb_acquisition_profile = None\n    args.mcpb_acquisition_profile_sha256 = None\n    return _original_prepare(args)\n"
        self.files["setup/prepare_host.py"] = (raw, 0o644)
        self.publish_release()
        self.refused("mcpb_acquisition_not_prepared")
        self.assertFalse(self.client.exists())


if __name__ == "__main__":
    unittest.main()
