"""Inline metadata custody through the existing selected proposal owner."""
import copy
import json
import os
import sys
import unittest
from unittest.mock import patch

from test_selected_setup import SelectedFixture, SelectedRunner, adapter, backend
from test_selected_mcpb import McpbFixture


class InlineRunner(SelectedRunner):
    def __init__(self, workspace):
        super().__init__(workspace)
        self.inspection_output = None
        self.inspection_code = 0
        self.inspection_calls = []

    def run_compose(self, argv, *, input_file, input_sha256, output_limit, timeout):
        raw = input_file.read_bytes()
        assert backend.digest(raw) == input_sha256
        self.inspection_calls.append((list(argv), raw, output_limit, timeout))
        return self.inspection_code, self.inspection_output or backend.encoded({
            "schema_version": 1, "scope": "metadata_only", "format": "integration-package",
            "status": "accepted", "source_digest": input_sha256, "diagnostics": [],
            "summary": {"identity": {"publisher": "fixture", "name": "chosen", "version": "1"},
                        "package_content_digest": "a" * 64,
                        "strategies": [{"id": "local", "kind": "local_mcp"}],
                        "exports": [{"id": "service", "role": "service_connector"}]}})


class InlineTests(SelectedFixture, unittest.TestCase):
    def inline(self, profile="python", name="choice", *, remove=True):
        old = self.stage(profile, name)
        source = self.staging / name
        result = {key: old[key] for key in ("profile", "staged_id", "component")}
        for field, filename in (("package_json", "package.json"), ("activation_json", "activation.json")):
            result[field] = (source / filename).read_text()
            if remove:
                (source / filename).unlink()
        return old, result

    def software_inline(self, profile="python", name="choice"):
        old = self.software_stage(profile, name)
        source = self.staging / name
        request = {key: old[key] for key in ("profile", "staged_id", "component")}
        for field, filename in (("package_json", "package.json"), ("declaration_json", "declaration.json")):
            request[field] = (source / filename).read_text()
            (source / filename).unlink()
        return old, request

    def test_inline_software_normalizes_explicit_form_and_refuses_ambiguous_inputs(self):
        for profile in ("python", "node"):
            c = self.controller()
            old, request = self.software_inline(profile, "inline-software-" + profile)
            for change in ({"document_kind": "local_software_declaration"}, {"activation_json": "{}"},
                           {"selection_sha256": old["selection_sha256"]}, {"configuration_json": "{}"},
                           {"profile": "mcpb"}, {"declaration_json": None}):
                with self.subTest(profile=profile, change=change), self.assertRaises(backend.SetupError):
                    c.selected_check({**request, **change})
            self.assertEqual(c.runner.calls, [])
            checked = c.selected_check(request)
            snapshot = c.workspace / "selections" / checked["validation_id"]
            self.assertEqual((snapshot / "declaration.json").read_bytes(), request["declaration_json"].encode())
            self.assertEqual(backend.decode((snapshot / "effect.json").read_bytes())["request"], old)
            self.assertEqual(c.selected_check(copy.deepcopy(request)), checked)
            self.assertEqual(sorted(p.name for p in (self.staging / old["staged_id"]).iterdir()),
                             ["artifact.whl" if profile == "python" else "artifact.tgz", "closure"])

    def inspection_controller(self):
        self.profile["package_inspector"] = self.tools["python"]
        self.write_profile()
        return self.controller(runner_factory=InlineRunner)

    def test_inline_freezes_without_writing_staging_and_replays_with_old_hash_form(self):
        c = self.controller()
        old, request = self.inline()
        first = c.selected_check(request)
        source = self.staging / "choice"
        self.assertEqual(sorted(p.name for p in source.iterdir()), ["artifact.whl", "closure"])
        snapshot = c.workspace / "selections" / first["validation_id"]
        self.assertEqual((snapshot / "package.json").read_bytes(), request["package_json"].encode())
        self.assertEqual(backend.decode((snapshot / "effect.json").read_bytes())["request"], old)
        self.assertEqual(first, c.selected_check(copy.deepcopy(request)))
        self.assertEqual(len(c.runner.calls), 1)
        for field, filename in (("package_json", "package.json"), ("activation_json", "activation.json")):
            (source / filename).write_text(request[field])
        self.assertEqual(first, c.selected_check(old))

    def test_inline_ignores_staged_metadata_but_rechecks_artifacts(self):
        c = self.controller()
        _, request = self.inline(remove=False)
        source = self.staging / "choice"
        (source / "package.json").write_text("different private staged metadata")
        first = c.selected_check(request)
        self.assertEqual((source / "package.json").read_text(), "different private staged metadata")
        self.assertEqual(first, c.selected_check(request))
        (source / "artifact.whl").write_bytes(b"changed")
        with self.assertRaises(backend.SetupError):
            c.selected_check(request)
        job = c.selected_install(self.selection(first))["job"]
        c.join()
        self.assertEqual(c.jobs[job["id"]]["status"], "succeeded")
        recovered = self.recover(c, job)
        self.assertEqual(recovered.reconcile({"job_id": job["id"]})["observation"]["status"], "matched")

    def test_mixed_missing_and_oversized_inline_requests_leave_no_snapshot(self):
        c = self.controller()
        _, valid = self.inline()
        changes = [{"package_sha256": "a" * 64}, {"configuration_json": "{}"},
                   {"package_json": None}, {"package_json": "x" * (32 * 1024 + 1)},
                   {"package_json": "\\" * 32000, "activation_json": "\\" * 32000}]
        for changed in changes:
            with self.subTest(changed=list(changed)):
                with self.assertRaises(backend.SetupError):
                    c.selected_check({**valid, **changed})
        missing = dict(valid)
        del missing["activation_json"]
        with self.assertRaises(backend.SetupError):
            c.selected_check(missing)
        self.assertEqual(list((c.workspace / "selections").iterdir()), [])
        self.assertEqual(c.runner.calls, [])

    def test_inspection_requires_optional_pinned_owner_and_keeps_exact_utf8(self):
        c = self.controller()
        with self.assertRaises(backend.SetupError):
            c.inspect_package({"package_json": "{}"})
        enabled = self.inspection_controller()
        raw = '{"description":"caf\u00e9"}\n'
        result = enabled.inspect_package({"package_json": raw})
        self.assertEqual(result["source_digest"], backend.digest(raw.encode()))
        self.assertEqual(result["summary"]["package_content_digest"], "a" * 64)
        self.assertEqual(enabled.runner.inspection_calls[0][1], raw.encode())
        self.assertEqual(enabled.runner.inspection_calls[0][0][1:], ["integration-package"])
        self.assertIn("package_inspector", enabled.selected_binding["tools"])
        self.assertNotIn("package_inspector", c.selected_binding["tools"])
        self.assertIsNone(enabled.job)

    def test_inspection_rejects_wrong_raw_digest_unknown_output_and_nonzero(self):
        c = self.inspection_controller()
        accepted = c.inspect_package({"package_json": "{}"})
        for mutate in (lambda r: r.update(source_digest="f" * 64),
                       lambda r: r.update(private="DO_NOT_REFLECT"),
                       lambda r: r["summary"].update(private="DO_NOT_REFLECT")):
            value = copy.deepcopy(accepted)
            mutate(value)
            c.runner.inspection_output = backend.encoded(value)
            with self.assertRaises(backend.SetupError) as caught:
                c.inspect_package({"package_json": "{}"})
            self.assertNotIn("DO_NOT_REFLECT", str(caught.exception))
        c.runner.inspection_output = b'private failure'
        c.runner.inspection_code = 2
        with self.assertRaises(backend.SetupError):
            c.inspect_package({"package_json": "{}"})

    def test_canonical_rejection_has_fixed_help_and_budget_is_shared(self):
        c = self.inspection_controller()
        c.runner.inspection_code = 2
        c.runner.inspection_output = backend.encoded({"schema_version": 1, "scope": "metadata_only",
            "status": "invalid", "error": {"kind": "invalid_metadata", "path": "PRIVATE", "message": "PRIVATE"}})
        with self.assertRaises(backend.SetupError) as caught:
            c.inspect_package({"package_json": "private invalid input"})
        public = backend.setup_error_payload(caught.exception)
        self.assertEqual(public["error"], "selected_invalid_package")
        self.assertIn("diagnostic", public)
        self.assertNotIn("PRIVATE", json.dumps(public))
        self.assertEqual(list((c.workspace / "tmp").iterdir()), [])
        c.selected_attempts = 128
        with self.assertRaisesRegex(backend.SetupError, "new_workspace_required"):
            c.inspect_package({"package_json": "{}"})
        _, inline = self.inline()
        with self.assertRaisesRegex(backend.SetupError, "new_workspace_required"):
            c.selected_check(inline)

    def test_inspector_mismatch_fails_before_workspace_and_unsupported_runner_cannot_bypass(self):
        self.profile["package_inspector"] = {**self.tools["python"], "sha256": "0" * 64}
        self.write_profile()
        with self.assertRaisesRegex(backend.SetupError, "invalid_selected_host_profile"):
            self.controller()
        self.assertFalse((self.root / "work-1").exists())
        self.profile["package_inspector"] = self.tools["python"]
        self.write_profile()
        c = self.controller()
        with self.assertRaisesRegex(backend.SetupError, "package_inspection_unavailable"):
            c.inspect_package({"package_json": "{}"})
        self.assertEqual(c.runner.calls, [])


