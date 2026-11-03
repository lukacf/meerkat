"""Retention boundary tests; the independent consumer exercises real SQLite."""
import asyncio
import copy
import json
import os
from pathlib import Path
import shutil
import sys
import threading
import unittest

from mcp import Client
from test_selected_mcpb import McpbFixture, McpbRunner, adapter, backend
from test_selected_setup import software_form


DECISION = {"owner": "investigation-owner", "record": "chosen-tool", "revision": "1"}
VERIFICATION = [{"owner": "observation-owner", "record": "partial-check", "revision": "v1"}]
CANARY = "PRIVATE_CHILD_DETAIL"


class RetentionRunner(McpbRunner):
    def __init__(self, workspace):
        super().__init__(workspace)
        self.saved = {}
        self.save_calls = []
        self.save_entered = threading.Event()
        self.save_release = None
        self.save_override = None

    def run(self, argv, capture=False, inspection_timeout=None):
        if len(argv) < 2 or argv[1] != "save":
            return super().run(argv, capture=capture, inspection_timeout=inspection_timeout)
        self.save_calls.append(list(argv))
        self.save_entered.set()
        if self.save_release is not None and not self.save_release.wait(10):
            raise AssertionError("test did not release save")
        if self.save_override is not None:
            return self.save_override(argv) if callable(self.save_override) else self.save_override
        def arg(name):
            return argv[argv.index(name) + 1]
        package = backend.decode(Path(arg("--package")).read_bytes())
        plan = package["fixture_plan"]
        references = backend.decode(Path(arg("--verification")).read_bytes())
        software = "--declaration" in argv
        row = {"kind": "local_software_selection" if software else "local_selection", "identity": plan["package"],
               "package_content_digest": plan["package_content_digest"],
               **({"document_kind": "local_software_declaration", "declaration_digest": plan["declaration_digest"]}
                  if software else {"manifest_digest": plan["manifest_digest"]}), "profile": arg("--profile"),
               "component": plan["component"], "display_name": package["display_name"],
               "origin": {"kind": "local_investigation", "decision": {
                   key: arg("--decision-" + key) for key in ("owner", "record", "revision")}},
               "verification": references, "reuse": "requires_current_host_selection_and_artifact_validation"}
        if arg("--profile") == "mcpb":
            row["configuration_digest"] = backend.digest(Path(arg("--configuration")).read_bytes())
        if "--plugin-source" in argv:
            row["plugin_source_digest"] = backend.digest(Path(arg("--plugin-source")).read_bytes())
        row["digest"] = backend.digest(backend.encoded(row))
        self.saved[json.dumps(row["identity"], sort_keys=True)] = copy.deepcopy(row)
        return 0, backend.encoded({"schema_version": 1, "scope": backend.LOCAL_RECIPES_OUTPUT_SCOPE,
                                   "status": "retained", "recipe": row})


class RetentionFixture(McpbFixture):
    def setUp(self):
        super().setUp()
        self.profile["validators"].update({key: self.tools[key] for key in ("python", "node")})
        self.profile["installer"] = self.tools["installer"]
        self.write_profile()
        self.database = self.root / "recipes.sqlite3"
        self.database.write_bytes(b"fixture store; no SQLite claim")
        tool = self.root / "recipe-owner"
        tool.write_bytes(b"inert canonical-owner stand-in")
        tool.chmod(0o700)
        self.recipe_profile = {"schema_version": 1, "scope": backend.LOCAL_RECIPES_RETAIN_SCOPE,
            "database": str(self.database), "realm": "test-realm", "owner": "test-principal",
            "tool": {"path": str(tool), "sha256": backend.digest(tool.read_bytes())}}
        self.recipe_path = self.root / "recipes-profile.json"

    def controller(self, **changes):
        self.recipe_path.write_bytes(backend.encoded(self.recipe_profile))
        changes.setdefault("local_recipes_profile", self.recipe_path)
        changes.setdefault("local_recipes_profile_sha256", backend.digest(self.recipe_path.read_bytes()))
        changes.setdefault("runner_factory", RetentionRunner)
        changes.setdefault("npm_cli", sys.executable)
        return super().controller(**changes)

    def stage(self, profile="mcpb", name="choice", mutate=None):
        request = super().stage(profile, name, mutate)
        path = self.staging / name / "package.json"
        package = backend.decode(path.read_bytes())
        package["display_name"] = "Fixture selected package"
        path.write_bytes(backend.encoded(package))
        request["package_sha256"] = backend.digest(path.read_bytes())
        return request

    def installed(self, profile="python", name="choice", unconfirmed=False):
        c = self.controller()
        c.runner.nonzero = unconfirmed
        checked = c.selected_check(self.stage(profile, name))
        job = c.selected_install(self.selection(checked))["job"]
        c.join()
        return c, checked, job

    def request(self, job):
        return {"job_id": job["id"], "decision": copy.deepcopy(DECISION),
                "verification": copy.deepcopy(VERIFICATION)}

    def assert_error(self, call, code):
        with self.assertRaises(backend.SetupError) as caught:
            call()
        self.assertEqual(caught.exception.code, code)
        self.assertNotIn(CANARY, str(caught.exception))

    def tearDown(self):
        for c in self.controllers:
            if c.runner.save_release is not None:
                c.runner.save_release.set()
        super().tearDown()


