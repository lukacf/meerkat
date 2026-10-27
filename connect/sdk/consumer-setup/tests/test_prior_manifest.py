"""Saved request selection feeds fresh setup without restoring old authority."""
import asyncio
import copy
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

import test_prepare_host as preparation
import test_bootstrap_host as bootstraps
from test_successor import historical

host, sha, SOURCE = preparation.host, preparation.sha, preparation.SOURCE


def arguments(args):
    result = []
    for name, value in vars(args).items():
        if value is not None and value is not False:
            result.append("--" + name.replace("_", "-"))
            if value is not True:
                result.append(str(value))
    return result


def saved_requests(fixture, records=None, root_name="retained setup"):
    root = fixture.base / root_name
    (root / "requests").mkdir(parents=True, exist_ok=True)
    if records is None:
        records = [historical({"family": "services", "profile": profile, "operations": operations,
            "values": values}) for profile, operations, values in (
                ("slack", ["auth_test", "read_interval"], {"token_env": "SLACK_TOKEN", "timeout_seconds": 12,
                    "max_interval_requests": 5, "allowed_channel_ids": ["C123", "C456"]}),
                ("gmail", ["get_message"], {"token_env": "GMAIL_TOKEN", "timeout_seconds": 15}))]
    paths, entries = [], []
    for index, supplied in enumerate(records):
        record = copy.deepcopy(supplied)
        record["job_id"], record["workspace"] = f"{index + 1:032x}", str(root)
        path = root / "requests" / (record["job_id"] + ".json")
        path.write_text(json.dumps(record, indent=2) + "\n")
        paths.append(path)
        entries.append({"id": record["job_id"], "sha256": sha(path.read_bytes())})
    manifest = fixture.base / "toolkit-recovery.json"
    manifest.write_text(json.dumps({"schema_version": 1, "workspace": str(root), "requests": entries}, indent=2) + "\n")
    fixture.args.prior_manifest = str(manifest)
    fixture.args.prior_manifest_sha256 = sha(manifest.read_bytes())
    fixture.args.prior_request_id = entries[0]["id"]
    return manifest, root, paths


def inventory(root):
    return {str(path.relative_to(root)): (path.read_bytes(), path.stat().st_mode, path.stat().st_mtime_ns)
            for path in root.rglob("*") if path.is_file()}