class InlineMcpbTests(McpbFixture, unittest.TestCase):
    def test_inline_mcpb_configuration_is_exact_and_missing_or_mixed_refuses(self):
        c = self.controller()
        original = self.stage()
        source = self.staging / "choice"
        request = {key: original[key] for key in ("profile", "staged_id", "component")}
        for field, name in (("package_json", "package.json"), ("activation_json", "activation.json"),
                            ("configuration_json", "configuration.json")):
            request[field] = (source / name).read_text()
            (source / name).unlink()
        missing = dict(request)
        del missing["configuration_json"]
        for bad in (missing, {**request, "configuration_sha256": original["configuration_sha256"]}):
            with self.assertRaises(backend.SetupError):
                c.selected_check(bad)
        first = c.selected_check(request)
        self.assertEqual(first, c.selected_check(request))
        self.assertEqual(sorted(p.name for p in source.iterdir()), ["artifact.mcpb"])
        snapshot = c.workspace / "selections" / first["validation_id"]
        self.assertEqual((snapshot / "configuration.json").read_bytes(), request["configuration_json"].encode())
        self.assertEqual(backend.decode((snapshot / "effect.json").read_bytes())["request"], original)
        c.selected_install(self.selection(first))
        c.join()
        self.assertEqual(c.job["status"], "succeeded")