class RetentionTests(RetentionFixture, unittest.TestCase):
    def test_software_retention_binds_fixed_document_and_exclusive_summary(self):
        for profile in ("python", "node"):
            with self.subTest(profile=profile):
                c = self.controller()
                request = software_form(self.staging, self.stage(profile, "retain-" + profile))
                checked = c.selected_check(request)
                job = c.selected_install(self.selection(checked))["job"]
                c.join()
                self.assertEqual(c.jobs[job["id"]]["status"], "succeeded", c.jobs[job["id"]])
                shutil.rmtree(self.staging / request["staged_id"])
                value = c.remember(self.request(job))
                row = value["recipe"]
                self.assertEqual(row["kind"], "local_software_selection")
                self.assertEqual(row["document_kind"], "local_software_declaration")
                self.assertEqual(row["declaration_digest"], checked["declaration_digest"])
                self.assertNotIn("manifest_digest", row)
                argv = c.runner.save_calls[-1]
                self.assertNotIn("--selection", argv)
                self.assertEqual(Path(argv[argv.index("--declaration") + 1]),
                    c.workspace / "selections" / checked["validation_id"] / "declaration.json")
                self.assertEqual(c.remember(self.request(job)), value)
                for change in ({"manifest_digest": row["declaration_digest"]}, {"kind": "local_selection"},
                               {"declaration_digest": "f" * 64}, {"document_kind": "remote_declaration"}):
                    changed = copy.deepcopy(value)
                    changed["recipe"].update(change)
                    c.runner.save_override = (0, backend.encoded(changed))
                    self.assert_error(lambda: c.remember(self.request(job)), "retention_unconfirmed")

    def test_all_profiles_retain_frozen_documents_replay_and_unknown_install(self):
        for profile in ("python", "node", "mcpb"):
            with self.subTest(profile=profile):
                c, checked, job = self.installed(profile, profile, unconfirmed=profile != "mcpb")
                before = copy.deepcopy(c.job)
                shutil.rmtree(self.staging / profile)
                value = c.remember(self.request(job))
                self.assertEqual(value["status"], "retained")
                self.assertEqual(value["recipe"]["profile"], profile)
                self.assertEqual(c.remember(self.request(job)), value)
                self.assertEqual(c.job, before)
                self.assertEqual(len(c.runner.save_calls), 2)
                self.assertEqual(c.runner.save_calls[0], c.runner.save_calls[1])
                argv = c.runner.save_calls[0]
                self.assertEqual(argv[argv.index("--package") + 1], str(c.workspace / "selections" / checked["validation_id"] / "package.json"))
                self.assertEqual("--configuration" in argv, profile == "mcpb")
                self.assertNotIn("--plugin-source", argv)
                files = list((c.workspace / "recipe-retention").iterdir())
                self.assertEqual(len(files), 1)
                self.assertEqual(files[0].name, backend.digest(files[0].read_bytes()) + ".json")
                self.assertEqual(backend.decode(files[0].read_bytes()), VERIFICATION)

    def test_disclosure_only_and_invalid_profile_do_not_enable_retention(self):
        self.recipe_profile["scope"] = backend.LOCAL_RECIPES_SCOPE
        c, _, job = self.installed()
        self.assertNotIn("retention_available", c.state()["local_recipes"])
        self.assertFalse((c.workspace / "recipe-retention").exists())
        self.assert_error(lambda: c.remember(self.request(job)), "recipe_retention_unavailable")
        self.assertEqual(c.runner.save_calls, [])
        self.recipe_profile["scope"] = "retain_everything"
        self.assert_error(self.controller, "invalid_local_recipes_profile")

    def test_only_settled_local_selected_jobs_are_eligible(self):
        c, _, job = self.installed()
        self.assert_error(lambda: c.remember(dict(self.request(job), job_id="0" * 32)), "retention_job_unavailable")
        c.jobs[job["id"]]["status"] = "running"
        self.assert_error(lambda: c.remember(self.request(job)), "operation_in_progress")
        c.jobs[job["id"]]["status"] = "succeeded"
        c.jobs[job["id"]]["family"] = "services"
        self.assert_error(lambda: c.remember(self.request(job)), "retention_job_unavailable")
        c.jobs[job["id"]]["family"] = "selected-python"
        c.jobs[job["id"]]["recovered"] = True
        self.assert_error(lambda: c.remember(self.request(job)), "retention_job_unavailable")
        self.assertEqual(c.runner.save_calls, [])

    def test_invalid_references_refused_before_child_entry(self):
        c, _, job = self.installed()
        original = self.request(job)
        cases = [dict(original, arbitrary="path"), dict(original, verification=[]),
                 dict(original, verification=VERIFICATION * 2), dict(original, verification=VERIFICATION * 33),
                 dict(original, decision=dict(DECISION, owner="../secret")),
                 dict(original, decision=dict(DECISION, revision="\ud800")),
                 dict(original, decision=dict(DECISION, revision=False)),
                 dict(original, decision=dict(DECISION, revision="\0")),
                 dict(original, decision=dict(DECISION, revision="å" * 129))]
        for value in cases:
            self.assert_error(lambda: c.remember(value), "invalid_request")
        self.assertEqual(c.runner.save_calls, [])

    def test_snapshot_tool_and_immutable_reference_file_tampering_refused(self):
        for kind in ("request", "proposal", "artifact", "tool", "profile", "references", "parent-link"):
            with self.subTest(kind=kind):
                c, checked, job = self.installed(name=kind)
                source = c.workspace / "selections" / checked["validation_id"]
                ref = c.workspace / "recipe-retention" / (backend.digest(backend.encoded(VERIFICATION)) + ".json")
                targets = {"request": c.workspace / "requests" / (job["id"] + ".json"),
                    "proposal": source / "effect.json", "artifact": source / "artifact.whl",
                    "tool": c.workspace / "local-recipes-tool", "profile": c.workspace / "local-recipes-profile.json", "references": ref}
                if kind == "parent-link":
                    directory = c.workspace / "recipe-retention"
                    directory.rmdir(); directory.symlink_to(self.root, target_is_directory=True)
                else:
                    targets[kind].write_bytes(CANARY.encode())
                self.assert_error(lambda: c.remember(self.request(job)), "retention_source_unavailable")
                self.assertEqual(c.runner.save_calls, [])

    def test_output_binding_and_uncertainty_do_not_echo_child_values(self):
        c, _, job = self.installed()
        request = self.request(job)
        good = c.remember(request)
        cases = [(1, b""), (0, b"[]"), (0, b"x" * (backend.MAX_JSON + 1)),
                 (0, backend.encoded(good) + b"{}"), (1, backend.encoded(good))]
        for key in ("identity", "profile", "component", "origin", "verification", "manifest_digest", "package_content_digest", "display_name", "reuse", "digest"):
            changed = copy.deepcopy(good); changed["recipe"][key] = CANARY
            cases.append((0, backend.encoded(changed)))
        for value in cases:
            c.runner.save_override = value
            self.assert_error(lambda: c.remember(request), "retention_unconfirmed")
        c.runner.save_override = (1, backend.encoded({"schema_version": 1, "scope": backend.LOCAL_RECIPES_OUTPUT_SCOPE,
            "status": "error", "error": "immutable_conflict"}))
        self.assert_error(lambda: c.remember(request), "retention_conflict")

    def test_captured_process_timeout_preserves_entered_mutation_uncertainty(self):
        c, _, job = self.installed()
        marker = self.root / "entered-save"
        def timed_out(_):
            script = "import os,time; from pathlib import Path; import sys; Path(sys.argv[1]).write_text(str(os.getpid())); print('entered',flush=True); time.sleep(10)"
            return backend.Runner(c.workspace).run([sys.executable, "-c", script, str(marker)], capture=True, inspection_timeout=.5)
        c.runner.save_override = timed_out
        self.assert_error(lambda: c.remember(self.request(job)), "retention_unconfirmed")
        self.assertTrue(marker.exists())
        with self.assertRaises(ProcessLookupError):
            os.kill(int(marker.read_text()), 0)
        self.assertEqual(len(c.runner.save_calls), 1)


