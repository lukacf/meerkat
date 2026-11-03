"""Ordinary plugin evidence through setup; canonical path/parser tests live in Rust.

The controlled inspector only supplies its typed envelope. These tests exercise
actual setup dispatch, evidence custody, selected installation and recovery.
"""
import asyncio
import copy
import json
from pathlib import Path
import shutil
import threading
import unittest

from test_selected_setup import SelectedFixture, SelectedRunner, adapter, backend


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()


class PluginRunner(SelectedRunner):
    def __init__(self, workspace):
        super().__init__(workspace)
        self.inspections = []
        self.override = None
        self.inspection_block = None
        self.inspection_entered = threading.Event()

    def run_compose(self, argv, *, input_file, input_sha256, output_limit, timeout):
        raw = input_file.read_bytes()
        assert backend.digest(raw) == input_sha256
        self.inspections.append((list(argv), raw))
        self.inspection_entered.set()
        if self.inspection_block:
            self.inspection_block.wait(10)
        if self.override is not None:
            return self.override
        request = backend.decode(raw)
        root = Path(request["root"]) / request["directory"]
        rows = []
        for path in request["files"]:
            data = (root / path).read_bytes()
            report = None
            if path in ("./plugin.json", "./mcp.json"):
                kind = "agent-plugin" if path == "./plugin.json" else "mcp-servers"
                report = {"schema_version": 1, "scope": "metadata_only", "format": kind,
                          "status": "accepted", "source_digest": backend.digest(data),
                          "summary": {"name": "ordinary-plugin"} if kind == "agent-plugin" else {"servers": []},
                          "diagnostics": []}
            rows.append({"path": path, "text": data.decode(), "byte_length": len(data),
                         "source_digest": backend.digest(data), "inspection": report})
        return 0, backend.encoded({"schema_version": 1, "scope": "metadata_only", "format": "agent-plugin-files",
            "status": "accepted", "source_digest": backend.digest(canonical(rows)), "files": rows})


class PluginFixture(SelectedFixture):
    def setUp(self):
        super().setUp()
        self.profile["package_inspector"] = self.tools["python"]
        self.profile["plugin_inspection"] = True
        self.write_profile()
        self.plugin = self.staging / "ordinary"
        (self.plugin / "skills" / "guide").mkdir(parents=True)
        (self.plugin / "plugin.json").write_text('{"name":"ordinary-plugin","description":"caf\u00e9"}\n')
        (self.plugin / "mcp.json").write_text('{"mcpServers":{}}\n')
        (self.plugin / "skills/guide/SKILL.md").write_text("# Guide\nReview the selected artifact before installation.\n")
        self.request = {"staged_id": "ordinary", "files": ["./plugin.json", "./mcp.json", "./skills/guide/SKILL.md"]}

    def enabled(self):
        return self.controller(runner_factory=PluginRunner)

    def observe(self, controller):
        return controller.inspect_plugin(copy.deepcopy(self.request))


