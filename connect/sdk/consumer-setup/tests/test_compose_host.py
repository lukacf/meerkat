"""Ordinary host preparation preserves Compose acquisition/execution separation."""
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


host = preparation_tests.host
SOURCE, sha = preparation_tests.SOURCE, preparation_tests.sha
bootstrap = bootstrap_tests.bootstrap


def enable_compose(case, *, acquisition=False, execution=False, mode="new"):
    for name in ("compose_acquisition.py", "compose_bridge.py"):
        case.files["setup/" + name] = ((SOURCE / name).read_bytes(), 0o644)
    runtime = SOURCE.parent / "python/src/meerkat_toolkit/compose.py"
    case.files[host.COMPOSE_RUNTIME] = (runtime.read_bytes(), 0o644)
    case.files[host.COMPOSE_PLANNER] = (b"inert canonical planner, never execute\n", 0o755)
    case.files["selected/provenance/selection.json"] = (json.dumps({
        "platform": "macos-arm64", "scope": "retained_selected_tools",
        "capabilities": ["compose-local-execution"]}).encode(), 0o644)
    engine = {"id": "test-engine", "endpoint": "unix:///not-a-real-engine.sock"}
    for name in ("docker", "compose"):
        tool = case.base / (name + "-tool")
        tool.write_text("inert host tool, must never execute\n")
        tool.chmod(0o755)
        engine[name] = {"path": str(tool), "sha256": sha(tool.read_bytes())}
    common = {"schema_version": 1, "owner": "host", "placement": "local-compose",
              "host": {"placements": ["local-compose"], "runtimes": ["container"],
                       "contracts": {}, "services": {}}, "project": "test-project", "engine": engine}
    profiles = {}
    if acquisition:
        profiles["acquisition_host"] = {**common,
            "scope": "prepare_agent_selected_local_container_plan_with_image_pull",
            "allowed_registries": ["docker.io"]}
    if execution:
        profiles["backend"] = {**common, "components": {}, "volumes": {"state": {}}, "files": {}}
        graph = case.base / "graph"
        if mode == "reopen":
            graph.mkdir()
            (graph / "sentinel").write_text("existing graph must not change")
        case.args.compose_graph_workspace = str(graph)
        case.args.compose_mode = mode
        case.args.compose_expected_plan_sha256 = "a" * 64 if mode == "reopen" else None
        case.args.compose_allowed_actions = "observe,stop" if mode == "reopen" else "prepare,start,observe"
    for name, value in profiles.items():
        source = case.base / ("original " + name + ".json")
        source.write_text(json.dumps(value, indent=2) + "\n")
        setattr(case.args, "compose_" + name + "_profile", str(source))
        setattr(case.args, "compose_" + name + "_profile_sha256", sha(source.read_bytes()))
    case.publish_release()
    return profiles


def enable_successor(case, *, large=False):
    profiles = enable_compose(case, execution=True, mode="reopen")
    inputs = case.base / "successor-inputs.json"
    # Each file fits the canonical file bound; the input envelope exceeds MAX_BODY.
    inputs.write_text(json.dumps({"schema_version": 1, "package_json": "{}", "manifest_json": "{}",
        "configuration": "{}", "policy": "{}", "files": {"one": "a" * (60000 if large else 3), "two": "b" * 60000}}))
    target = case.base / "successor-profile.json"
    target.write_text(json.dumps({**profiles["backend"], "owner": "successor-host"}))
    case.args.compose_successor_inputs = str(inputs)
    case.args.compose_successor_inputs_sha256 = sha(inputs.read_bytes())
    case.args.compose_successor_backend_profile = str(target)
    case.args.compose_successor_backend_profile_sha256 = sha(target.read_bytes())
    case.args.compose_successor_transition_sha256 = "b" * 64
    case.args.compose_allowed_actions = "observe,prepare_successor,adopt_successor,reconcile_successor,rebind"
    return inputs, target


def cli_arguments(args):
    result = []
    for name, value in vars(args).items():
        if value is not None and value is not False:
            result.append("--" + name.replace("_", "-"))
            if value is not True:
                result.append(str(value))
    return result


