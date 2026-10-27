"""MCPB inspection admission and projection through the actual setup/MCP owners.

The fake canonical reader supplies envelopes only. It does not validate ZIPs,
MCPB declarations, secret defaults or executable compatibility; Rust tests own
those claims. No package bytes are executed by these tests.
"""
import asyncio
import copy
import json
from pathlib import Path
import sys
import threading
import unittest

from test_selected_mcpb import McpbFixture, McpbRunner, adapter, backend


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False,
                      separators=(",", ":"), allow_nan=False).encode()


def seal(value):
    value["source_digest"] = backend.digest(canonical(value["files"]))
    return value


def report(request):
    texts = {"./manifest.json": '{"name":"caf\u00e9","manifest_version":"0.3"}\n',
             "./server/main.js": "// Untrusted evidence; do not execute.\n",
             "./README.md": "# Bundle guidance\n"}
    rows = []
    for name in request["files"]:
        text = texts[name]
        source = backend.digest(text.encode())
        inspection = None
        if name == "./manifest.json":
            inspection = {"schema_version": 1, "scope": "metadata_only", "format": "mcpb",
                "status": "accepted", "source_digest": source,
                "summary": {"name": "caf\u00e9", "declarations": {
                    "runtime": "node", "entry_point": "server/main.js", "command": "node",
                    "args": ["${__dirname}/server/main.js"]},
                    "user_configuration": [{"name": "api_key", "type": "string", "sensitive": True}]},
                "diagnostics": []}
        rows.append({"path": name, "text": text, "byte_length": len(text.encode()),
                     "source_digest": source, "inspection": inspection})
    return seal({"schema_version": 1, "scope": "metadata_only", "format": "mcpb-archive",
        "status": "accepted", "artifact_sha256": request["artifact_sha256"],
        "artifact_byte_length": 103, "archive_entry_count": 3, "inventory_entry_count": 4,
        "inventory": [{"path": "./README.md", "byte_length": len(texts["./README.md"]), "directory": False},
                      {"path": "./manifest.json", "byte_length": len(texts["./manifest.json"].encode()), "directory": False},
                      {"path": "./server", "byte_length": 0, "directory": True},
                      {"path": "./server/main.js", "byte_length": len(texts["./server/main.js"]), "directory": False}],
        "inventory_truncated": False, "files": rows})


class InspectionRunner(McpbRunner):
    def __init__(self, workspace):
        super().__init__(workspace)
        self.inspections = []
        self.override = None
        self.mutate = None
        self.inspect_entered = threading.Event()
        self.inspect_release = None

    def run_compose(self, argv, *, input_file, input_sha256, output_limit, timeout):
        raw = input_file.read_bytes()
        assert backend.digest(raw) == input_sha256
        self.inspections.append({"argv": list(argv), "raw": raw, "sha256": input_sha256,
                                 "output_limit": output_limit, "timeout": timeout})
        self.inspect_entered.set()
        if self.inspect_release is not None:
            assert self.inspect_release.wait(5), "inspection test must release its worker"
        if self.mutate:
            self.mutate(input_file)
        if self.override is not None:
            return self.override
        return 0, backend.encoded(report(backend.decode(raw)))


class InspectionFixture(McpbFixture):
    def setUp(self):
        super().setUp()
        self.profile["package_inspector"] = self.tools["python"]
        self.profile["mcpb_inspection"] = True
        self.write_profile()
        self.request = {"staged_id": "unfamiliar", "artifact_sha256": "a" * 64,
                        "files": ["./server/main.js"]}

    def enabled(self):
        return self.controller(runner_factory=InspectionRunner)

    def inspect(self, c, request=None):
        return c.inspect_mcpb(copy.deepcopy(self.request if request is None else request))

    def unchanged(self, c):
        self.assertIsNone(c.job)
        self.assertEqual(c.jobs, {})
        self.assertEqual(c.validations, {})
        self.assertEqual(c.runner.calls, [])
        self.assertEqual(list((c.workspace / "tmp").iterdir()), [])