class PluginTests(PluginFixture, unittest.TestCase):
    def test_exact_utf8_evidence_is_retained_and_replay_reuses_reference(self):
        c = self.enabled()
        result = self.observe(c)
        self.assertEqual(result, self.observe(c))
        reference = result["plugin_source"]
        raw = c.plugin_source_bytes(reference)
        retained = backend.decode(raw)
        self.assertEqual(reference["inspection_sha256"], backend.digest(raw))
        self.assertEqual(retained["report"], {k: v for k, v in result.items() if k != "plugin_source"})
        self.assertEqual(retained["inspector_sha256"], self.tools["python"]["sha256"])
        self.assertEqual(result["files"][0]["text"], (self.plugin / "plugin.json").read_text())
        argv, invocation = c.runner.inspections[0]
        self.assertEqual(argv[1:], ["agent-plugin-files"])
        self.assertEqual(backend.decode(invocation), {"root": str(self.staging), "directory": "./ordinary", "files": self.request["files"]})
        self.assertNotIn(str(self.staging), json.dumps(result))
        self.assertEqual(c.runner.calls, [])
        self.assertIsNone(c.job)
        self.assertEqual(list((c.workspace / "tmp").iterdir()), [])

    def test_evidence_follows_selected_install_and_fresh_recovery_without_reopening_source(self):
        c = self.enabled()
        result = self.observe(c)
        source = result["plugin_source"]
        request = {**self.stage(), "plugin_source": source}
        checked = c.selected_check(request)
        self.assertEqual(checked, c.selected_check(request))
        snapshot = c.workspace / "selections" / checked["validation_id"]
        self.assertEqual((snapshot / "plugin-source.json").read_bytes(), c.plugin_source_bytes(source))
        self.assertNotIn("plugin-source.json", checked["files"])
        shutil.rmtree(self.plugin)
        job = c.selected_install(self.selection(checked))["job"]
        c.join()
        self.assertEqual(c.jobs[job["id"]]["status"], "succeeded")
        self.assertEqual(c.jobs[job["id"]]["result"]["plugin_source"], source)
        recovered = self.recover(c, job)
        result = recovered.reconcile({"job_id": job["id"]})
        self.assertEqual(result["observation"]["status"], "matched")
        self.assertEqual(result["observation"]["result"]["plugin_source"], source)
        self.assertNotIn("plugin_inspection", recovered.state())
        before = list(recovered.runner.calls)
        recalled = recovered.plugin_source({"job_id": job["id"]})
        self.assertEqual(recalled, {**self.observe_from_snapshot(c, checked), "plugin_source": source, "job_id": job["id"]})
        self.assertEqual(recovered.runner.calls, before)

    def observe_from_snapshot(self, controller, checked):
        path = controller.workspace / "selections" / checked["validation_id"] / "plugin-source.json"
        return backend.decode(path.read_bytes())["report"]

    def test_recovered_source_refuses_wrong_job_changed_sidecar_and_changed_request(self):
        c = self.enabled()
        observed = self.observe(c)
        checked = c.selected_check({**self.stage(), "plugin_source": observed["plugin_source"]})
        job = c.selected_install(self.selection(checked))["job"]
        c.join()
        recovered = self.recover(c, job)
        shutil.rmtree(self.plugin)
        with self.assertRaisesRegex(backend.SetupError, "plugin_source_unavailable"):
            recovered.plugin_source({"job_id": "0" * 32})
        sidecar = c.workspace / "selections" / checked["validation_id"] / "plugin-source.json"
        old = sidecar.read_bytes()
        sidecar.write_bytes(b"changed")
        with self.assertRaisesRegex(backend.SetupError, "plugin_source_changed"):
            recovered.plugin_source({"job_id": job["id"]})
        sidecar.write_bytes(old)
        request = c.workspace / "requests" / (job["id"] + ".json")
        request.write_bytes(b"changed")
        with self.assertRaisesRegex(backend.SetupError, "plugin_source_changed"):
            recovered.plugin_source({"job_id": job["id"]})
        self.assertEqual(recovered.runner.calls, [])
        self.assertEqual(recovered.job["status"], "unconfirmed")

    def test_selected_job_without_attached_plugin_has_no_source(self):
        c = self.enabled()
        checked = c.selected_check(self.stage())
        job = c.selected_install(self.selection(checked))["job"]
        c.join()
        with self.assertRaisesRegex(backend.SetupError, "plugin_source_unavailable"):
            c.plugin_source({"job_id": job["id"]})

    def test_inline_selection_and_retry_copy_exact_source_without_claiming_installer_inputs(self):
        c = self.enabled()
        source = self.observe(c)["plugin_source"]
        request = self.stage()
        stage = self.staging / request["staged_id"]
        inline = {key: request[key] for key in ("profile", "staged_id", "component")}
        inline.update(package_json=(stage / "package.json").read_text(),
                      activation_json=(stage / "activation.json").read_text(), plugin_source=source)
        checked = c.selected_check(inline)
        c.runner.nonzero = True
        job = c.selected_install(self.selection(checked))["job"]
        c.join()
        self.assertEqual(c.jobs[job["id"]]["status"], "unconfirmed")
        shutil.rmtree(self.staging)
        retry = c.selected_retry({"job_id": job["id"]})
        original = c.workspace / "selections" / checked["validation_id"] / "plugin-source.json"
        successor = c.workspace / "selections" / retry["validation_id"] / "plugin-source.json"
        self.assertEqual(original.read_bytes(), successor.read_bytes())
        self.assertEqual(retry["plugin_source"], source)
        c.runner.nonzero = False
        c.selected_install(self.selection(retry))
        c.join()
        self.assertEqual(c.job["status"], "succeeded")

    def test_foreign_reference_and_changed_evidence_fail_before_validation_or_install(self):
        c = self.enabled()
        source = self.observe(c)["plugin_source"]
        request = {**self.stage(), "plugin_source": source}
        foreign = self.enabled()
        with self.assertRaisesRegex(backend.SetupError, "plugin_source_unavailable"):
            foreign.selected_check(request)
        self.assertEqual(foreign.runner.calls, [])
        checked = c.selected_check(request)
        sidecar = c.workspace / "selections" / checked["validation_id"] / "plugin-source.json"
        original = sidecar.read_bytes()
        sidecar.write_bytes(b"changed")
        with self.assertRaisesRegex(backend.SetupError, "selected_proposal_changed"):
            c.selected_install(self.selection(checked))
        self.assertIsNone(c.job)
        sidecar.write_bytes(original)
        retained = c.workspace / "plugin-inspections" / (source["inspection_id"] + ".json")
        retained.write_bytes(b"changed")
        with self.assertRaisesRegex(backend.SetupError, "plugin_source_changed"):
            c.selected_check(request)

    def test_inspection_bounds_missing_optin_and_changed_inspector(self):
        c = self.enabled()
        for request in ({**self.request, "files": []}, {**self.request, "files": ["./mcp.json"]},
                        {**self.request, "files": ["./plugin.json"] * 2}, {**self.request, "staged_id": "../ordinary"},
                        {**self.request, "files": ["./plugin.json", 42]}, {**self.request, "root": "/"}):
            with self.subTest(request=request), self.assertRaises(backend.SetupError):
                c.inspect_plugin(request)
        self.assertEqual(c.runner.inspections, [])
        self.profile.pop("package_inspector")
        self.profile.pop("plugin_inspection")
        self.write_profile()
        disabled = self.controller()
        with self.assertRaisesRegex(backend.SetupError, "plugin_inspection_unavailable"):
            disabled.inspect_plugin(self.request)
        (c.workspace / "selected-tools/package_inspector").write_bytes(b"changed")
        with self.assertRaisesRegex(backend.SetupError, "selected_tool_changed"):
            c.inspect_plugin(self.request)
        self.assertEqual(c.runner.inspections, [])

    def test_host_must_explicitly_enable_new_inspector_contract(self):
        for value in (None, False):
            if value is None:
                self.profile.pop("plugin_inspection", None)
            else:
                self.profile["plugin_inspection"] = value
            self.write_profile()
            c = self.enabled()
            self.assertNotIn("plugin_inspection", c.state())
            with self.assertRaisesRegex(backend.SetupError, "plugin_inspection_unavailable"):
                self.observe(c)
            self.assertEqual(c.runner.inspections, [])
        for value in (1, "true", None):
            self.profile["plugin_inspection"] = value
            self.write_profile()
            with self.assertRaisesRegex(backend.SetupError, "invalid_selected_host_profile"):
                self.enabled()
        self.profile["plugin_inspection"] = True
        self.profile.pop("package_inspector")
        self.write_profile()
        with self.assertRaisesRegex(backend.SetupError, "invalid_selected_host_profile"):
            self.enabled()

    def test_canonical_refusal_is_fixed_and_malformed_evidence_never_retains(self):
        c = self.enabled()
        c.runner.override = (2, backend.encoded({"schema_version": 1, "scope": "metadata_only", "status": "invalid",
            "error": {"kind": "invalid_metadata", "path": "PRIVATE", "message": "PRIVATE"}}))
        with self.assertRaises(backend.SetupError) as caught:
            self.observe(c)
        self.assertEqual(caught.exception.code, "plugin_metadata_rejected")
        self.assertNotIn("PRIVATE", json.dumps(backend.setup_error_payload(caught.exception)))
        self.assertEqual(c.plugin_inspections, {})
        c.runner.override = None
        good = self.observe(c)
        good.pop("plugin_source")
        for mutate in (lambda v: v.update(source_digest="f" * 64),
                       lambda v: v["files"][0].update(text="changed"),
                       lambda v: v["files"][0].update(byte_length=True),
                       lambda v: v["files"][0]["inspection"].update(source_digest="f" * 64),
                       lambda v: v["files"].append(v["files"][0]),
                       lambda v: v.update(root="PRIVATE")):
            bad = copy.deepcopy(good)
            mutate(bad)
            c.runner.override = (0, backend.encoded(bad))
            with self.assertRaisesRegex(backend.SetupError, "plugin_inspection_unavailable"):
                self.observe(c)
        self.assertEqual(len(c.plugin_inspections), 1)
        c.selected_attempts = 128
        with self.assertRaisesRegex(backend.SetupError, "new_workspace_required"):
            self.observe(c)

    def test_diagnostics_preserved_and_skill_only_remains_nonexecutable(self):
        c = self.enabled()
        only = {"staged_id": "ordinary", "files": ["./plugin.json", "./skills/guide/SKILL.md"]}
        result = c.inspect_plugin(only)
        self.assertIsNone(result["files"][1]["inspection"])
        self.assertIsNone(c.job)
        value = {k: v for k, v in result.items() if k != "plugin_source"}
        value["files"][0]["inspection"].update(status="accepted_with_diagnostics",
            diagnostics=[{"scope": "unrecognized", "message": "Unknown top-level field ignored"}])
        value["status"] = "accepted_with_diagnostics"
        value["source_digest"] = backend.digest(canonical(value["files"]))
        c.runner.override = (0, backend.encoded(value))
        observed = c.inspect_plugin(only)
        self.assertEqual(observed["status"], "accepted_with_diagnostics")
        self.assertEqual(observed["files"][0]["inspection"]["diagnostics"], value["files"][0]["inspection"]["diagnostics"])
        self.assertIsNone(c.job)