class InlineMcpTests(SelectedFixture, unittest.IsolatedAsyncioTestCase):
    inline = InlineTests.inline
    software_inline = InlineTests.software_inline
    inspection_controller = InlineTests.inspection_controller

    async def test_actual_mcp_schema_admits_new_inline_and_staged_forms_only_explicitly(self):
        from mcp import Client
        c = self.controller()
        owner = adapter.SetupAdapter(c)
        old, request = self.software_inline("node", "mcp-software")
        try:
            async with Client(adapter.create_server(owner)) as client:
                for change in ({"document_kind": "local_software_declaration"}, {"activation_json": "{}"},
                               {"configuration_json": "{}"}, {"profile": "mcpb"}):
                    refused = await client.call_tool("setup_selected_check", {**request, **change})
                    self.assertTrue(refused.is_error)
                    self.assertEqual(refused.structured_content, {"error": "invalid_arguments"})
                checked = await client.call_tool("setup_selected_check", request)
                self.assertFalse(checked.is_error, checked)
                self.assertEqual(checked.structured_content["document_kind"], "local_software_declaration")
                for field, filename in (("package_json", "package.json"), ("declaration_json", "declaration.json")):
                    (self.staging / old["staged_id"] / filename).write_text(request[field])
                replay = await client.call_tool("setup_selected_check", old)
                self.assertFalse(replay.is_error, replay)
                self.assertEqual(replay.structured_content, checked.structured_content)
        finally:
            await owner.aclose()

    async def test_actual_mcp_inline_and_inspection_optin_and_recovery_surface(self):
        from mcp import Client
        c = self.inspection_controller()
        owner = adapter.SetupAdapter(c)
        _, request = self.inline("node")
        try:
            async with Client(adapter.create_server(owner)) as client:
                self.assertIn("setup_inspect_package", {t.name for t in (await client.list_tools()).tools})
                inspect = await client.call_tool("setup_inspect_package", {"package_json": request["package_json"]})
                self.assertFalse(inspect.is_error, inspect)
                self.assertEqual(inspect.structured_content["source_digest"], backend.digest(request["package_json"].encode()))
                for bad in ({**request, "package_sha256": "a" * 64}, {**request, "package_json": "x" * 32769}):
                    refused = await client.call_tool("setup_selected_check", bad)
                    self.assertTrue(refused.is_error)
                    self.assertEqual(refused.structured_content, {"error": "invalid_arguments"})
                checked = await client.call_tool("setup_selected_check", request)
                self.assertFalse(checked.is_error, checked)
                installed = await client.call_tool("setup_selected_install", self.selection(checked.structured_content))
                c.join()
                job = c.jobs[installed.structured_content["job"]["id"]]
                self.assertEqual(job["status"], "succeeded")
        finally:
            await owner.aclose()
        recovery = self.recover(c, job)
        recovered_owner = adapter.SetupAdapter(recovery)
        try:
            async with Client(adapter.create_server(recovered_owner)) as client:
                self.assertEqual({t.name for t in (await client.list_tools()).tools}, {"setup_recovery_manifest", "setup_state", "setup_reconcile"})
        finally:
            await recovered_owner.aclose()
        self.profile.pop("package_inspector")
        self.write_profile()
        legacy_owner = adapter.SetupAdapter(self.controller())
        try:
            async with Client(adapter.create_server(legacy_owner)) as client:
                self.assertNotIn("setup_inspect_package", {t.name for t in (await client.list_tools()).tools})
        finally:
            await legacy_owner.aclose()


