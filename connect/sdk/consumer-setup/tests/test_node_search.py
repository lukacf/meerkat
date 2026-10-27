"""Public npm research is bounded evidence, separate from acquisition authority."""
import asyncio
import copy
import importlib.util
import json
import os
from pathlib import Path
import sys
import threading
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


fixtures = load("node_search_acquisition_fixture", ROOT / "tests/test_node_acquisition.py")
M, B = fixtures.M, fixtures.B


class SearchRunner(B.Runner):
    """Controlled npm JSON at the existing process-owner interface."""
    def __init__(self, workspace):
        super().__init__(workspace)
        self.calls = []
        self.block = None
        self.entered = threading.Event()
        self.raw = M.wire([{"name": "@example/files", "version": "2.3.4", "description": "Read local files"}])
        self.code = 0

    def run(self, argv, **kwargs):
        self.calls.append((list(argv), kwargs, dict(self._compose_context.settings)))
        assert argv[-3] == "--search"
        config = Path(argv[-2]).read_bytes()
        assert M.sha(config) == argv[-1]
        value = json.loads(config)
        self.entered.set()
        if self.block:
            self.block.wait(5)
        (Path(value["directory"]) / "runtime.json").write_bytes(M.wire(
            {"version": "v24.19.0", "platform": "darwin", "arch": "arm64"}))
        return self.code, self.raw


class NodeSearchTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.NodeAcquisitionTests("runTest")
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.fixture.profile["allow_acquisition"] = False

    def owner(self):
        f = self.fixture
        f.profile_path.write_bytes(M.wire(f.profile))
        owner = M.NodeAcquisition(f.io, f.profile_path, M.sha(f.profile_path.read_bytes()),
                                  f.python, None, None, None, f.node)
        owner.bind(f.workspace)
        return owner

    def test_search_preserves_registry_order_and_only_safe_evidence(self):
        owner = self.owner()
        runner = SearchRunner(owner.workspace)
        runner.raw = M.wire([
            {"name": "z-first", "version": "1.2.3", "description": "untrusted\u001b " + "a" * 3000,
             "links": {"repository": "file:///secret"}, "score": 1, "private": "CANARY"},
            {"name": "@a/second", "version": "2.0.0-beta.1", "description": "Agent must assess this"}])
        before = set(owner.workspace.rglob("*"))
        staging_before = list(self.fixture.staging.iterdir())
        result = owner.search({"query": "MCP file reading"}, runner)
        self.assertEqual([row["name"] for row in result["candidates"]], ["z-first", "@a/second"])
        self.assertEqual(result["scope"], "untrusted_public_npm_search_evidence")
        self.assertTrue(result["selection_required"])
        self.assertLessEqual(len(result["candidates"][0]["description"].encode()), 2048)
        self.assertNotIn("CANARY", json.dumps(result))
        self.assertNotIn("file:///", json.dumps(result))
        self.assertNotIn("\u001b", result["candidates"][0]["description"])
        self.assertEqual(set(owner.workspace.rglob("*")), before)
        self.assertEqual(list(self.fixture.staging.iterdir()), staging_before)
        self.assertEqual(runner.calls[0][1], {"capture": True, "inspection_timeout": 30})
        self.assertEqual(runner.calls[0][2]["output_limit"], 256 * 1024)
        with self.assertRaisesRegex(B.SetupError, "node_acquisition_unavailable"):
            owner.run(self.fixture.request, self.fixture.job, runner.run, lambda: None)

    def test_invalid_query_never_starts_process(self):
        owner = self.owner()
        runner = SearchRunner(owner.workspace)
        for value in ({}, {"query": "ok", "registry": "https://evil.invalid"}, {"query": None},
                      *({"query": q} for q in ("", " ", " files", "files ", "x\n--registry=evil",
                      "/regexp/", "files /regexp/", "--registry=evil", "x --json=false", "x" * 513,
                      "é" * 257, "\ud800", "ab\u202ecd"))):
            with self.subTest(value=repr(value)), self.assertRaisesRegex(B.SetupError, "invalid_node_search_arguments"):
                owner.search(value, runner)
        self.assertEqual(runner.calls, [])

    def test_malformed_oversized_nonzero_and_empty_output(self):
        owner = self.owner()
        runner = SearchRunner(owner.workspace)
        valid = {"name": "files", "version": "1.0.0", "description": "safe"}
        for raw in (b"CANARY", b"{}", b"[", b" " * (M.SEARCH_BYTES + 1), M.wire([valid] * 21),
                    b'[{"name":"a","name":"b","version":"1.0.0"}]',
                    *(M.wire([dict(valid, **change)]) for change in
                      ({"name": "../../evil"}, {"version": "latest"}, {"version": "1.0.0-01"},
                       {"description": {"private": "CANARY"}}))):
            runner.raw = raw
            with self.subTest(raw=raw[:60]), self.assertRaisesRegex(B.SetupError, "node_search_failed") as error:
                owner.search({"query": "files"}, runner)
            self.assertNotIn("CANARY", str(error.exception))
        runner.raw, runner.code = M.wire([valid]), 1
        with self.assertRaisesRegex(B.SetupError, "node_search_failed"):
            owner.search({"query": "files"}, runner)
        runner.raw, runner.code = b"[]", 0
        self.assertEqual(owner.search({"query": "files"}, runner)["candidates"], [])
        self.assertEqual(list((owner.workspace / "tmp").iterdir()), [])

    def test_host_change_and_non_boolean_scope_refuse(self):
        for invalid in (0, 1, "false", None):
            self.fixture.profile["allow_acquisition"] = invalid
            with self.assertRaisesRegex(B.SetupError, "invalid_node_acquisition_profile"):
                self.owner()
        self.fixture.profile["allow_acquisition"] = False
        owner = self.owner()
        self.fixture.profile["allow_acquisition"] = True
        self.fixture.profile_path.write_bytes(M.wire(self.fixture.profile))
        runner = SearchRunner(owner.workspace)
        with self.assertRaisesRegex(B.SetupError, "node_acquisition_host_changed"):
            owner.search({"query": "files"}, runner)
        self.assertEqual(runner.calls, [])

    def test_worker_literal_argv_and_clean_environment(self):
        owner = self.owner()
        directory = self.fixture.root / "worker"
        directory.mkdir()
        for name in ("project", "home", "tmp", "cache"):
            (directory / name).mkdir()
        value = {"node": str(owner.node), "node_sha256": M.sha(owner.inputs[owner.node]),
                 "npm": str(owner.assets / "npm"), "directory": str(directory), "query": "MCP file reading"}
        cwd = Path.cwd()
        try:
            with patch("platform.machine", return_value="arm64"), patch.object(M.sys, "platform", "darwin"), \
                 patch("resource.setrlimit"), patch.object(M.os, "execve") as execute, \
                 patch.dict(os.environ, {"NODE_OPTIONS": "--require=/secret", "NPM_TOKEN": "CANARY", "HTTPS_PROXY": "http://evil"}):
                M.search_worker(value)
            _, argv, env = execute.call_args.args
            self.assertEqual(argv[-2:], ["--", "MCP file reading"])
            self.assertIn("search", argv)
            self.assertNotIn("install", argv)
            self.assertIn("--registry=https://registry.npmjs.org", argv)
            self.assertIn("--searchlimit=20", argv)
            self.assertFalse({"NODE_OPTIONS", "NPM_TOKEN", "HTTPS_PROXY"} & set(env))
            self.assertEqual(env["HOME"], str(directory / "home"))
            for key in ("npm_config_userconfig", "npm_config_globalconfig"):
                self.assertEqual(Path(env[key]).read_bytes(), b"# Host-owned empty configuration\n")
        finally:
            os.chdir(cwd)

    def test_real_runner_deadline_and_output_cap_join_child(self):
        owner = self.owner()
        for mode in ("deadline", "oversized"):
            pid_path = self.fixture.root / (mode + ".pid")
            class BoundedRunner(B.Runner):
                def run_compose(self, argv, **kwargs):
                    program = ("import os,pathlib,time; pathlib.Path(" + repr(str(pid_path))
                               + ").write_text(str(os.getpid())); "
                               + ("print('x'*8192,flush=True); " if mode == "oversized" else "")
                               + "time.sleep(10)")
                    return super().run_compose([sys.executable, "-I", "-S", "-c", program],
                                               output_limit=4096, timeout=0.3)
            with self.subTest(mode=mode), self.assertRaisesRegex(B.SetupError, "node_search_failed"):
                owner.search({"query": "files"}, BoundedRunner(owner.workspace))
            with self.assertRaises(ProcessLookupError):
                os.kill(int(pid_path.read_text()), 0)
        self.assertEqual(list((owner.workspace / "tmp").iterdir()), [])


class SearchControllerTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.adapter = load("search_controller_adapter", ROOT / "mcp_adapter.py")
        self.fixture_module = load("search_controller_fixture", ROOT / "tests/test_backend.py")
        self.fixture_module.backend = self.adapter.backend
        self.fixture = self.fixture_module.ControllerTests("runTest")
        self.fixture.setUp()
        self.addCleanup(self.fixture.tearDown)
        self.input_fixture = fixtures.NodeAcquisitionTests("runTest")
        self.input_fixture.setUp()
        self.addCleanup(self.input_fixture.doCleanups)
        f = self.input_fixture
        f.profile["allow_acquisition"] = False
        f.profile_path.write_bytes(M.wire(f.profile))
        self.options = {"families": ("services",), "node": None, "npm_cli": None,
            "selected_node": str(f.node), "selected_npm_cli": None,
            "node_acquisition_profile": f.profile_path,
            "node_acquisition_profile_sha256": M.sha(f.profile_path.read_bytes()),
            "allow_node_search": True, "runner_factory": SearchRunner}

    async def test_real_mcp_search_only_has_no_acquisition_or_install_mutation(self):
        from mcp import Client
        controller = self.fixture.controller(**self.options)
        before = copy.deepcopy((controller.job, controller.jobs, controller.acquisition_requests, controller.validations))
        owner = self.adapter.SetupAdapter(controller)
        try:
            async with Client(self.adapter.create_server(owner)) as client:
                tools = {tool.name: tool for tool in (await client.list_tools()).tools}
                self.assertIn("setup_search_node", tools)
                self.assertNotIn("setup_acquire_node", tools)
                self.assertNotIn("setup_selected_install", tools)
                self.assertTrue(tools["setup_search_node"].annotations.read_only_hint)
                self.assertTrue(tools["setup_search_node"].annotations.open_world_hint)
                result = await client.call_tool("setup_search_node", {"query": "local files"})
                self.assertFalse(result.is_error, result)
                self.assertEqual(result.structured_content["candidates"][0]["name"], "@example/files")
                self.assertTrue((await client.call_tool("setup_search_node", {"query": "files", "url": "https://evil"})).is_error)
            with self.assertRaisesRegex(self.adapter.backend.SetupError, "acquisition_unavailable"):
                controller.dispatch("/api/acquire_node", self.input_fixture.request)
            self.assertIn("node_search", controller.state())
            self.assertNotIn("node_acquisition", controller.state())
            self.assertFalse((controller.workspace / "acquisitions").exists())
            self.assertEqual((controller.job, controller.jobs, controller.acquisition_requests, controller.validations), before)
        finally:
            await owner.aclose()

    async def test_disabled_and_recovery_scope_refuse(self):
        controller = self.fixture.controller(families=("services",))
        self.assertNotIn("node_search", controller.state())
        with self.assertRaisesRegex(self.adapter.backend.SetupError, "node_search_unavailable"):
            controller.search_node({"query": "files"})
        for changes in ({"allow_node_search": False}, {"allow_node_search": 1},
                        {"recover_workspace": self.fixture.root}):
            with self.assertRaisesRegex(self.adapter.backend.SetupError, "invalid_node_search_scope|invalid_runtime"):
                self.fixture.controller(**dict(self.options, **changes))

    async def test_search_coexists_with_mcpb_profile_without_package_installer(self):
        f = self.input_fixture
        profile = {"schema_version": 1, "scope": self.adapter.backend.SELECTED_SCOPE,
            "staging_root": str(f.staging), "validators": {
                "mcpb": {"path": str(f.node), "sha256": M.sha(f.node.read_bytes())}}}
        path = f.root / "mcpb-profile.json"
        path.write_bytes(M.wire(profile))
        controller = self.fixture.controller(**dict(self.options,
            selected_install_profile=path, selected_install_profile_sha256=M.sha(path.read_bytes())))
        self.assertIsNone(controller.node_acquisition)
        self.assertIsNotNone(controller.node_search)
        self.assertEqual(controller.search_node({"query": "files"})["candidates"][0]["name"], "@example/files")

    async def test_cancellation_and_close_join_current_search(self):
        controller = self.fixture.controller(**self.options)
        controller.runner.block = threading.Event()
        owner = self.adapter.SetupAdapter(controller)
        task = asyncio.create_task(owner.dispatch("/api/search_node", {"query": "files"}))
        await asyncio.to_thread(controller.runner.entered.wait, 3)
        self.assertTrue(controller.runner.entered.is_set())
        # The state lock remains usable while the process is active.
        self.assertTrue(controller.state()["operation_in_progress"])
        task.cancel()
        close = asyncio.create_task(owner.aclose())
        await asyncio.sleep(0.05)
        self.assertFalse(task.done())
        self.assertFalse(close.done())
        controller.runner.block.set()
        with self.assertRaises(asyncio.CancelledError):
            await task
        await close
        self.assertFalse(controller.operation_lock.locked())
        self.assertIsNone(controller.job)
        self.assertEqual(list((controller.workspace / "tmp").iterdir()), [])
        with self.assertRaisesRegex(self.adapter.backend.SetupError, "host_closing"):
            controller.search_node({"query": "files"})


if __name__ == "__main__":
    unittest.main()