def inventory_report(directory="./", rows=None):
    return {"schema_version": 1, "scope": "metadata_only", "format": "agent-plugin-files",
            "status": "inventoried", "directory": directory,
            "inventory": rows if rows is not None else [
                {"path": "./plugin.json", "kind": "file", "byte_length": 17},
                {"path": "./skills", "kind": "directory"}], "complete": True}


class PluginInventoryTests(PluginFixture, unittest.TestCase):
    def test_inventory_uses_pinned_reader_without_source_retention(self):
        c = self.enabled()
        report = inventory_report()
        c.runner.override = (0, backend.encoded(report))
        result = c.inspect_plugin({"staged_id": "ordinary", "inventory": "./"})
        self.assertEqual(result, report)
        argv, invocation = c.runner.inspections[0]
        self.assertEqual(argv, [str(c.workspace / "selected-tools/package_inspector"), "agent-plugin-files"])
        self.assertEqual(backend.decode(invocation),
            {"root": str(self.staging), "directory": "./ordinary", "inventory": "./"})
        self.assertEqual(c.plugin_inspections, {})
        self.assertEqual(list((c.workspace / "plugin-inspections").iterdir()), [])
        self.assertEqual(list((c.workspace / "tmp").iterdir()), [])
        self.assertEqual(c.runner.calls, [])
        self.assertIsNone(c.job)
        self.assertFalse(c.operation_lock.locked())

    def test_inventory_request_is_exclusive_and_bounded_before_reader_entry(self):
        c = self.enabled()
        request = {"staged_id": "ordinary", "inventory": "./"}
        for value in (None, 1, "", ".", "/", "skills", "./skills/", "./a/../b", "./a/./b",
                      "./a//b", "./a:b", "./a\\b", "./bad\n", "./bad\x85", "./bad\ud800",
                      "./" + "a/" * 15 + "b", "./" + "\u00e9" * 512):
            with self.subTest(inventory=repr(value)), self.assertRaises(backend.SetupError):
                c.inspect_plugin({**request, "inventory": value})
        for value in ({**request, "files": ["./plugin.json"]}, {"staged_id": "ordinary"},
                      {**request, "root": "/"}, {**request, "directory": "./other"},
                      {**request, "staged_id": "../ordinary"}):
            with self.subTest(request=value), self.assertRaises(backend.SetupError):
                c.inspect_plugin(value)
        self.assertEqual(c.runner.inspections, [])
        self.assertEqual(c.plugin_inspections, {})

    def test_inventory_accepts_exact_bounds_and_content_free_kinds(self):
        c = self.enabled()
        cases = [("./", []), ("./", [
            {"path": "./a", "kind": "directory"}, {"path": "./b", "kind": "symlink"},
            {"path": "./c", "kind": "special"},
            {"path": "./d", "kind": "file", "byte_length": 2**64 - 1}]),
            ("./", [{"path": "./" + str(i).zfill(3) + "a" * 251, "kind": "file", "byte_length": 0}
                    for i in range(256)]),
            ("./" + "/".join(["a"] * 15), [{"path": "./" + "/".join(["a"] * 16), "kind": "directory"}]),
            ("./", [{"path": "./" + "\u00e9" * 511, "kind": "file", "byte_length": 0}])]
        for directory, rows in cases:
            report = inventory_report(directory, rows)
            c.runner.override = (0, backend.encoded(report))
            with self.subTest(directory=directory, count=len(rows)):
                self.assertEqual(c.inspect_plugin({"staged_id": "ordinary", "inventory": directory}), report)
        self.assertEqual(c.plugin_inspections, {})

    def test_inventory_refuses_unsafe_or_partial_reader_output_without_retention(self):
        c = self.enabled()
        good = inventory_report("./skills", [
            {"path": "./skills/a", "kind": "directory"},
            {"path": "./skills/b", "kind": "file", "byte_length": 0}])
        mutations = [
            lambda v: v.update(complete=False), lambda v: v.update(complete=1),
            lambda v: v.update(directory="./other"), lambda v: v.update(status="accepted"),
            lambda v: v.update(source_digest="f" * 64), lambda v: v.update(files=[]),
            lambda v: v.update(plugin_source={}), lambda v: v["inventory"].reverse(),
            lambda v: v["inventory"].append(v["inventory"][0]),
            lambda v: v["inventory"][0].update(path="./skills"),
            lambda v: v["inventory"][0].update(path="./skills/a/nested"),
            lambda v: v["inventory"][0].update(path="./other/a"),
            lambda v: v["inventory"][0].update(path="./skills/../a"),
            lambda v: v["inventory"][0].update(path="./skills/a\n"),
            lambda v: v["inventory"][0].update(path="./skills/a:b"),
            lambda v: v["inventory"][0].update(kind="unknown"),
            lambda v: v["inventory"][0].update(byte_length=0),
            lambda v: v["inventory"][1].update(byte_length=True),
            lambda v: v["inventory"][1].update(byte_length=-1),
            lambda v: v["inventory"][1].update(byte_length=2**64),
            lambda v: v["inventory"][1].pop("byte_length"),
            lambda v: v["inventory"][1].update(text="PRIVATE"),
            lambda v: v.update(inventory=[{"path": "./skills/" + str(i).zfill(3), "kind": "directory"}
                                         for i in range(257)]),
            lambda v: v.update(inventory=[{"path": "./skills/" + "a" * 1016, "kind": "directory"}]),
            lambda v: v.update(inventory=[{"path": "./skills/" + str(i).zfill(3) + "a" * 245,
                                          "kind": "directory"} for i in range(256)]),
        ]
        for index, mutate in enumerate(mutations):
            report = copy.deepcopy(good)
            mutate(report)
            c.runner.override = (0, backend.encoded(report))
            with self.subTest(index=index), self.assertRaisesRegex(backend.SetupError, "plugin_inspection_unavailable"):
                c.inspect_plugin({"staged_id": "ordinary", "inventory": "./skills"})
        c.runner.override = (2, backend.encoded({"schema_version": 1, "scope": "metadata_only", "status": "invalid",
            "error": {"kind": "invalid_metadata", "path": "PRIVATE", "message": "PRIVATE"}}))
        with self.assertRaises(backend.SetupError) as caught:
            c.inspect_plugin({"staged_id": "ordinary", "inventory": "./skills"})
        self.assertEqual(caught.exception.code, "plugin_inspection_unavailable")
        self.assertNotIn("PRIVATE", json.dumps(backend.setup_error_payload(caught.exception)))
        self.assertEqual(c.plugin_inspections, {})
        self.assertEqual(list((c.workspace / "plugin-inspections").iterdir()), [])

    def test_inventory_keeps_existing_admission_and_input_tool_binding(self):
        for corruption in ("input", "tool"):
            c = self.enabled()
            c.runner.override = (0, backend.encoded(inventory_report()))
            original = c.runner.run_compose

            def changed(argv, **kwargs):
                result = original(argv, **kwargs)
                path = kwargs["input_file"] if corruption == "input" else Path(argv[0])
                path.write_bytes(b"changed")
                return result

            c.runner.run_compose = changed
            with self.subTest(corruption=corruption), self.assertRaisesRegex(backend.SetupError, "plugin_inspection_unavailable"):
                c.inspect_plugin({"staged_id": "ordinary", "inventory": "./"})
            self.assertEqual(c.plugin_inspections, {})
            self.assertFalse(c.operation_lock.locked())
        c = self.enabled()
        c.operation_lock.acquire()
        try:
            with self.assertRaisesRegex(backend.SetupError, "operation_in_progress"):
                c.inspect_plugin({"staged_id": "ordinary", "inventory": "./"})
        finally:
            c.operation_lock.release()
        c.recovery_only = True
        with self.assertRaises(backend.SetupError):
            c.inspect_plugin({"staged_id": "ordinary", "inventory": "./"})
        self.assertEqual(c.runner.inspections, [])


