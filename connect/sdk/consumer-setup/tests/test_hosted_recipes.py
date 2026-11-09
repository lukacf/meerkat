"""Hosted recipe bridge checks with real MCP and child custody, fixture store owner.

The independent canonical-CLI trial validates Rust document/store semantics.
"""
import asyncio
import copy
import json
import os
import shutil
from pathlib import Path
import sys
import threading
import time
import unittest
from unittest.mock import patch

from mcp import Client
import test_local_recipes as fixtures

adapter, backend = fixtures.adapter, fixtures.backend
DECISION = {"owner": "decisions", "record": "hosted-choice", "revision": "1"}
# This fixture transports an owner result. Rust store tests establish the real
# reference derivation and durable replay; this is not a second implementation.
SUBMISSION_DECISION = {"owner": "toolkit.local-recipes.current-submission",
                       "record": "submission/" + "c" * 64, "revision": "d" * 64}
REQUEST = {"plugin_json": '{ "name": "fixture" }\r\n',
           "mcp_json": '{"$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json","mcpServers":{"fixture":{"type":"streamable-http","url":"https://fixture.invalid/mcp"}}}',
           "recipes_json": '{"guidance":"' + "x" * 2200 + '"}', "decision": DECISION}
EXPORT = {**fixtures.REQUEST, "activation_json": '{ "fixture": "activation" }\n'}


class HostedRunner(backend.Runner):
    def __init__(self, workspace):
        super().__init__(workspace)
        self.calls, self.records = [], {}
        self.mode = None
        self.block = None
        self.entered = threading.Event()

    def run(self, argv, *, capture=False):
        self.calls.append((list(argv), capture))
        action = argv[1]
        def arg(name):
            return argv[argv.index(name) + 1]
        self.entered.set()
        if self.block is not None:
            self.block.wait(5)
        summary = fixtures.record()
        if action == "save":
            documents = {"plugin_json": Path(arg("--plugin")).read_bytes().decode("utf-8"),
                         "recipes_json": Path(arg("--recipes")).read_bytes().decode("utf-8"),
                         "mcp_json": Path(arg("--mcp")).read_bytes().decode("utf-8") if "--mcp" in argv else None}
            summary["origin"]["decision"] = ({key: arg("--decision-" + key) for key in DECISION}
                if "--decision-owner" in argv else copy.deepcopy(SUBMISSION_DECISION))
            if self.mode == "malformed_origin":
                summary["origin"]["decision"] = {"owner": "owner", "record": True, "revision": "1"}
            self.records = {**summary, "documents": documents}
            if self.mode == "conflict":
                return 1, backend.encoded(fixtures.envelope("error", error="immutable_conflict"))
            if self.mode == "lost_save":
                return 0, b""
            return 0, backend.encoded(fixtures.envelope("retained", recipe=summary))
        if action == "read":
            result = copy.deepcopy(self.records)
            if self.mode == "wrong_documents":
                result["documents"]["plugin_json"] = "different"
            if self.mode == "wrong_origin":
                result["origin"]["decision"]["revision"] = "different"
            if self.mode == "lost_read":
                return 0, b""
            return 0, backend.encoded(fixtures.envelope("found", recipe=result))
        if action == "export":
            if self.mode == "sleep":
                return super().run([sys.executable, "-I", "-c",
                    "import os,pathlib,time; pathlib.Path('child.pid').write_text(str(os.getpid())); time.sleep(60)"], capture=True)
            if self.records["documents"]["mcp_json"] is None:
                return 1, backend.encoded(fixtures.envelope("error", error="portable_export_refused"))
            directory = Path(arg("--destination"))
            directory.mkdir()
            files = {}
            for name, field in (("plugin.json", "plugin_json"), ("mcp.json", "mcp_json")):
                data = self.records["documents"][field].encode()
                (directory / name).write_bytes(data)
                files[name] = {"sha256": backend.digest(data), "bytes": len(data)}
            result = fixtures.envelope("exported", recipe={key: value for key, value in self.records.items() if key != "documents"},
                                      selection_digest="b" * 64, files=files, destination=str(directory))
            if "--declaration" in argv:
                result["selection_kind"] = "remote_declaration"
            if self.mode == "mismatch":
                (directory / "mcp.json").write_text("changed")
            if self.mode == "symlink":
                (directory / "mcp.json").unlink()
                (directory / "mcp.json").symlink_to(directory / "plugin.json")
            if self.mode == "extra_file":
                (directory / "extra.json").write_text("{}")
            if self.mode == "wrong_identity":
                result["recipe"]["identity"]["name"] = "different"
            if self.mode == "mutate_input":
                Path(arg("--declaration" if "--declaration" in argv else "--selection")).write_text("{}")
            if self.mode == "lost_export":
                return 0, b""
            return 0, backend.encoded(result)
        raise AssertionError(action)