class PriorManifestTests(unittest.TestCase):
    def setUp(self):
        self.fixture = preparation.PreparationTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.args = self.fixture.args
        self.manifest, self.original, self.requests = saved_requests(self.fixture)

    def refuse(self, code):
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no execution allowed")):
            with self.assertRaisesRegex(Exception, code):
                host.prepare(self.args)
        self.assertFalse(Path(self.args.output).exists())
        self.assertFalse(Path(self.args.workspace).exists())

    def test_cli_selects_exact_request_and_preserves_original_bytes_without_effects(self):
        self.args.prior_request_id = self.requests[1].stem
        before, manifest_before = inventory(self.original), self.manifest.read_bytes()
        with redirect_stdout(io.StringIO()), patch.object(subprocess, "Popen", side_effect=AssertionError("no execution allowed")):
            self.assertEqual(host.main(arguments(self.args)), 0)
        output = Path(self.args.output)
        selected = self.requests[1].read_bytes()
        self.assertEqual((output / "prior-request.json").read_bytes(), selected)
        self.assertEqual((output / "prior-request.json").stat().st_mode & 0o777, 0o600)
        receipt = json.loads((output / "prepared.json").read_bytes())
        self.assertEqual(receipt["prior_selection"], {"source": "recovery_manifest",
            "manifest_sha256": self.args.prior_manifest_sha256,
            "request_id": self.requests[1].stem, "request_sha256": sha(selected)})
        self.assertNotIn("recovery", receipt)
        argv = json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertEqual(argv[argv.index("--prior-request") + 1], str(output / "prior-request.json"))
        self.assertEqual(argv[argv.index("--prior-request-sha256") + 1], sha(selected))
        self.assertNotIn("--recover-manifest", argv)
        self.assertNotIn("--prior-manifest", argv)
        self.assertEqual(inventory(self.original), before)
        self.assertEqual(self.manifest.read_bytes(), manifest_before)
        self.assertFalse(Path(self.args.workspace).exists())

    def test_complete_explicit_selection_and_valid_id_are_required_before_reads(self):
        original = copy.copy(self.args)
        cases = [({name: value}, "prior_manifest_selection_required")
                 for name in ("prior_manifest", "prior_manifest_sha256", "prior_request_id")
                 for value in (None, "", False)]
        cases += [({"prior_request_id": value}, "invalid_prior_request_id")
                  for value in ("../request", "a" * 33, "a" * 32 + "\n")]
        cases += [({"prior_manifest_sha256": "bad"}, "invalid_sha256"),
                  ({"prior_manifest": "relative.json"}, "invalid_input_path")]
        for changed, code in cases:
            with self.subTest(changed=changed):
                self.args = copy.copy(original)
                vars(self.args).update(changed)
                with patch.object(host, "release_snapshot", side_effect=AssertionError("no release read")):
                    self.refuse(code)

    def test_raw_prior_and_readback_modes_cannot_be_combined(self):
        original = copy.copy(self.args)
        for changes, code in (({"prior_request": str(self.requests[0])}, "ambiguous_prior_selection"),
                              ({"prior_request_sha256": "a" * 64}, "ambiguous_prior_selection"),
                              ({"prior_request": ""}, "ambiguous_prior_selection"),
                              ({"recover_manifest": str(self.manifest),
                                "recover_manifest_sha256": sha(self.manifest.read_bytes())}, "recovery_read_only")):
            with self.subTest(changes=changes):
                self.args = copy.copy(original)
                vars(self.args).update(changes)
                self.refuse(code)

    def test_missing_or_foreign_request_is_not_inferred(self):
        self.args.prior_request_id = "f" * 32
        self.refuse("prior_request_not_selected")
        self.args.prior_request_id = self.requests[0].stem
        record = json.loads(self.requests[0].read_bytes())
        for key, value in (("job_id", "f" * 32), ("workspace", str(self.fixture.base))):
            changed = {**record, key: value}
            self.requests[0].write_bytes(host.encoded(changed))
            self.refresh_manifest()
            self.refuse("invalid_recovery_request")

    def refresh_manifest(self):
        manifest = json.loads(self.manifest.read_bytes())
        for entry, path in zip(manifest["requests"], self.requests):
            entry["sha256"] = sha(path.read_bytes())
        self.manifest.write_bytes(host.encoded(manifest))
        self.args.prior_manifest_sha256 = sha(self.manifest.read_bytes())

    def test_nonconfigured_kinds_and_invalid_configuration_are_refused(self):
        from test_backend import backend
        raw = self.requests[0].read_bytes()
        for kind in (backend.ACQUISITION_REQUEST, backend.SELECTED_REQUEST, backend.REMOTE_EXPORT_REQUEST):
            record = json.loads(raw)
            record["kind"] = kind
            self.requests[0].write_bytes(host.encoded(record))
            self.refresh_manifest()
            self.refuse("unsupported_prior_request")
        record = json.loads(raw)
        record["configuration_sha256"] = "0" * 64
        self.requests[0].write_bytes(host.encoded(record))
        self.refresh_manifest()
        self.refuse("invalid_prior_request")

    def test_unselected_other_kinds_are_not_copied_or_made_setup_authority(self):
        from test_backend import backend
        records = [historical()] + [{**historical(), "kind": kind} for kind in
            (backend.ACQUISITION_REQUEST, backend.SELECTED_REQUEST, backend.REMOTE_EXPORT_REQUEST)]
        _, root, paths = saved_requests(self.fixture, records)
        before = inventory(root)
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no execution allowed")):
            host.prepare(self.args)
        output = Path(self.args.output)
        self.assertEqual((output / "prior-request.json").read_bytes(), paths[0].read_bytes())
        self.assertFalse((output / "recover-manifest.json").exists())
        self.assertEqual(inventory(root), before)

    def test_hash_changes_and_prepublication_mutation_refuse(self):
        raw_manifest, raw_request = self.manifest.read_bytes(), self.requests[0].read_bytes()
        original_hash = self.args.prior_manifest_sha256
        self.args.prior_manifest_sha256 = "0" * 64
        self.refuse("recovery_manifest_changed")
        self.args.prior_manifest_sha256 = original_hash
        self.requests[0].write_bytes(raw_request + b" ")
        self.refuse("recovery_request_changed")
        self.requests[0].write_bytes(raw_request)
        recheck = host.recheck_pins
        for path in (self.manifest, *self.requests):
            with self.subTest(path=path.name):
                old = path.read_bytes()
                def mutate(pins):
                    recheck(pins)
                    path.write_bytes(old + b" ")
                with patch.object(host, "recheck_pins", side_effect=mutate):
                    self.refuse("changed")
                path.write_bytes(old)
        self.assertEqual(self.manifest.read_bytes(), raw_manifest)

    def test_symlink_sources_and_workspace_aliases_refuse(self):
        for source in (self.manifest, self.requests[0]):
            with self.subTest(source=source.name):
                saved = source.with_suffix(".saved")
                source.rename(saved)
                source.symlink_to(saved)
                try:
                    self.refuse("Too many levels|invalid_file|not a directory|invalid_recovery_path")
                finally:
                    source.unlink()
                    saved.rename(source)
        alias = self.fixture.base / "history-alias"
        alias.symlink_to(self.original, target_is_directory=True)
        record = json.loads(self.manifest.read_bytes())
        record["workspace"] = str(alias)
        self.manifest.write_bytes(host.encoded(record))
        self.args.prior_manifest_sha256 = sha(self.manifest.read_bytes())
        self.refuse("invalid_recovery_path|invalid_recovery_manifest")

    def test_old_workspace_manifest_and_recipe_sidecars_cannot_be_output_locations(self):
        original = copy.copy(self.args)
        for changes in ({"output": str(self.original / "new-output")},
                        {"workspace": str(self.original / "new-workspace")},
                        {"staging_root": str(self.original)}):
            with self.subTest(changes=changes):
                self.args = copy.copy(original)
                vars(self.args).update(changes)
                self.refuse("prior_manifest_overlaps_host_paths")
        self.args = copy.copy(original)
        self.fixture.args = self.args
        database = self.fixture.recipes("retain")
        protected_database = self.original / "recipes.sqlite3"
        protected_database.write_bytes(b"inert existing database")
        self.args.recipes_database = str(protected_database)
        self.refuse("recipes_database_overlaps_host_paths")
        self.args.recipes_database = str(database)
        for suffix in ("", "-wal", "-shm", "-journal"):
            source = Path(str(database) + suffix)
            source.write_bytes(self.manifest.read_bytes())
            self.args.prior_manifest = str(source)
            self.refuse("recipes_database_overlaps_host_paths")
            self.assertEqual(source.read_bytes(), self.manifest.read_bytes())
        self.args.recipes_database = self.args.recipes_realm = self.args.recipes_owner = self.args.recipes_scope = None
        moved = self.fixture.staging / self.manifest.name
        moved.write_bytes(self.manifest.read_bytes())
        self.args.prior_manifest = str(moved)
        self.refuse("prior_manifest_overlaps_host_paths")

    def test_case_and_unicode_aliases_cannot_publish_inside_historical_workspace(self):
        for original_name, alias_name in (("HistoryCase", "historycase"), ("caf\u00e9", "cafe\u0301")):
            with self.subTest(original_name=original_name):
                _, root, _ = saved_requests(self.fixture, root_name=original_name)
                alias = self.fixture.base / alias_name
                alias.mkdir(exist_ok=True)
                self.args.output = str(alias / "new-config")
                self.refuse("prior_manifest_overlaps_host_paths")
                self.assertFalse((root / "new-config").exists())

    def test_official_request_versions_keep_existing_parser_and_no_implicit_family(self):
        for version, family in ((2, "official-github"), (3, "official-playwright")):
            with self.subTest(version=version):
                record = historical({"family": family, "profile": "example", "operations": ["read"], "values": {}})
                record.update(schema_version=version, official_component=family.removeprefix("official-"))
                if version == 3:
                    record["playwright_host_profile_sha256"] = "e" * 64
                _, _, paths = saved_requests(self.fixture, [record])
                self.args.output = str(self.fixture.base / ("official-" + str(version)))
                host.prepare(self.args)
                output = Path(self.args.output)
                self.assertEqual((output / "prior-request.json").read_bytes(), paths[0].read_bytes())
                argv = json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
                self.assertEqual(argv[argv.index("--families") + 1], self.args.families)

    def test_generated_mcp_uses_current_profile_after_original_files_are_removed(self):
        from test_backend import help_source, profiles
        current_help = help_source("services")
        current_help["config_fields"]["allowed_channel_ids"] = {"kind": "string_list"}
        current_help["config_fields"].pop("max_interval_requests")
        metadata = "import json,sys\nprint(json.dumps(" + repr(current_help) + " if 'config-help' in sys.argv else " + repr(profiles("services")) + "))\n"
        installer = """import json,pathlib,sys
def arg(flag): return sys.argv[sys.argv.index(flag)+1]
assert '--config' not in sys.argv, 'configured installation must not run'
d=pathlib.Path(arg('--destination')); p=d/'runtime/bin/python'; p.parent.mkdir(parents=True)
p.write_text('#!'+sys.executable+'\\n'+METADATA); p.chmod(0o755)
(d/'installed.json').write_text(json.dumps({'family':'services','destination':str(d),
    'scope':'observed_software_installation_only','contents_sha256':arg('--contents-sha256'),
    'command':[str(p),'-I','-m','meerkat_toolkit_services.adapters.services_mcp']}))
""".replace("METADATA", repr(metadata)).encode()
        self.fixture.files.update({"setup/mcp_adapter.py": ((SOURCE / "mcp_adapter.py").read_bytes(), 0o644),
            "consumers/install-library-consumer.py": (installer, 0o644),
            "consumers/services/PROFILE-GUIDE.md": (b"Current selected release\n", 0o644)})
        manifest = {name.removeprefix("consumers/"): {"sha256": sha(raw)}
                    for name, (raw, _) in self.fixture.files.items()
                    if name.startswith("consumers/") and name != "consumers/CONTENTS.json"}
        self.fixture.files["consumers/CONTENTS.json"] = (host.encoded({"files": manifest}), 0o644)
        self.fixture.publish_release()
        self.args.families = "services"
        self.args.node = self.args.node_sha256 = self.args.npm_cli = self.args.npm_cli_sha256 = None
        self.args.python = self.args.mcp_python = sys.executable
        self.args.python_sha256 = self.args.mcp_python_sha256 = sha(Path(sys.executable).resolve().read_bytes())
        before = inventory(self.original)
        host.prepare(self.args)
        self.assertEqual(inventory(self.original), before)
        raw = self.requests[0].read_bytes()
        self.requests[0].unlink()
        self.manifest.unlink()
        entry = json.loads((Path(self.args.output) / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]

        async def exercise():
            from mcp import ClientSession, StdioServerParameters
            from mcp.client.stdio import stdio_client
            async with asyncio.timeout(30):
                async with stdio_client(StdioServerParameters(**entry)) as (reader, writer):
                    async with ClientSession(reader, writer) as client:
                        await client.initialize()
                        async def call(name, args):
                            result = await client.call_tool(name, args)
                            self.assertFalse(result.is_error, result)
                            return result.structured_content
                        state = await call("setup_state", {})
                        self.assertEqual(state["mode"], "setup")
                        self.assertEqual(state["job_history"]["count"], 0)
                        self.assertEqual(state["prior_selection"], {"family": "services", "profile": "slack", "request_sha256": sha(raw)})
                        params = {"prior_request_sha256": sha(raw)}
                        self.assertEqual((await call("setup_successor", params))["status"], "preparation_required")
                        await call("setup_prepare", {"family": "services"})
                        while (state := await call("setup_state", {}))["job"]["status"] == "running":
                            await asyncio.sleep(0.02)
                        self.assertEqual(state["job"]["status"], "succeeded", state)
                        result = await call("setup_successor", params)
                        self.assertEqual(result["status"], "review_required")
                        self.assertEqual(result["values"], {"token_env": "SLACK_TOKEN", "timeout_seconds": 12,
                            "allowed_channel_ids": ["C123", "C456"]})
                        self.assertEqual(result["prior_operations"], ["auth_test", "read_interval"])
                        self.assertEqual(result["unapplied"], [{"kind": "field", "name": "max_interval_requests",
                            "prior_type": "integer", "reason": "not_in_current_profile"}])
                        self.assertEqual(result["help"]["guide_text"], "Current selected release\n")
                        self.assertNotEqual(result["current_contents_sha256"], result["reported_prior_contents_sha256"])
                        self.assertEqual((await call("setup_state", {}))["job_history"]["count"], 1)
        asyncio.run(exercise())
        self.assertEqual((Path(self.args.workspace) / "prior-request.json").read_bytes(), raw)
        self.assertFalse(list((Path(self.args.workspace) / "requests").iterdir()))


class PriorManifestBootstrapTests(unittest.TestCase):
    def setUp(self):
        self.fixture = bootstraps.BootstrapTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.args = self.fixture.args
        self.manifest, self.original, self.requests = saved_requests(self.fixture)

    def test_cli_delegates_selected_request_to_current_preparation(self):
        before = inventory(self.original)
        with redirect_stdout(io.StringIO()):
            self.assertEqual(bootstraps.bootstrap.main(arguments(self.args)), 0)
        output = Path(self.args.output)
        self.assertEqual((output / "prior-request.json").read_bytes(), self.requests[0].read_bytes())
        self.assertEqual(inventory(self.original), before)
        self.assertFalse(Path(self.args.workspace).exists())

    def test_original_workspace_and_manifest_cannot_be_client_outputs(self):
        for path in (self.original / "client", self.manifest):
            self.args.client_directory = str(path)
            with patch.object(bootstraps.bootstrap, "run_owner", side_effect=AssertionError("no installer")):
                self.fixture.refused("prior_manifest_overlaps_host_paths|output_or_workspace_exists")

    def test_older_preparation_owner_cannot_silently_ignore_selection(self):
        code, mode = self.fixture.files["setup/prepare_host.py"]
        code += b"\nprevious_prepare = prepare\ndef prepare(args):\n    args = SimpleNamespace(**vars(args))\n    args.prior_manifest = args.prior_manifest_sha256 = args.prior_request_id = None\n    return previous_prepare(args)\n"
        self.fixture.files["setup/prepare_host.py"] = code, mode
        self.fixture.publish_release()
        with patch.object(bootstraps.bootstrap, "run_owner", side_effect=AssertionError("no installer")):
            self.fixture.refused("prior_manifest_not_prepared")
        self.assertFalse(self.fixture.client.exists())

    def test_change_after_preview_refuses_before_client_installation(self):
        recheck = bootstraps.bootstrap.host.recheck_pins
        def mutate(pins):
            recheck(pins)
            self.manifest.write_bytes(self.manifest.read_bytes() + b" ")
        with patch.object(bootstraps.bootstrap.host, "recheck_pins", side_effect=mutate), \
                patch.object(bootstraps.bootstrap, "run_owner", side_effect=AssertionError("no installer")):
            self.fixture.refused("recovery_manifest_changed")
        self.assertFalse(self.fixture.client.exists())

    def test_change_after_client_installation_preserves_partial_client(self):
        run = bootstraps.bootstrap.run_owner
        calls = []
        def mutate(*args):
            calls.append(args[0])
            result = run(*args)
            self.requests[0].write_bytes(self.requests[0].read_bytes() + b" ")
            return result
        with patch.object(bootstraps.bootstrap, "run_owner", side_effect=mutate):
            self.fixture.refused("recovery_request_changed")
        self.assertEqual(len(calls), 1)
        self.assertTrue((self.fixture.client / "installed.json").exists())
        self.assertFalse((self.fixture.client / "verify-entered").exists())


if __name__ == "__main__":
    unittest.main()