class InspectionTests(InspectionFixture, unittest.TestCase):
    def test_explicit_node_only_runtime_constructs_mcpb_and_python_mcpb_controllers(self):
        selected = self.root / "selected-node"
        selected.write_text("inert selected Node, never executed\n")
        selected.chmod(0o700)
        for include_python in (False, True):
            with self.subTest(include_python=include_python):
                if include_python:
                    self.profile["validators"]["python"] = self.tools["python"]
                    self.profile["installer"] = self.tools["installer"]
                    self.write_profile()
                # A library npm path is deliberately present. It must not be
                # borrowed for an explicit Node-only selected override.
                c = self.controller(selected_node=str(selected), selected_npm_cli=None,
                                    npm_cli=sys.executable, runner_factory=InspectionRunner)
                self.assertEqual(c.selected_node, str(selected))
                self.assertIsNotNone(c.npm_cli)
                self.assertIsNone(c.selected_npm_cli)
                self.assertIsNone(c.selected_binding["runtimes"]["npm_cli"])
                self.inspect(c)
                self.unchanged(c)

    def test_explicit_node_only_runtime_rejects_unrelated_or_node_package_profiles(self):
        selected = self.root / "selected-node"
        selected.write_text("inert selected Node, never executed\n")
        selected.chmod(0o700)
        for validators in (("mcpb", "node"), ("node",), ("python",)):
            with self.subTest(validators=validators):
                self.profile["validators"] = {name: self.tools[name] for name in validators}
                self.profile["installer"] = self.tools["installer"]
                self.profile["mcpb_inspection"] = "mcpb" in validators
                self.write_profile()
                workspace = self.root / ("refused-" + "-".join(validators))
                with self.assertRaisesRegex(backend.SetupError, "invalid_runtime"):
                    self.controller(workspace=workspace, selected_node=str(selected), selected_npm_cli=None,
                                    npm_cli=sys.executable)
                self.assertFalse(workspace.exists())
        workspace = self.root / "refused-no-selection"
        with self.assertRaisesRegex(backend.SetupError, "invalid_runtime"):
            self.controller(workspace=workspace, families=("services",), selected_install_profile=None,
                            selected_install_profile_sha256=None, selected_node=str(selected), selected_npm_cli=None)
        self.assertFalse(workspace.exists())
        with self.assertRaisesRegex(backend.SetupError, "invalid_runtime"):
            self.controller(selected_node=None, selected_npm_cli=sys.executable)

    def test_explicit_complete_pair_and_omitted_override_keep_existing_runtime_bindings(self):
        selected = self.root / "selected-node"
        selected.write_text("inert selected Node, never executed\n")
        selected.chmod(0o700)
        c = self.controller(selected_node=str(selected), selected_npm_cli=sys.executable,
                            node=sys.executable, npm_cli=sys.executable)
        self.assertEqual(c.selected_node, str(selected))
        self.assertEqual(c.selected_npm_cli, str(Path(sys.executable).resolve()))
        legacy = self.controller(node=sys.executable, npm_cli=sys.executable)
        self.assertEqual((legacy.selected_node, legacy.selected_npm_cli), (legacy.node, legacy.npm_cli))

    def test_exact_request_and_nonsecret_declarations_without_job_or_staging_read(self):
        c = self.enabled()
        # There is deliberately no staged directory. Only the canonical reader
        # may open the archive; this test inspects the parent envelope contract.
        result = self.inspect(c)
        self.assertEqual(result, self.inspect(c))
        invocation = c.runner.inspections[0]
        self.assertEqual(invocation["argv"], [str(c.workspace / "selected-tools/package_inspector"), "mcpb-archive"])
        self.assertEqual(backend.decode(invocation["raw"]), {
            "root": str(self.staging), "directory": "./unfamiliar",
            "artifact_sha256": "a" * 64, "files": ["./manifest.json", "./server/main.js"]})
        self.assertEqual(invocation["timeout"], backend.MCPB_INSPECTION_TIMEOUT)
        self.assertEqual(invocation["output_limit"], backend.PACKAGE_INSPECTION_OUTPUT)
        self.assertTrue(result["files"][0]["inspection"]["summary"]["user_configuration"][0]["sensitive"])
        self.assertEqual(result["archive_entry_count"], 3)
        self.assertEqual(result["inventory_entry_count"], 4)
        self.assertNotIn(str(self.staging), json.dumps(result))
        self.assertTrue(c.state()["mcpb_inspection"]["available"])
        self.unchanged(c)

    def test_manifest_is_implicit_and_explicit_manifest_is_read_once(self):
        c = self.enabled()
        for files in ([], ["./manifest.json"], ["./README.md", "./manifest.json"]):
            result = self.inspect(c, {**self.request, "files": files})
            names = [row["path"] for row in result["files"]]
            self.assertEqual(names, ["./manifest.json", *[name for name in files if name != "./manifest.json"]])
        self.unchanged(c)

    def test_request_bounds_and_host_paths_refuse_before_reader_entry(self):
        c = self.enabled()
        invalid = [dict(self.request, root="/private"), dict(self.request, staged_id="../other"),
            dict(self.request, staged_id="."), dict(self.request, artifact_sha256=True),
            dict(self.request, artifact_sha256="A" * 64), dict(self.request, files=None),
            dict(self.request, files=["./README.md"] * 2),
            dict(self.request, files=["./file" + str(n) for n in range(16)])]
        for name in ("../outside", "/absolute", "./a/../b", "./a//b", "./a\\b", "./x\x00", "./x\x7f", "./x\u0085"):
            invalid.append(dict(self.request, files=[name]))
        for request in invalid:
            with self.subTest(request=request), self.assertRaises(backend.SetupError):
                self.inspect(c, request)
        self.assertEqual(c.runner.inspections, [])
        self.unchanged(c)

    def test_optin_and_required_current_tools_are_validated(self):
        for value in (False, None):
            if value is None:
                self.profile.pop("mcpb_inspection", None)
            else:
                self.profile["mcpb_inspection"] = value
            self.write_profile()
            c = self.enabled()
            self.assertNotIn("mcpb_inspection", c.state())
            with self.assertRaisesRegex(backend.SetupError, "mcpb_inspection_unavailable"):
                self.inspect(c)
            self.assertEqual(c.runner.inspections, [])
        for value in (None, 1, "true"):
            self.profile["mcpb_inspection"] = value
            self.write_profile()
            with self.assertRaisesRegex(backend.SetupError, "invalid_selected_host_profile"):
                self.enabled()
        self.profile["mcpb_inspection"] = True
        inspector = self.profile.pop("package_inspector")
        self.write_profile()
        with self.assertRaisesRegex(backend.SetupError, "invalid_selected_host_profile"):
            self.enabled()
        self.profile["package_inspector"] = inspector
        self.profile["validators"] = {"python": self.tools["python"]}
        self.profile["installer"] = self.tools["installer"]
        self.write_profile()
        with self.assertRaisesRegex(backend.SetupError, "invalid_selected_host_profile"):
            self.enabled()

    def test_malformed_reports_refuse_without_creating_installer_state(self):
        c = self.enabled()
        good = self.inspect(c)
        def set_summary(value, summary):
            value["files"][0]["inspection"]["summary"] = summary
            seal(value)
        mutations = [lambda v: v.update(schema_version=True), lambda v: v.update(scope="installed"),
            lambda v: v.update(artifact_sha256="b" * 64), lambda v: v.update(artifact_byte_length=True),
            lambda v: v.update(artifact_byte_length=backend.SELECTED_FILE + 1),
            lambda v: v.update(source_digest="b" * 64), lambda v: v.update(root="PRIVATE"),
            lambda v: v.update(archive_entry_count=5), lambda v: v.update(inventory_entry_count=True),
            lambda v: v.update(inventory_truncated=True), lambda v: v["inventory"].reverse(),
            lambda v: v["inventory"][0].update(byte_length=-1),
            lambda v: v["files"][0].update(path="./elsewhere"),
            lambda v: v["files"][0].update(text="replaced"),
            lambda v: v["files"][0].update(byte_length=True),
            lambda v: v["files"][0].update(source_digest="b" * 64),
            lambda v: v["files"][0]["inspection"].update(source_digest="b" * 64),
            lambda v: v["files"][0]["inspection"].update(status="accepted_with_diagnostics"),
            lambda v: set_summary(v, {}), lambda v: set_summary(v, {"declarations": []}),
            lambda v: set_summary(v, {"declarations": {"runtime": "node", "entry_point": "index.js", "command": "node"}}),
            lambda v: set_summary(v, {"declarations": {"runtime": "node", "entry_point": "index.js", "command": "node", "args": [1]}}),
            lambda v: v["files"][1].update(inspection={}), lambda v: v["files"].pop()]
        for index, mutate in enumerate(mutations):
            value = copy.deepcopy(good)
            mutate(value)
            c.runner.override = (0, backend.encoded(value))
            with self.subTest(index=index), self.assertRaisesRegex(backend.SetupError, "mcpb_inspection_unavailable"):
                self.inspect(c)
        for override in ((True, backend.encoded(good)), (1, backend.encoded(good)), (0, b"PRIVATE_FAILURE"),
                         (0, b" " * (backend.PACKAGE_INSPECTION_OUTPUT + 1))):
            c.runner.override = override
            with self.assertRaisesRegex(backend.SetupError, "mcpb_inspection_unavailable"):
                self.inspect(c)
        self.unchanged(c)

    def test_inventory_paths_sizes_and_directory_entries_are_bounded(self):
        c = self.enabled()
        good = self.inspect(c)
        mutations = [{"path": path} for path in ("../escape", "./../escape", "./a//b", "./a\\b", "./a\x00",
                     "./a\u0085", "./" + "x" * 1025)]
        mutations += [{"byte_length": backend.SELECTED_FILE + 1}, {"byte_length": True},
                      {"directory": True, "byte_length": 1}]
        for mutation in mutations:
            value = copy.deepcopy(good)
            value["inventory"][0].update(mutation)
            value["inventory"].sort(key=lambda item: item["path"])
            c.runner.override = (0, backend.encoded(value))
            with self.subTest(mutation=mutation), self.assertRaisesRegex(backend.SetupError, "mcpb_inspection_unavailable"):
                self.inspect(c)
        self.unchanged(c)

    def test_diagnostics_are_data_and_canonical_errors_do_not_echo_private_text(self):
        c = self.enabled()
        value = self.inspect(c)
        diagnostic = {"scope": "metadata", "message": "Unknown public field ignored"}
        value["files"][0]["inspection"].update(status="accepted_with_diagnostics", diagnostics=[diagnostic])
        value["status"] = "accepted_with_diagnostics"
        c.runner.override = (0, backend.encoded(seal(value)))
        self.assertEqual(self.inspect(c)["files"][0]["inspection"]["diagnostics"], [diagnostic])
        c.runner.override = (2, backend.encoded({"schema_version": 1, "scope": "metadata_only", "status": "invalid",
            "error": {"kind": "invalid_metadata", "path": "PRIVATE_PATH", "message": "PRIVATE_SOURCE"}}))
        with self.assertRaises(backend.SetupError) as caught:
            self.inspect(c)
        self.assertEqual(caught.exception.code, "mcpb_metadata_rejected")
        self.assertNotIn("PRIVATE", json.dumps(backend.setup_error_payload(caught.exception)))
        self.unchanged(c)

    def test_text_bounds_control_characters_and_exact_utf8_lengths(self):
        c = self.enabled()
        good = self.inspect(c)
        for text in ("x" * (backend.SELECTED_INLINE_DOCUMENT + 1), "x\x00y", "x\u0085y"):
            value = copy.deepcopy(good)
            row = value["files"][1]
            row.update(text=text, byte_length=len(text.encode()), source_digest=backend.digest(text.encode()))
            c.runner.override = (0, backend.encoded(seal(value)))
            with self.assertRaisesRegex(backend.SetupError, "mcpb_inspection_unavailable"):
                self.inspect(c)
        self.unchanged(c)

    def test_total_text_limit_applies_even_when_each_file_fits(self):
        c = self.enabled()
        files = ["./part" + str(index) for index in range(4)]
        request = {**self.request, "files": files}
        value = report({**request, "files": ["./manifest.json"]})
        text = "x" * backend.SELECTED_INLINE_DOCUMENT
        for path in files:
            value["files"].append({"path": path, "text": text, "byte_length": len(text),
                "source_digest": backend.digest(text.encode()), "inspection": None})
        value["inventory"] = [{"path": row["path"], "byte_length": row["byte_length"], "directory": False}
                              for row in value["files"]]
        value.update(archive_entry_count=5, inventory_entry_count=5)
        c.runner.override = (0, backend.encoded(seal(value)))
        with self.assertRaisesRegex(backend.SetupError, "mcpb_inspection_unavailable"):
            self.inspect(c, request)
        self.unchanged(c)

    def test_reader_input_and_trusted_tool_mutations_refuse_then_release_lock(self):
        for mutation in ("request", "inspector", "validator"):
            c = self.enabled()
            def mutate(path, kind=mutation):
                target = path if kind == "request" else c.workspace / "selected-tools" / (
                    "package_inspector" if kind == "inspector" else "mcpb")
                target.write_bytes(b"changed while inspecting")
            c.runner.mutate = mutate
            with self.subTest(mutation=mutation), self.assertRaisesRegex(backend.SetupError, "mcpb_inspection_unavailable"):
                self.inspect(c)
            self.assertFalse(c.operation_lock.locked())
            self.unchanged(c)
        c = self.enabled()
        (c.workspace / "selected-tools/package_inspector").write_bytes(b"changed before inspecting")
        with self.assertRaisesRegex(backend.SetupError, "selected_tool_changed"):
            self.inspect(c)
        self.assertEqual(c.runner.inspections, [])

    def test_inspection_serializes_with_other_inspection_and_selected_check(self):
        c = self.enabled()
        c.runner.inspect_release = threading.Event()
        self.addCleanup(c.runner.inspect_release.set)
        outcome = []
        def run():
            try:
                outcome.append(self.inspect(c))
            except BaseException as error:
                outcome.append(error)
        worker = threading.Thread(target=run)
        worker.start()
        try:
            self.assertTrue(c.runner.inspect_entered.wait(3))
            with self.assertRaisesRegex(backend.SetupError, "operation_in_progress"):
                self.inspect(c)
            with self.assertRaisesRegex(backend.SetupError, "operation_in_progress"):
                c.selected_check(self.stage())
            self.assertEqual(len(c.runner.inspections), 1)
        finally:
            c.runner.inspect_release.set()
            worker.join(5)
        self.assertFalse(worker.is_alive())
        self.assertEqual(len(outcome), 1)
        self.assertIsInstance(outcome[0], dict)
        self.assertFalse(c.operation_lock.locked())
        self.unchanged(c)