class PluginMcpTests(PluginFixture, unittest.IsolatedAsyncioTestCase):
    async def test_actual_mcp_inspects_then_binds_source_to_selected_install(self):
        from mcp import Client
        c = self.enabled()
        owner = adapter.SetupAdapter(c)
        try:
            async with Client(adapter.create_server(owner)) as client:
                self.assertIn("setup_inspect_plugin", {tool.name for tool in (await client.list_tools()).tools})
                refused = await client.call_tool("setup_inspect_plugin", {**self.request, "root": "/"})
                self.assertTrue(refused.is_error)
                self.assertEqual(c.runner.inspections, [])
                observed = await client.call_tool("setup_inspect_plugin", self.request)
                self.assertFalse(observed.is_error, observed)
                request = {**self.stage(), "plugin_source": observed.structured_content["plugin_source"]}
                checked = await client.call_tool("setup_selected_check", request)
                self.assertFalse(checked.is_error, checked)
                installed = await client.call_tool("setup_selected_install", self.selection(checked.structured_content))
                self.assertFalse(installed.is_error, installed)
                c.join()
                self.assertEqual(c.job["status"], "succeeded")
        finally:
            await owner.aclose()
        observed_source = observed.structured_content
        job = c.job
        shutil.rmtree(self.plugin)
        recovered = self.recover(c, job)
        recovery_owner = adapter.SetupAdapter(recovered)
        try:
            server = adapter.create_server(recovery_owner)
            self.assertIn("setup_plugin_source", server.create_initialization_options().instructions)
            async with Client(server) as client:
                self.assertEqual({tool.name for tool in (await client.list_tools()).tools},
                    {"setup_recovery_manifest", "setup_state", "setup_reconcile", "setup_plugin_source"})
                source = await client.call_tool("setup_plugin_source", {"job_id": job["id"]})
                self.assertFalse(source.is_error, source)
                self.assertEqual(source.structured_content, {**observed_source, "job_id": job["id"]})
                self.assertEqual(recovered.runner.calls, [])
        finally:
            await recovery_owner.aclose()

    async def test_actual_mcp_inventory_is_exclusive_and_rejection_is_not_metadata_diagnosis(self):
        from mcp import Client
        c = self.enabled()
        c.runner.override = (0, backend.encoded(inventory_report()))
        owner = adapter.SetupAdapter(c)
        try:
            async with Client(adapter.create_server(owner)) as client:
                tool = next(t for t in (await client.list_tools()).tools if t.name == "setup_inspect_plugin")
                self.assertEqual(set(tool.input_schema["properties"]), {"staged_id", "inventory", "files"})
                self.assertNotIn("oneOf", tool.input_schema)
                request = {"staged_id": "ordinary", "inventory": "./"}
                for invalid in ({**request, "files": ["./plugin.json"]}, {"staged_id": "ordinary"}):
                    refused = await client.call_tool("setup_inspect_plugin", invalid)
                    self.assertTrue(refused.is_error)
                self.assertEqual(c.runner.inspections, [])
                result = await client.call_tool("setup_inspect_plugin", request)
                self.assertFalse(result.is_error, result)
                self.assertEqual(result.structured_content, inventory_report())
                c.runner.override = (2, backend.encoded({"schema_version": 1, "scope": "metadata_only", "status": "invalid",
                    "error": {"kind": "invalid_metadata", "path": "PRIVATE", "message": "PRIVATE"}}))
                refused = await client.call_tool("setup_inspect_plugin", request)
                self.assertTrue(refused.is_error)
                self.assertEqual(refused.structured_content, {"error": "plugin_inspection_unavailable"})
                self.assertEqual(c.plugin_inspections, {})
        finally:
            await owner.aclose()

    async def test_cancelled_inventory_joins_without_retaining_names_as_source(self):
        c = self.enabled()
        c.runner.override = (0, backend.encoded(inventory_report()))
        c.runner.inspection_block = threading.Event()
        owner = adapter.SetupAdapter(c)
        pending = asyncio.create_task(owner.dispatch("/api/inspect_plugin", {"staged_id": "ordinary", "inventory": "./"}))
        self.assertTrue(await asyncio.to_thread(c.runner.inspection_entered.wait, 2))
        pending.cancel()
        c.runner.inspection_block.set()
        try:
            with self.assertRaises(asyncio.CancelledError):
                await pending
        finally:
            await owner.aclose()
        self.assertEqual(c.plugin_inspections, {})
        self.assertEqual(c.runner.calls, [])
        self.assertIsNone(c.job)
        self.assertFalse(c.operation_lock.locked())
        self.assertEqual(list((c.workspace / "tmp").iterdir()), [])

    async def test_cancelled_call_settles_only_evidence_and_does_not_start_install(self):
        c = self.enabled()
        c.runner.inspection_block = threading.Event()
        owner = adapter.SetupAdapter(c)
        pending = asyncio.create_task(owner.dispatch("/api/inspect_plugin", self.request))
        await asyncio.to_thread(c.runner.inspection_entered.wait, 2)
        pending.cancel()
        c.runner.inspection_block.set()
        try:
            with self.assertRaises(asyncio.CancelledError):
                await pending
        finally:
            await owner.aclose()
        self.assertEqual(len(c.plugin_inspections), 1)
        self.assertEqual(c.runner.calls, [])
        self.assertIsNone(c.job)
        self.assertFalse(c.operation_lock.locked())


if __name__ == "__main__":
    unittest.main()
