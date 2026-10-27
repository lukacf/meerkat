"""Existing selected job/effect ownership with controlled installer responses.

No compiler, npm, package or native runtime is executed by these controls.
"""
import asyncio
import copy
import importlib.util
import json
from pathlib import Path
import threading
import unittest
from unittest.mock import patch

from test_selected_setup import SelectedFixture, SelectedRunner, adapter, backend

# Use the actual second owner's file formatter. Its receipt encoding deliberately
# differs from both stdout's JSON and backend.encoded's semantic projection.
SPEC = importlib.util.spec_from_file_location("build_owner_installer", Path(__file__).resolve().parents[3] / "scripts/install-library-consumer.py")
INSTALLER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(INSTALLER)


class BuildRunner(SelectedRunner):
    def __init__(self, workspace):
        super().__init__(workspace)
        self._job_context = threading.local()
        self.reply = None
        self.change_before_readback = False
        self.observed_timeout = None

    def run(self, argv, capture=False):
        def arg(key):
            return argv[argv.index(key) + 1]
        if "--describe-node-build" in argv:
            self.calls.append((list(argv), capture))
            return 0, backend.encoded({"schema_version": 1, "scope": "selected_node_build_planning",
                "profile_sha256": arg("--node-build-profile-sha256"), "executables": {"compile": {"sha256": "a" * 64}},
                "files": {}, "environment_keys": ["LANG"], "target": {}, "limits": {"timeout_seconds": 900}})
        if "--review-node-build" in argv:
            self.calls.append((list(argv), capture))
            root = Path(arg("--review-node-build"))
            raw = (root / "closure/node-build-plan").read_bytes()
            return 0, backend.encoded({"schema_version": 1, "scope": "selected_node_build",
                "plan_sha256": backend.digest(raw), "profile_sha256": arg("--node-build-profile-sha256"),
                "limits": {"timeout_seconds": 900}, "commands": [{"executable": "compile", "argv": ["controlled-compiler", "literal", "out.js"]}],
                "outputs": [{"path": "out.js", "destination": "node_modules/fixture/generated.js", "replaces_sha256": None}]})
        code, raw = super().run(argv, capture)
        if "--selected-node" not in argv or "--verify-existing" in argv:
            return code, raw
        self.observed_timeout = getattr(self._job_context, "installer_timeout", None)
        self.assert_capture = capture
        source = Path(arg("--selected-node"))
        effect = backend.decode((source / "effect.json").read_bytes())
        if "build_review" not in effect:
            return code, raw
        destination = Path(arg("--destination"))
        path = destination / "ready/installed.json"
        receipt = backend.decode(path.read_bytes())
        receipt["node_build"] = {"review": effect["build_review"], "steps": [{"index": 0, "exit_code": 0}], "outputs": {}}
        path.write_bytes(INSTALLER.encoded(receipt))
        # Match the real installer main serialization, deliberately not encoded().
        response = json.dumps(receipt, sort_keys=True).encode() + b"\n"
        self.original_reply = response
        self.original_receipt_hash = backend.digest(INSTALLER.encoded(receipt))
        self.original_semantic_hash = backend.digest(backend.encoded(receipt))
        if self.change_before_readback:
            receipt["padding"] = "changed after actual owner reply"
            path.write_bytes(INSTALLER.encoded(receipt))
        return code, response if self.reply is None else self.reply