class ComposeHostTests(unittest.TestCase):
    setUp = preparation_tests.PreparationTests.setUp
    publish_release = preparation_tests.PreparationTests.publish_release
    refuse = preparation_tests.PreparationTests.refuse

    def test_reopen_preparation_preserves_explicit_dormant_deletion_grant(self):
        enable_compose(self, execution=True, mode="reopen")
        source = Path(self.args.compose_backend_profile)
        profile = json.loads(source.read_bytes()); profile["volumes"] = {}
        source.write_text(json.dumps(profile))
        self.args.compose_backend_profile_sha256 = sha(source.read_bytes())
        self.args.compose_allowed_actions = "observe,stop,remove"
        self.args.compose_delete_state_namespaces = "vendor:data"
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            host.prepare(self.args)
        output = Path(self.args.output)
        outer = json.loads((output / "compose-execution.json").read_bytes())
        self.assertEqual(outer["delete_state_namespaces"], ["vendor:data"])
        self.assertEqual(outer["expected_plan_sha256"], "a" * 64)
        self.assertEqual(Path(outer["backend_profile"]["path"]).read_bytes(), source.read_bytes())
        self.assertEqual((Path(self.args.compose_graph_workspace) / "sentinel").read_text(), "existing graph must not change")

    def test_successor_retains_exact_inputs_and_profile_without_graph_readback(self):
        inputs, target = enable_successor(self, large=True)
        before = (Path(self.args.compose_graph_workspace) / "sentinel").read_bytes()
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            host.prepare(self.args)
        output = Path(self.args.output)
        outer = json.loads((output / "compose-execution.json").read_bytes())
        self.assertEqual(outer["successor"], {"inputs": {"path": str(output / "compose-successor-inputs.json"),
            "sha256": sha(inputs.read_bytes())}, "backend_profile": {"path": str(output / "compose-successor-backend.json"),
            "sha256": sha(target.read_bytes())}, "predecessor_plan_sha256": "a" * 64, "transition_sha256": "b" * 64})
        self.assertEqual((output / "compose-successor-inputs.json").read_bytes(), inputs.read_bytes())
        self.assertEqual((output / "compose-successor-backend.json").read_bytes(), target.read_bytes())
        self.assertEqual((Path(self.args.compose_graph_workspace) / "sentinel").read_bytes(), before)

    def test_successor_requires_both_pins_reopen_and_bounded_assembled_input(self):
        inputs, _ = enable_successor(self)
        saved = self.args.compose_successor_backend_profile_sha256
        self.args.compose_successor_backend_profile_sha256 = None
        self.refuse("compose_successor_pair_required")
        self.args.compose_successor_backend_profile_sha256 = saved
        raw = json.loads(inputs.read_bytes()); raw["files"] = {"large": "x" * (2 * 1024 * 1024)}
        inputs.write_text(json.dumps(raw)); self.args.compose_successor_inputs_sha256 = sha(inputs.read_bytes())
        with self.assertRaises(Exception): host.prepare(self.args)
        self.assertFalse(Path(self.args.output).exists())

    def test_acquisition_pair_is_not_silently_ignored(self):
        self.args.compose_acquisition_host_profile = str(self.base / "acquisition.json")
        self.args.compose_acquisition_host_profile_sha256 = None
        self.refuse("compose_acquisition_host_profile_pair_required")

    def test_execution_requires_explicit_actions(self):
        self.args.compose_backend_profile = str(self.base / "backend.json")
        self.args.compose_backend_profile_sha256 = "a" * 64
        self.args.compose_mode = "new"
        self.args.compose_graph_workspace = str(self.base / "graph")
        self.refuse("compose_execution_choices_required")

    def test_plain_release_is_unchanged_without_opt_in(self):
        host.prepare(self.args)
        output = Path(self.args.output)
        self.assertFalse(any("compose" in p.name for p in output.iterdir()))
        self.assertNotIn("--compose-", (output / "mcp.json").read_text())

    def test_acquisition_only_preserves_inner_bytes_and_cannot_enable_execution(self):
        enable_compose(self, acquisition=True)
        source = Path(self.args.compose_acquisition_host_profile)
        original = source.read_bytes(), source.stat().st_mtime_ns
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")), redirect_stdout(io.StringIO()):
            self.assertEqual(host.main(cli_arguments(self.args)), 0)
        output = Path(self.args.output)
        outer = json.loads((output / "compose-acquisition.json").read_bytes())
        self.assertEqual((output / "compose-acquisition-host.json").read_bytes(), original[0])
        self.assertEqual((source.read_bytes(), source.stat().st_mtime_ns), original)
        self.assertEqual(outer["staging_root"], str(self.staging))
        self.assertEqual(outer["python_sha256"], self.args.python_sha256)
        self.assertEqual(outer["host_profile"]["path"], str(output / "compose-acquisition-host.json"))
        self.assertEqual(outer["runtime"]["path"], str(self.release / host.COMPOSE_RUNTIME))
        self.assertEqual(outer["planner"]["path"], str(self.release / host.COMPOSE_PLANNER))
        command = json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertIn("--compose-acquisition-profile", command)
        self.assertNotIn("--compose-profile", command)
        self.assertFalse((output / "compose-execution.json").exists())
        self.assertFalse(Path(self.args.workspace).exists())
        self.assertEqual(list(self.staging.iterdir()), [])

    def test_execution_retains_explicit_actions_without_acquisition(self):
        enable_compose(self, execution=True)
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            host.prepare(self.args)
        output = Path(self.args.output)
        outer = json.loads((output / "compose-execution.json").read_bytes())
        self.assertEqual(outer["allowed_actions"], ["prepare", "start", "observe"])
        self.assertEqual(outer["delete_state_namespaces"], [])
        self.assertEqual(outer["mode"], "new")
        self.assertIsNone(outer["expected_plan_sha256"])
        self.assertEqual(outer["python"]["path"], self.args.python)
        self.assertNotIn("--compose-acquisition-profile", (output / "mcp.json").read_text())
        self.assertFalse(Path(self.args.compose_graph_workspace).exists())

    def test_reopen_retains_exact_expected_plan_without_mutating_graph(self):
        enable_compose(self, execution=True, mode="reopen")
        graph = Path(self.args.compose_graph_workspace)
        before = {p.name: p.read_bytes() for p in graph.iterdir()}
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no planner or engine call")):
            host.prepare(self.args)
        outer = json.loads((Path(self.args.output) / "compose-execution.json").read_bytes())
        self.assertEqual(outer["expected_plan_sha256"], "a" * 64)
        self.assertEqual(outer["mode"], "reopen")
        self.assertEqual({p.name: p.read_bytes() for p in graph.iterdir()}, before)

    def test_reopen_cannot_grant_prepare_and_fresh_cannot_claim_existing_plan(self):
        enable_compose(self, execution=True)
        self.args.compose_expected_plan_sha256 = "a" * 64
        self.refuse("new_compose_plan_must_be_unset")
        graph = Path(self.args.compose_graph_workspace)
        graph.mkdir()
        self.args.compose_mode = "reopen"
        with self.assertRaisesRegex(Exception, "invalid_compose_profile"):
            host.prepare(self.args)
        self.assertFalse(Path(self.args.output).exists())

    def test_scope_and_registry_validation_uses_runtime_owner(self):
        profiles = enable_compose(self, acquisition=True)
        path = Path(self.args.compose_acquisition_host_profile)
        for change in ({"scope": "prepare_agent_selected_local_container_plan"},
                       {"allowed_registries": ["https://docker.io"]},
                       {"schema_version": True}, {"unexpected": True}):
            with self.subTest(change=change):
                path.write_text(json.dumps({**profiles["acquisition_host"], **change}))
                self.args.compose_acquisition_host_profile_sha256 = sha(path.read_bytes())
                with self.assertRaises(Exception):
                    host.prepare(self.args)
                self.assertFalse(Path(self.args.output).exists())

    def test_capability_and_complete_artifacts_required(self):
        enable_compose(self, execution=True)
        provenance = json.loads(self.files["selected/provenance/selection.json"][0])
        provenance["capabilities"] = []
        self.files["selected/provenance/selection.json"] = (json.dumps(provenance).encode(), 0o644)
        self.publish_release()
        self.refuse("compose_not_in_release")
        provenance["capabilities"] = ["compose-local-execution"]
        self.files["selected/provenance/selection.json"] = (json.dumps(provenance).encode(), 0o644)
        del self.files[host.COMPOSE_PLANNER]
        (self.release / host.COMPOSE_PLANNER).unlink()
        self.publish_release()
        self.refuse("compose_not_in_release")

    def test_profile_and_tool_hash_drift_refuses_before_publication(self):
        profiles = enable_compose(self, execution=True)
        source = Path(self.args.compose_backend_profile)
        self.args.compose_backend_profile_sha256 = "0" * 64
        self.refuse("compose_profile_changed")
        self.args.compose_backend_profile_sha256 = sha(source.read_bytes())
        Path(profiles["backend"]["engine"]["docker"]["path"]).write_text("changed")
        self.refuse("host_input_changed")

    def test_graph_profile_tool_overlap_and_aliases_refused(self):
        profiles = enable_compose(self, execution=True)
        original_graph = self.args.compose_graph_workspace
        self.args.compose_graph_workspace = str(self.staging / "graph")
        self.refuse("compose_graph_overlaps_host_paths")
        self.args.compose_graph_workspace = original_graph
        source = Path(self.args.compose_backend_profile)
        original_source = str(source)
        self.args.compose_backend_profile = str(self.staging / "profile")
        self.refuse("compose_profile_overlaps_host_paths")
        self.args.compose_backend_profile = original_source
        tool = self.staging / "docker"
        tool.write_text("inert")
        tool.chmod(0o755)
        profiles["backend"]["engine"]["docker"] = {"path": str(tool), "sha256": sha(tool.read_bytes())}
        source.write_text(json.dumps(profiles["backend"]))
        self.args.compose_backend_profile_sha256 = sha(source.read_bytes())
        self.refuse("compose_tool_overlaps_host_paths")
        alias = self.base / "alias"
        alias.symlink_to(tool)
        profiles["backend"]["engine"]["docker"]["path"] = str(alias)
        source.write_text(json.dumps(profiles["backend"]))
        self.args.compose_backend_profile_sha256 = sha(source.read_bytes())
        self.refuse("invalid_compose_engine_binding")

    def test_fresh_graph_rejects_case_and_unicode_aliases_of_host_paths(self):
        enable_compose(self, execution=True)
        for destination, graph in (("FutureGraph", "futuregraph"), ("caf\u00e9", "cafe\u0301")):
            for field in ("output", "workspace"):
                with self.subTest(destination=destination, field=field):
                    old = getattr(self.args, field)
                    setattr(self.args, field, str(self.base / destination))
                    self.args.compose_graph_workspace = str(self.base / graph)
                    self.refuse("compose_graph_overlaps_host_paths")
                    setattr(self.args, field, old)

    def test_explicit_choices_and_timeout_are_not_broadened(self):
        enable_compose(self, execution=True)
        for actions in ("start,start", "observe,all", " start"):
            self.args.compose_allowed_actions = actions
            with self.assertRaisesRegex(Exception, "invalid_compose_profile"):
                host.prepare(self.args)
            self.assertFalse(Path(self.args.output).exists())
        self.args.compose_allowed_actions = "observe"
        for timeout in (False, 0, 3601, "300"):
            self.args.compose_operation_timeout_seconds = timeout
            self.refuse("invalid_compose_timeout")

    def test_inputs_changed_after_private_validation_refused(self):
        profiles = enable_compose(self, execution=True)
        actual = host.recheck_release
        calls = 0
        def changed(*args):
            nonlocal calls
            calls += 1
            actual(*args)
            if calls == 2:
                Path(profiles["backend"]["engine"]["compose"]["path"]).write_text("drift")
        with patch.object(host, "recheck_release", side_effect=changed):
            self.refuse("host_input_changed")

    def test_generated_acquisition_mcp_has_no_execution_tools(self):
        self.generated_mcp(acquisition=True, execution=False)

    def test_generated_execution_mcp_only_exposes_explicit_actions(self):
        self.generated_mcp(acquisition=False, execution=True)

    def generated_mcp(self, *, acquisition, execution):
        self.args.python = str(Path(sys.executable).resolve())
        self.args.mcp_python = sys.executable
        self.args.python_sha256 = self.args.mcp_python_sha256 = sha(Path(sys.executable).resolve().read_bytes())
        enable_compose(self, acquisition=acquisition, execution=execution)
        self.args.families = "services"
        self.args.node = self.args.node_sha256 = self.args.npm_cli = self.args.npm_cli_sha256 = None
        self.files["setup/mcp_adapter.py"] = ((SOURCE / "mcp_adapter.py").read_bytes(), 0o644)
        self.files["consumers/services/PROFILE-GUIDE.md"] = (b"Inert services guide\n", 0o644)
        members = {name.removeprefix("consumers/"): {"sha256": sha(raw)}
                   for name, (raw, _) in self.files.items()
                   if name.startswith("consumers/") and name != "consumers/CONTENTS.json"}
        self.files["consumers/CONTENTS.json"] = (json.dumps({"files": members}).encode(), 0o644)
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
                        names = {tool.name for tool in (await client.list_tools()).tools}
                        self.assertEqual("setup_acquire_compose" in names, acquisition)
                        self.assertEqual({name for name in names if name.startswith("setup_compose_")},
                                         {"setup_compose_prepare", "setup_compose_start", "setup_compose_observe"}
                                         if execution else set())
                        result = await client.call_tool("setup_state", {})
                        state = json.loads(result.content[0].text)
                        self.assertIsNone(state["job"])
                        if acquisition:
                            self.assertTrue(state["compose_acquisition"]["available"])
                        else:
                            self.assertNotIn("compose_acquisition", state)
                        if execution:
                            self.assertTrue(state["compose"]["available"])
                            self.assertIsNone(state["compose"]["plan_sha256"])
                        else:
                            self.assertNotIn("compose", state)
        asyncio.run(exercise())