class ProcessRunner(backend.Runner):
    block = None


class InspectionProcessTests(SelectedFixture, unittest.TestCase):
    def process_controller(self, body, runner=ProcessRunner):
        path = self.root / "inspector"
        path.write_text("#!" + sys.executable + "\n" + body + "\n")
        path.chmod(0o700)
        self.profile["package_inspector"] = {"path": str(path), "sha256": backend.digest(path.read_bytes())}
        self.write_profile()
        return self.controller(runner_factory=runner)

    def test_real_runner_reads_exact_stdin_and_joins_failed_child(self):
        script = """import hashlib,json,sys
raw=sys.stdin.buffer.read()
print(json.dumps({'schema_version':1,'scope':'metadata_only','format':'integration-package',
'status':'accepted','source_digest':hashlib.sha256(raw).hexdigest(),'diagnostics':[],
'summary':{'identity':{'publisher':'fixture','name':'chosen','version':'1'},
'package_content_digest':'a'*64,'strategies':[],'exports':[]}}))
"""
        c = self.process_controller(script)
        raw = '{"text":"caf\u00e9"}\n'
        self.assertEqual(c.inspect_package({"package_json": raw})["source_digest"], backend.digest(raw.encode()))
        failed = self.process_controller("import os,time\nopen('entered.pid','w').write(str(os.getpid()))\ntime.sleep(20)")
        with patch.object(backend, "INSPECTION_TIMEOUT", 0.1):
            with self.assertRaisesRegex(backend.SetupError, "package_inspection_unavailable"):
                failed.inspect_package({"package_json": "{}"})
        pid = int((failed.workspace / "entered.pid").read_text())
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)
        self.assertEqual(list((failed.workspace / "tmp").iterdir()), [])
        self.assertTrue(failed.operation_lock.acquire(False))
        failed.operation_lock.release()

    def test_real_runner_bounds_output_and_detects_frozen_inspector_mutation(self):
        for script in ("import sys; sys.stdout.write('x'*262145)",
                       "open(__file__,'a').write('# changed')\nprint('{}')"):
            c = self.process_controller(script)
            with self.assertRaisesRegex(backend.SetupError, "package_inspection_unavailable"):
                c.inspect_package({"package_json": "{}"})
            self.assertEqual(list((c.workspace / "tmp").iterdir()), [])


if __name__ == "__main__":
    unittest.main()