class SelectedNodeBuildOwnerTests(SelectedFixture, unittest.TestCase):
    def setUp(self):
        super().setUp()
        self.build_profile = self.root / "build-profile.json"
        self.build_profile.write_bytes(b'{"scope":"controlled pinned parser fixture"}')
        self.profile["node_build"] = {"path": str(self.build_profile), "sha256": backend.digest(self.build_profile.read_bytes())}
        self.write_profile()

    def controller(self, **kwargs):
        return super().controller(runner_factory=BuildRunner, **kwargs)

    def stage_build(self):
        raw = b'{"fixture":"same pinned parser owns build semantics"}'
        request = self.stage("node", mutate=lambda plan: plan["closure"].update({"node-build-plan": backend.digest(raw)}))
        request["build_plan_sha256"] = backend.digest(raw)
        (self.staging / request["staged_id"] / "closure/node-build-plan").write_bytes(raw)
        return request

    def started(self, c, checked):
        result = c.selected_install(self.selection(checked))["job"]
        c.join()
        return c.jobs[result["id"]]

    def test_review_summary_replay_original_reply_deadline_and_later_live_readback(self):
        c = self.controller()
        request = self.stage_build()
        checked = c.selected_check(request)
        self.assertEqual(checked["build"]["commands"][0]["argv"], ["controlled-compiler", "literal", "out.js"])
        self.assertEqual(c.selected_check(request), checked)
        self.assertEqual(sum("--review-node-build" in call[0] for call in c.runner.calls), 1)
        job = self.started(c, checked)
        self.assertEqual(job["status"], "succeeded", job)
        self.assertTrue(c.runner.assert_capture)
        self.assertEqual(c.runner.observed_timeout, 900)
        self.assertFalse(hasattr(c._job_context, "installer_timeout"))
        origin = job["result"]["installer_reply"]
        self.assertEqual(origin["raw_sha256"], backend.digest(c.runner.original_reply))
        self.assertEqual(origin["receipt_sha256"], c.runner.original_receipt_hash)
        self.assertEqual(origin["semantic_sha256"], c.runner.original_semantic_hash)
        self.assertNotEqual(origin["semantic_sha256"], origin["receipt_sha256"])
        self.assertNotEqual(origin["raw_sha256"], origin["receipt_sha256"])
        self.assertEqual(job["result"]["build_receipt_binding"], "original_installer_reply")
        self.assertEqual(c.selected_install(self.selection(checked))["job"]["id"], job["id"])
        self.assertEqual(c.reconcile({"job_id": job["id"]})["observation"]["status"], "matched")
        receipt = Path(job["result"]["receipt_path"])
        original_file = receipt.read_bytes()
        value = backend.decode(original_file)
        receipt.write_bytes(backend.encoded(value))
        self.assertNotEqual(receipt.read_bytes(), original_file)
        count = len(c.runner.calls)
        self.assertEqual(c.reconcile({"job_id": job["id"]})["observation"]["status"], "unconfirmed")
        self.assertEqual(len(c.runner.calls), count)  # Later live reads also retain the exact file pin.
        receipt.write_bytes(original_file)
        value["padding"] = "mutable receipt and permissive verifier cannot replace original reply"
        receipt.write_bytes(backend.encoded(value))
        count = len(c.runner.calls)
        self.assertEqual(c.reconcile({"job_id": job["id"]})["observation"]["status"], "unconfirmed")
        self.assertEqual(len(c.runner.calls), count)  # Refuse before a new verifier result can become the origin.

    def test_first_ready_file_must_match_reply_before_readback(self):
        c = self.controller()
        checked = c.selected_check(self.stage_build())
        c.runner.change_before_readback = True
        job = self.started(c, checked)
        self.assertEqual(job["status"], "unconfirmed", job)
        self.assertFalse(any("--verify-existing" in argv for argv, _ in c.runner.calls))
        self.assertTrue(Path(checked["destination"], "ready/installed.json").exists())
        self.assertIn("installer_reply", c.install_intents[job["id"]])
        self.assertNotIn("receipt_sha256", c.install_intents[job["id"]]["installer_reply"])

    def test_invalid_or_oversized_reply_retains_partial_state_and_same_job(self):
        for index, reply in enumerate((b"", b"not JSON", b"{}", b'{"node_build":[]}',
                                       b'{"node_build":{},"node_build":{}}',
                                       b"x" * (backend.MAX_JSON + 1), None)):
            c = self.controller()
            # Each controller can share this single source under its own owner.
            request = self.stage_build() if index == 0 else copy.deepcopy(request)
            checked = c.selected_check(request)
            c.runner.reply = reply
            c.runner.nonzero = reply is None  # A committed ready file and nonzero exit has no successful origin.
            job = self.started(c, checked)
            self.assertEqual(job["status"], "unconfirmed", job)
            self.assertTrue(Path(checked["destination"], "ready/installed.json").exists())
            self.assertFalse(any("--verify-existing" in argv for argv, _ in c.runner.calls))
            self.assertEqual(c.selected_install(self.selection(checked))["job"]["id"], job["id"])
            intent = c.install_intents[job["id"]]
            self.assertTrue(intent["installer_reply_required"])
            self.assertNotIn("installer_reply", intent)
            count = len(c.runner.calls)
            self.assertEqual(c.reconcile({"job_id": job["id"]})["observation"]["status"], "unconfirmed")
            self.assertEqual(len(c.runner.calls), count)

    def test_successful_live_reconcile_retains_pin_after_first_verifier_failure(self):
        c = self.controller()
        checked = c.selected_check(self.stage_build())
        c.runner.bad_proof = {"verified": False}
        job = self.started(c, checked)
        self.assertEqual(job["status"], "unconfirmed", job)
        origin = c.install_intents[job["id"]]["installer_reply"]
        self.assertNotIn("receipt_sha256", origin)
        c.runner.bad_proof = None
        observed = c.reconcile({"job_id": job["id"]})["observation"]
        self.assertEqual(observed["status"], "matched", observed)
        self.assertEqual(origin["receipt_sha256"], observed["result"]["receipt_sha256"])
        self.assertEqual(c.jobs[job["id"]]["status"], "unconfirmed")
        path = Path(observed["result"]["receipt_path"])
        original_file = path.read_bytes()
        value = backend.decode(original_file)
        for changed in (backend.encoded(value), INSTALLER.encoded({**value, "padding": "changed content"})):
            self.assertNotEqual(changed, original_file)
            path.write_bytes(changed)
            count = len(c.runner.calls)
            self.assertEqual(c.reconcile({"job_id": job["id"]})["observation"]["status"], "unconfirmed")
            self.assertEqual(len(c.runner.calls), count)
        path.write_bytes(original_file)
        self.assertEqual(c.reconcile({"job_id": job["id"]})["observation"]["status"], "matched")
        self.assertEqual(c.jobs[job["id"]]["status"], "unconfirmed")

    def test_fresh_retry_retains_review_and_does_not_rebuild(self):
        c = self.controller()
        checked = c.selected_check(self.stage_build())
        c.runner.reply = b"invalid original reply"
        job = self.started(c, checked)
        before = list(c.runner.calls)
        retry = c.selected_retry({"job_id": job["id"]})
        self.assertEqual(retry["build"], checked["build"])
        self.assertNotEqual(retry["destination"], checked["destination"])
        self.assertEqual(c.runner.calls, before)
        self.assertEqual(c.selected_retry({"job_id": job["id"]}), retry)

    def test_cold_recovery_claims_current_consistency_without_original_reply(self):
        c = self.controller()
        checked = c.selected_check(self.stage_build())
        c.runner.reply = b"lost original successful reply"
        job = self.started(c, checked)
        self.assertEqual(job["status"], "unconfirmed")
        recovered = self.recover(c, job)
        result = recovered.reconcile({"job_id": job["id"]})["observation"]
        self.assertEqual(result["status"], "matched", result)
        self.assertEqual(result["result"]["build_receipt_binding"], "current_receipt_consistency_only")
        self.assertNotIn("installer_reply", result["result"])
        self.assertTrue(all("--verify-existing" in argv for argv, _ in recovered.runner.calls))

    def test_optional_plan_requires_node_explicit_digest_and_host_support(self):
        request = self.stage_build()
        self.profile.pop("node_build")
        self.write_profile()
        c = self.controller()
        with self.assertRaises(backend.SetupError):
            c.selected_check(request)
        self.assertEqual(c.runner.calls, [])
        request.pop("build_plan_sha256")
        with self.assertRaises(backend.SetupError):
            c.selected_check(request)
        with self.assertRaises(backend.SetupError):
            backend.selected_source_request({**request, "profile": "python", "build_plan_sha256": "a" * 64})

    def test_acquired_plan_is_separate_and_preserves_actual_receipt_and_source(self):
        original = {"primary": {"url": "https://example.invalid/package.tgz", "sha256": "a" * 64},
                    "files": {"artifact.tgz": "a" * 64, "closure/package-json": "b" * 64, "closure/package-lock": "c" * 64},
                    "receipt_sha256": "d" * 64}
        before = copy.deepcopy(original)
        document = backend.encoded({"components": [{"id": "server", "source": {"kind": "artifact", "entrypoint": {"executable": "index.js", "arguments": []}}}]})
        expanded = backend.decode(backend.materialize_acquired_source(document, "server", original, "e" * 64))
        closure = expanded["components"][0]["source"]["dependencies"]["artifacts"]
        self.assertEqual(closure, {"package-json": "b" * 64, "package-lock": "c" * 64, "node-build-plan": "e" * 64})
        self.assertEqual(original, before)
        request = {"profile": "node", "staged_id": "source", "component": "server", "package_json": "{}",
                   "declaration_json": document.decode(), "build_plan_json": '{"literal":"agent plan"}'}
        normalized, documents = backend.selected_check_request(request)
        self.assertEqual(normalized["build_plan_sha256"], backend.digest(documents["closure/node-build-plan"]))
        self.assertEqual(documents["closure/node-build-plan"], request["build_plan_json"].encode())

    def test_final_mcp_schema_preserves_build_and_acquisition_conditions(self):
        from mcp import Client
        c = self.controller()
        # A controlled canonical-owner fixture enables the MCPB schema branch;
        # this control exercises exported tools/dispatch validation, not its installer.
        c.selected["validators"]["mcpb"] = copy.deepcopy(c.selected["validators"]["node"])
        owner = adapter.SetupAdapter(c)
        inline = {"profile": "node", "staged_id": "choice", "component": "server",
                  "package_json": "{}", "declaration_json": "{}", "build_plan_json": "{}"}
        staged = {"profile": "node", "staged_id": "choice", "component": "server",
                  "package_sha256": "a" * 64, "selection_sha256": "b" * 64, "build_plan_sha256": "c" * 64}
        acquired = {**inline, "acquired_source": {"job_id": "1" * 32, "receipt_sha256": "d" * 64}}
        invalid = [dict(inline, profile="python"), dict(staged, profile="python"),
                   dict(staged, profile="mcpb", configuration_sha256="d" * 64),
                   dict(inline, profile="mcpb", configuration_json="{}"),
                   dict(inline, build_plan_sha256="c" * 64), dict(staged, build_plan_json="{}"),
                   dict(inline, acquired_source=acquired["acquired_source"], profile="mcpb"),
                   dict(staged, acquired_source=acquired["acquired_source"])]
        async def exercise():
            try:
                with patch.object(c, "dispatch", return_value={"validated": True}) as dispatch:
                    async with Client(adapter.create_server(owner)) as client:
                        tools = {tool.name: tool for tool in (await client.list_tools()).tools}
                        properties = tools["setup_selected_check"].input_schema["properties"]
                        self.assertIn("build_plan_json", properties)
                        self.assertIn("build_plan_sha256", properties)
                        # As on the existing surface, the model envelope omits
                        # allOf; the final server execution validator retains it.
                        for value in (inline, staged, acquired):
                            result = await client.call_tool("setup_selected_check", value)
                            self.assertFalse(result.is_error, result)
                        before = dispatch.call_count
                        self.assertEqual(before, 3)
                        for value in invalid:
                            result = await client.call_tool("setup_selected_check", value)
                            self.assertTrue(result.is_error, (value, result))
                            self.assertEqual(result.structured_content, {"error": "invalid_arguments"})
                        self.assertEqual(dispatch.call_count, before)
            finally:
                await owner.aclose()
        asyncio.run(exercise())

    def test_planning_uses_pinned_projection_and_restores_existing_timeout_context(self):
        c = self.controller()
        self.assertTrue(c.state()["node_build"]["available"])
        self.assertNotIn(str(self.build_profile), backend.encoded(c.state()["node_build"]).decode())
        c._job_context.installer_timeout = 77
        seen = []
        with patch.object(c, "_run", side_effect=lambda *a, **kw: seen.append(c._job_context.installer_timeout) or (0, b"ok")):
            c._run_selected_installer([], "node", installer_timeout=900, capture=True)
        self.assertEqual(seen, [900])
        self.assertEqual(c._job_context.installer_timeout, 77)


if __name__ == "__main__":
    unittest.main()