class HostedFixture(fixtures.RecipeFixture):
    def setUp(self):
        super().setUp()
        self.profile["scope"] = backend.LOCAL_RECIPES_RETAIN_SCOPE
        self.write_profile()

    def controller(self, **changes):
        return super().controller(**{"allow_remote_recipes": True, "runner_factory": HostedRunner, **changes})

    def tearDown(self):
        for controller in self.controllers:
            for job in list(controller.jobs.values()):
                if job["status"] == "running":
                    controller.stop({"job_id": job["id"]})
        super().tearDown()

    def retained(self):
        controller = self.controller()
        result = controller.remember_remote(REQUEST)
        self.assertEqual(result["status"], "retained")
        return controller


class HostedRecipeTests(HostedFixture, unittest.TestCase):
    def test_opt_in_requires_retention_profile_before_workspace_creation(self):
        self.profile["scope"] = backend.LOCAL_RECIPES_SCOPE
        self.write_profile()
        self.assert_refused(lambda: self.controller(), "invalid_remote_recipes_profile")
        self.assertFalse((self.root / "work-1").exists())
        controller = self.controller(allow_remote_recipes=False)
        self.assert_refused(lambda: controller.remember_remote(REQUEST), "remote_recipes_unavailable")
        self.assert_refused(lambda: controller.export_recipe(EXPORT), "remote_recipes_unavailable")
        self.assertEqual(controller.runner.calls, [])
        self.assertFalse(controller.state()["local_recipes"].get("export_available", False))

    def test_exact_documents_decision_and_fixed_scope_are_frozen_and_read_back(self):
        controller = self.retained()
        self.assertEqual([argv[1] for argv, _ in controller.runner.calls], ["save", "read"])
        for argv, capture in controller.runner.calls:
            self.assertTrue(capture)
            for key in ("database", "realm", "owner"):
                self.assertEqual(argv[argv.index("--" + key) + 1], self.profile[key])
        argv = controller.runner.calls[0][0]
        self.assertEqual(Path(argv[argv.index("--recipes") + 1]).read_bytes(), REQUEST["recipes_json"].encode())
        self.assertEqual(controller.runner.records["documents"], {key: REQUEST[key] for key in ("plugin_json", "mcp_json", "recipes_json")})
        result = controller.remember_remote(REQUEST)
        self.assertEqual(result["recipe"]["origin"]["decision"], DECISION)
        self.assertEqual([argv[1] for argv, _ in controller.runner.calls], ["save", "read", "save", "read"])

    def test_current_submission_is_requested_from_owner_and_preserved_across_fresh_workspaces(self):
        request = {key: value for key, value in REQUEST.items() if key != "decision"}
        first = self.controller()
        saved = first.remember_remote(request)
        self.assertEqual(saved["recipe"]["origin"]["decision"], SUBMISSION_DECISION)
        argv = first.runner.calls[0][0]
        self.assertFalse(any(arg.startswith("--decision-") for arg in argv))
        retained = json.loads((Path(argv[argv.index("--plugin") + 1]).parent / "request.json").read_bytes())
        self.assertEqual(retained, request)
        self.assertNotIn("decision", request)
        self.assertEqual(first.remember_remote(request), saved)
        first.close_admission()
        first.join()
        shutil.rmtree(first.workspace)
        second = self.controller()
        self.assertEqual(second.remember_remote(request), saved)
        self.assertEqual([argv[1] for argv, _ in second.runner.calls], ["save", "read"])
        self.assertNotEqual(first.workspace, second.workspace)
        self.assertEqual(second.runner.records["documents"], request)

    def test_current_submission_uncertainty_uses_one_save_and_never_invents_a_reference(self):
        request = {key: value for key, value in REQUEST.items() if key != "decision"}
        for mode in ("lost_save", "lost_read", "wrong_documents", "wrong_origin", "malformed_origin"):
            with self.subTest(mode=mode):
                controller = self.controller()
                controller.runner.mode = mode
                self.assert_refused(lambda: controller.remember_remote(request), "retention_unconfirmed")
                self.assertEqual(sum(argv[1] == "save" for argv, _ in controller.runner.calls), 1)
                self.assertFalse(any(arg.startswith("--decision-") for arg in controller.runner.calls[0][0]))
                self.assertNotIn("decision", request)
                if mode in ("lost_save", "malformed_origin"):
                    self.assertEqual(len(controller.runner.calls), 1)

    def test_documents_and_aggregate_are_bounded_before_writes(self):
        controller = self.controller()
        for request in ({**REQUEST, "plugin_json": "x" * (backend.REMOTE_RECIPE_DOCUMENT + 1)},
                        {**REQUEST, "plugin_json": "x" * 30000, "recipes_json": "x" * 30000, "mcp_json": "x" * 10000},
                        {**REQUEST, "plugin_json": "\u00e5" * 20000},
                        {**REQUEST, "decision": {**DECISION, "owner": "x\0y"}},
                        {**REQUEST, "decision": None},
                        {**REQUEST, "decision": {"owner": "partial"}},
                        {**REQUEST, "owner": "other"}):
            self.assert_refused(lambda: controller.remember_remote(request), "invalid_request")
        self.assertEqual(controller.runner.calls, [])
        self.assertEqual(list((controller.workspace / "remote-recipes").iterdir()), [])

    def test_frozen_owner_and_profile_tamper_refused_before_cli(self):
        for name in ("local-recipes-tool", "local-recipes-profile.json"):
            with self.subTest(name=name):
                controller = self.controller()
                (controller.workspace / name).write_text("tampered")
                self.assert_refused(lambda: controller.remember_remote(REQUEST), "retention_source_unavailable")
                self.assertEqual(controller.runner.calls, [])
                self.assertEqual(list((controller.workspace / "remote-recipes").iterdir()), [])

    def test_save_and_readback_uncertainty_never_claim_retained(self):
        for mode in ("lost_save", "wrong_documents", "wrong_origin", "lost_read", "conflict"):
            with self.subTest(mode=mode):
                controller = self.controller()
                controller.runner.mode = mode
                expected = "retention_conflict" if mode == "conflict" else "retention_unconfirmed"
                self.assert_refused(lambda: controller.remember_remote(REQUEST), expected)
                self.assertTrue(controller.runner.records)
                self.assertEqual(sum(argv[1] == "save" for argv, _ in controller.runner.calls), 1)

    def test_export_owned_result_files_and_replay(self):
        controller = self.retained()
        job = controller.export_recipe(EXPORT)["job"]
        controller.join()
        settled = controller.job_detail({"job_id": job["id"]})
        self.assertTrue(settled["reconciliation"]["available"])
        self.assertEqual(settled["job"]["kind"], "export")
        self.assertEqual(settled["job"]["family"], "remote-recipe")
        self.assertEqual(settled["job"]["status"], "succeeded")
        result = settled["job"]["result"]
        self.assertEqual(result["scope"], "portable_mcp_configuration_only")
        self.assertEqual(result["mcp_configuration"], json.loads(REQUEST["mcp_json"]))
        self.assertEqual(result["plugin_json"], REQUEST["plugin_json"])
        for name, path in (("mcp.json", result["mcp_path"]), ("plugin.json", result["plugin_path"])):
            self.assertEqual(backend.digest(Path(path).read_bytes()), result["files"][name]["sha256"])
        calls = copy.deepcopy(controller.runner.calls)
        self.assertEqual(controller.export_recipe(EXPORT)["job"], settled["job"])
        self.assertEqual(controller.runner.calls, calls)
        # This older fixture has no check-export implementation; observation is
        # unconfirmed without changing the successful historical job.
        self.assertEqual(controller.reconcile({"job_id": job["id"]})["observation"]["status"], "unconfirmed")

    def test_lost_or_mismatched_exports_remain_unconfirmed_without_reexport(self):
        for mode in ("lost_export", "mismatch", "symlink", "extra_file", "wrong_identity", "mutate_input"):
            with self.subTest(mode=mode):
                controller = self.retained()
                controller.runner.mode = mode
                job = controller.export_recipe(EXPORT)["job"]
                controller.join()
                settled = controller.jobs[job["id"]]
                self.assertEqual(settled["status"], "unconfirmed")
                self.assertNotIn("result", settled)
                calls = copy.deepcopy(controller.runner.calls)
                self.assertEqual(controller.export_recipe(EXPORT)["job"], settled)
                self.assertEqual(controller.runner.calls, calls)
                self.assertTrue(list((controller.workspace / "remote-recipes").glob("export-*/output")))

    def test_guidance_without_mcp_can_be_retained_but_not_exported(self):
        controller = self.controller()
        request = {key: value for key, value in REQUEST.items() if key != "mcp_json"}
        self.assertEqual(controller.remember_remote(request)["status"], "retained")
        self.assertNotIn("--mcp", controller.runner.calls[0][0])
        job = controller.export_recipe(EXPORT)["job"]
        controller.join()
        self.assertEqual(controller.jobs[job["id"]]["status"], "unconfirmed")
        self.assertEqual(list((controller.workspace / "remote-recipes").glob("export-*/output")), [])

    def test_busy_or_thread_start_failure_never_loses_export_identity(self):
        controller = self.retained()
        controller.operation_lock.acquire()
        try:
            self.assert_refused(lambda: controller.export_recipe(EXPORT), "operation_in_progress")
            self.assertEqual(controller.recipe_exports, {})
        finally:
            controller.operation_lock.release()
        with patch.object(backend.threading.Thread, "start", side_effect=RuntimeError("fixture start failure")):
            job = controller.export_recipe(EXPORT)["job"]
        self.assertEqual(job["status"], "unconfirmed")
        self.assertEqual(controller.export_recipe(EXPORT)["job"], job)
        self.assertFalse(controller.operation_lock.locked())
        self.assertFalse(any(argv[1] == "export" for argv, _ in controller.runner.calls))

    def test_simultaneous_export_requests_launch_one_owned_attempt(self):
        controller = self.retained()
        controller.runner.block = threading.Event()
        barrier, replies = threading.Barrier(3), []
        def export():
            barrier.wait(5)
            replies.append(controller.export_recipe(EXPORT)["job"])
        threads = [threading.Thread(target=export) for _ in range(2)]
        for thread in threads:
            thread.start()
        barrier.wait(5)
        for thread in threads:
            thread.join(5)
            self.assertFalse(thread.is_alive())
        self.assertEqual(len(replies), 2)
        self.assertEqual(replies[0]["id"], replies[1]["id"])
        controller.runner.block.set()
        controller.join()
        self.assertEqual(sum(argv[1] == "export" for argv, _ in controller.runner.calls), 1)

    def test_stop_real_export_child_then_replay_does_not_launch_again(self):
        controller = self.retained()
        controller.runner.mode = "sleep"
        job = controller.export_recipe(EXPORT)["job"]
        marker = controller.workspace / "child.pid"
        deadline = time.monotonic() + 5
        while not marker.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertTrue(marker.exists())
        pid = int(marker.read_text())
        result = controller.stop({"job_id": job["id"]})["job"]
        self.assertEqual(result["stop"]["status"], "requested")
        controller.join()
        settled = controller.jobs[job["id"]]
        self.assertEqual(settled["status"], "unconfirmed")
        self.assertEqual(settled["stop"]["status"], "settled")
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)
        self.assertEqual(controller.export_recipe(EXPORT)["job"], settled)
        self.assertEqual(sum(argv[1] == "export" for argv, _ in controller.runner.calls), 1)