class RetentionAsyncTests(RetentionFixture, unittest.IsolatedAsyncioTestCase):
    async def test_actual_mcp_visibility_binding_and_disclosure_compatibility(self):
        c, _, job = self.installed()
        bridge = adapter.SetupAdapter(c)
        async with Client(adapter.create_server(bridge)) as client:
            listed = await client.list_tools()
            self.assertIn("setup_remember", {tool.name for tool in listed.tools})
            result = await client.call_tool("setup_remember", self.request(job))
            self.assertFalse(result.is_error)
            self.assertEqual(json.loads(result.content[0].text), result.structured_content)
            self.assertEqual(result.structured_content["recipe"]["origin"]["decision"], DECISION)
            bad = await client.call_tool("setup_remember", dict(self.request(job), database="other"))
            self.assertTrue(bad.is_error)
        await bridge.aclose()
        self.recipe_profile["scope"] = backend.LOCAL_RECIPES_SCOPE
        readonly = self.controller(); old_bridge = adapter.SetupAdapter(readonly)
        async with Client(adapter.create_server(old_bridge)) as client:
            self.assertNotIn("setup_remember", {tool.name for tool in (await client.list_tools()).tools})
            self.assertTrue((await client.call_tool("setup_remember", self.request(job))).is_error)
        await old_bridge.aclose()

    async def test_cancel_and_close_join_save_without_replay_or_shared_lock_loss(self):
        c, _, job = self.installed()
        c.runner.save_release = threading.Event()
        bridge = adapter.SetupAdapter(c)
        active = asyncio.create_task(bridge.dispatch("/api/remember", self.request(job)))
        self.assertTrue(await asyncio.to_thread(c.runner.save_entered.wait, 5))
        self.assert_error(lambda: c.remember(self.request(job)), "operation_in_progress")
        active.cancel()
        closing = asyncio.create_task(bridge.aclose())
        await asyncio.sleep(.05)
        self.assertFalse(active.done()); self.assertFalse(closing.done())
        self.assertTrue(c.operation_lock.locked())
        c.runner.save_release.set()
        with self.assertRaises(asyncio.CancelledError):
            await active
        await closing
        self.assertEqual(len(c.runner.save_calls), 1)
        self.assertEqual(len(c.runner.saved), 1)
        self.assertFalse(c.operation_lock.locked())
        self.assert_error(lambda: c.remember(self.request(job)), "host_closing")

    async def test_recovery_excludes_retention_even_if_new_scope_supplied(self):
        c, _, job = self.installed()
        self.assert_error(lambda: self.controller(recover_workspace=c.workspace,
            recover_request=job["id"], recover_request_sha256=job["recovery_request"]["sha256"]), "invalid_local_recipes_profile")
        recovered = self.controller(local_recipes_profile=None, local_recipes_profile_sha256=None,
            recover_workspace=c.workspace, recover_request=job["id"], recover_request_sha256=job["recovery_request"]["sha256"])
        bridge = adapter.SetupAdapter(recovered)
        async with Client(adapter.create_server(bridge)) as client:
            self.assertEqual({tool.name for tool in (await client.list_tools()).tools}, {"setup_recovery_manifest", "setup_state", "setup_reconcile"})
        await bridge.aclose()


if __name__ == "__main__":
    unittest.main()