class InspectionMcpTests(InspectionFixture, unittest.IsolatedAsyncioTestCase):
    async def test_real_mcp_validation_dispatch_annotations_and_disabled_tool(self):
        from mcp import Client
        c = self.enabled()
        owner = adapter.SetupAdapter(c)
        try:
            async with Client(adapter.create_server(owner)) as client:
                tools = {tool.name: tool for tool in (await client.list_tools()).tools}
                tool = tools["setup_inspect_mcpb"]
                self.assertTrue(tool.annotations.read_only_hint)
                self.assertFalse(tool.annotations.open_world_hint)
                for invalid in (dict(self.request, root="/elsewhere"), dict(self.request, files=None),
                                dict(self.request, artifact_sha256="a"), dict(self.request, files=["./../x"])):
                    result = await client.call_tool("setup_inspect_mcpb", invalid)
                    self.assertTrue(result.is_error)
                self.assertEqual(c.runner.inspections, [])
                result = await client.call_tool("setup_inspect_mcpb", self.request)
                self.assertFalse(result.is_error, result)
                self.assertEqual(result.structured_content["artifact_sha256"], self.request["artifact_sha256"])
        finally:
            await owner.aclose()
        self.unchanged(c)
        self.profile["mcpb_inspection"] = False
        self.write_profile()
        disabled = self.enabled()
        owner = adapter.SetupAdapter(disabled)
        try:
            async with Client(adapter.create_server(owner)) as client:
                self.assertNotIn("setup_inspect_mcpb", {tool.name for tool in (await client.list_tools()).tools})
                result = await client.call_tool("setup_inspect_mcpb", self.request)
                self.assertTrue(result.is_error)
                self.assertEqual(result.structured_content, {"error": "unknown_tool"})
        finally:
            await owner.aclose()
        self.assertEqual(disabled.runner.inspections, [])

    async def test_recovery_cannot_inspect_new_staged_archives(self):
        from mcp import Client
        c = self.enabled()
        checked = c.selected_check(self.stage())
        job = c.selected_install(self.selection(checked))["job"]
        await asyncio.to_thread(c.join)
        recovered = self.recover(c, job)
        self.assertNotIn("mcpb_inspection", recovered.state())
        with self.assertRaisesRegex(backend.SetupError, "recovery_read_only"):
            self.inspect(recovered)
        owner = adapter.SetupAdapter(recovered)
        try:
            async with Client(adapter.create_server(owner)) as client:
                self.assertNotIn("setup_inspect_mcpb", {tool.name for tool in (await client.list_tools()).tools})
                result = await client.call_tool("setup_inspect_mcpb", self.request)
                self.assertTrue(result.is_error)
                self.assertEqual(result.structured_content, {"error": "unknown_tool"})
        finally:
            await owner.aclose()
        self.assertEqual(recovered.runner.calls, [])
        self.assertEqual(recovered.job["status"], "unconfirmed")


if __name__ == "__main__":
    unittest.main()