class HostedMcpTests(HostedFixture, unittest.IsolatedAsyncioTestCase):
    async def test_cancelled_export_reply_keeps_running_job_and_exact_replay(self):
        controller = self.retained()
        controller.runner.block = threading.Event()
        owner = adapter.SetupAdapter(controller)
        entered, release = threading.Event(), threading.Event()
        original = controller.dispatch
        def delayed_reply(route, arguments):
            result = original(route, arguments)
            if route == "/api/export_recipe":
                entered.set()
                if not release.wait(5):
                    raise AssertionError("missing reply release")
            return result
        try:
            async with Client(adapter.create_server(owner)) as client:
                with patch.object(controller, "dispatch", side_effect=delayed_reply):
                    pending = asyncio.create_task(client.call_tool("setup_export_recipe", EXPORT))
                    self.assertTrue(await asyncio.to_thread(entered.wait, 5))
                    pending.cancel()
                    release.set()
                    with self.assertRaises(asyncio.CancelledError):
                        await pending
                    async with asyncio.timeout(5):
                        while owner._inflight:
                            await asyncio.sleep(0.01)
                job = controller.state()["job"]
                self.assertEqual(job["status"], "running")
                self.assertNotIn("stop", job)
                replay = await client.call_tool("setup_export_recipe", EXPORT)
                self.assertFalse(replay.is_error)
                self.assertEqual(replay.structured_content["job"]["id"], job["id"])
                controller.runner.block.set()
                await asyncio.to_thread(controller.join)
                self.assertEqual(controller.jobs[job["id"]]["status"], "succeeded")
                self.assertEqual(sum(argv[1] == "export" for argv, _ in controller.runner.calls), 1)
        finally:
            release.set()
            controller.runner.block.set()
            await owner.aclose()

    async def test_real_mcp_scoped_document_limits_and_export_job(self):
        controller = self.controller()
        owner = adapter.SetupAdapter(controller)
        try:
            async with Client(adapter.create_server(owner)) as client:
                names = {tool.name for tool in (await client.list_tools()).tools}
                self.assertTrue({"setup_remember_remote", "setup_export_recipe"} <= names)
                result = await client.call_tool("setup_remember_remote", REQUEST)
                self.assertFalse(result.is_error, result)
                self.assertEqual(result.structured_content["status"], "retained")
                result = await client.call_tool("setup_export_recipe", EXPORT)
                self.assertFalse(result.is_error, result)
                job = result.structured_content["job"]
                await asyncio.to_thread(controller.join)
                result = await client.call_tool("setup_job", {"job_id": job["id"]})
                self.assertEqual(result.structured_content["job"]["status"], "succeeded")
                declaration = {**fixtures.REQUEST, "declaration_json": '{"selection_kind":"remote_declaration"}'}
                result = await client.call_tool("setup_export_recipe", declaration)
                self.assertFalse(result.is_error, result)
                await asyncio.to_thread(controller.join)
                current = await client.call_tool("setup_job", {"job_id": result.structured_content["job"]["id"]})
                self.assertEqual(current.structured_content["job"]["result"]["selection_kind"], "remote_declaration")
                self.assertTrue((await client.call_tool("setup_export_recipe", {**declaration, "activation_json": "{}"})).is_error)
                self.assertTrue((await client.call_tool("setup_export_recipe", {**declaration, "declaration_json": "x" * 33000})).is_error)
                self.assertTrue((await client.call_tool("setup_recipe", {**fixtures.REQUEST, "name": "x" * 2200})).is_error)
                for name, arguments in (("setup_remember_remote", {**REQUEST, "plugin_json": "x" * 33000}),
                                        ("setup_export_recipe", {**EXPORT, "activation_json": "x" * 33000})):
                    self.assertTrue((await client.call_tool(name, arguments)).is_error)
        finally:
            await owner.aclose()

    async def test_mcp_accepts_current_submission_without_a_historical_decision(self):
        controller = self.controller()
        owner = adapter.SetupAdapter(controller)
        try:
            async with Client(adapter.create_server(owner)) as client:
                definitions = {tool.name: tool for tool in (await client.list_tools()).tools}
                self.assertNotIn("decision", definitions["setup_remember_remote"].input_schema["required"])
                request = {key: value for key, value in REQUEST.items() if key != "decision"}
                result = await client.call_tool("setup_remember_remote", request)
                self.assertFalse(result.is_error, result)
                self.assertEqual(result.structured_content["recipe"]["origin"]["decision"], SUBMISSION_DECISION)
                self.assertTrue((await client.call_tool("setup_remember_remote", {**request, "decision": None})).is_error)
        finally:
            await owner.aclose()

    async def test_old_retention_scope_exposes_neither_new_tool_without_opt_in(self):
        controller = self.controller(allow_remote_recipes=False)
        owner = adapter.SetupAdapter(controller)
        try:
            async with Client(adapter.create_server(owner)) as client:
                names = {tool.name for tool in (await client.list_tools()).tools}
                self.assertIn("setup_remember", names)
                self.assertFalse({"setup_remember_remote", "setup_export_recipe"} & names)
        finally:
            await owner.aclose()


if __name__ == "__main__":
    unittest.main()