class ComposeBootstrapTests(unittest.TestCase):
    setUp = bootstrap_tests.BootstrapTests.setUp
    publish_release = preparation_tests.PreparationTests.publish_release
    installer = bootstrap_tests.BootstrapTests.installer

    def test_bootstrap_retains_successor_copies_and_admitted_transition(self):
        inputs, target = enable_successor(self, large=True)
        result = bootstrap.bootstrap(self.args)
        self.assertEqual(result["status"], "prepared")
        output = Path(self.args.output)
        self.assertEqual((output / "compose-successor-inputs.json").read_bytes(), inputs.read_bytes())
        self.assertEqual((output / "compose-successor-backend.json").read_bytes(), target.read_bytes())
        outer = json.loads((output / "compose-execution.json").read_bytes())
        self.assertEqual(outer["successor"]["transition_sha256"], "b" * 64)
        self.assertEqual(outer["expected_plan_sha256"], "a" * 64)

    def test_bootstrap_preserves_profiles_and_rebinds_only_client_python(self):
        enable_compose(self, acquisition=True, execution=True)
        result = bootstrap.bootstrap(self.args)
        self.assertEqual(result["status"], "prepared")
        output = Path(self.args.output)
        acquisition = json.loads((output / "compose-acquisition.json").read_bytes())
        execution = json.loads((output / "compose-execution.json").read_bytes())
        self.assertEqual(execution["python"]["path"], self.args.python)
        self.assertEqual(acquisition["python_sha256"], self.args.python_sha256)
        self.assertEqual((output / "compose-backend.json").read_bytes(), Path(self.args.compose_backend_profile).read_bytes())
        self.assertIn(str(Path(self.args.client_directory) / "runtime/bin/python"), (output / "mcp.json").read_text())
        self.assertFalse(Path(self.args.compose_graph_workspace).exists())

    def test_bootstrap_protects_real_client_from_graph_profile_and_tool(self):
        profiles = enable_compose(self, execution=True)
        original_graph = self.args.compose_graph_workspace
        self.args.compose_graph_workspace = self.args.client_directory
        with self.assertRaisesRegex(Exception, "compose_graph_overlaps_host_paths"):
            bootstrap.bootstrap(self.args)
        self.args.compose_graph_workspace = original_graph
        original_profile = self.args.compose_backend_profile
        self.args.compose_backend_profile = str(Path(self.args.client_directory) / "profile.json")
        with self.assertRaisesRegex(Exception, "compose_profile_overlaps_host_paths"):
            bootstrap.bootstrap(self.args)
        self.args.compose_backend_profile = original_profile
        source = Path(original_profile)
        # Alias spelling is conservatively refused by bootstrap before install.
        profiles["backend"]["engine"]["docker"]["path"] = self.args.output.upper()
        source.write_text(json.dumps(profiles["backend"]))
        self.args.compose_backend_profile_sha256 = sha(source.read_bytes())
        with self.assertRaises(Exception):
            bootstrap.bootstrap(self.args)
        self.assertFalse(Path(self.args.client_directory).exists())

    def test_bootstrap_refuses_archived_preparer_that_ignores_compose(self):
        enable_compose(self, execution=True)
        raw = self.files["setup/prepare_host.py"][0]
        raw += b'\n_original_prepare = prepare\ndef prepare(args):\n    args = SimpleNamespace(**{k: v for k, v in vars(args).items() if not k.startswith("compose_")})\n    return _original_prepare(args)\n'
        self.files["setup/prepare_host.py"] = (raw, 0o644)
        self.publish_release()
        with self.assertRaisesRegex(Exception, "compose_not_prepared"):
            bootstrap.bootstrap(self.args)
        self.assertFalse(Path(self.args.client_directory).exists())


if __name__ == "__main__":
    unittest.main()
