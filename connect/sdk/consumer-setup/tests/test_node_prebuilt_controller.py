"""Independent controls for prebuilt acquisition admission and real MCP routing.

Archive/readback owners are real. Runtime/network and canonical validator
outputs are inert fixtures, so these checks do not qualify package execution.
"""
import asyncio
import copy
import json
from pathlib import Path
import subprocess
import sys
import threading
import unittest
from unittest.mock import patch

from mcp import Client

import test_acquisition_recovery as recovery
import test_node_acquisition as node_tests
from test_node_prebuilt_acquisition import asset
from test_selected_setup import SelectedRunner, adapter, backend


class ControllerFixture:
    def build(self, enabled=True):
        case = node_tests.NodeAcquisitionTests()
        case.setUp()
        self.addCleanup(case.doCleanups)
        case.profile_path.write_bytes(backend.encoded(case.profile))
        validator = case.root / "validator"
        validator.write_bytes(b"inert validator owner boundary\n")
        validator.chmod(0o700)
        selected = case.root / "selected.json"
        selected.write_bytes(backend.encoded({"schema_version": 1, "scope": backend.SELECTED_SCOPE,
            "staging_root": str(case.staging),
            "installer": {"path": str(case.installer), "sha256": backend.digest(case.installer.read_bytes())},
            "validators": {"node": {"path": str(validator), "sha256": backend.digest(validator.read_bytes())}}}))
        target = {"node_sha256": backend.digest(case.node.read_bytes()), "node_version": "v24.19.0",
                  "node_abi": "137", "platform": "darwin", "arch": "arm64", "libc": None}
        profile = case.root / "prebuilt.json"
        profile.write_bytes(backend.encoded({"schema_version": 1, "scope": "trusted_public_node_prebuilt_acquisition",
            "python": {"path": str(case.python), "sha256": backend.digest(case.python.read_bytes())},
            "target": target, "allowed_origins": ["https://source.example"], "allowed_redirect_origins": []}))
        arguments = dict(bundle=None, contents_sha256=None, workspace=case.root / "setup", families=(),
            python=sys.executable, node=str(case.node), npm_cli=sys.executable, runner_factory=SelectedRunner,
            selected_install_profile=selected, selected_install_profile_sha256=backend.digest(selected.read_bytes()),
            node_acquisition_profile=case.profile_path,
            node_acquisition_profile_sha256=backend.digest(case.profile_path.read_bytes()))
        if enabled:
            arguments.update(node_prebuilt_profile=profile, node_prebuilt_profile_sha256=backend.digest(profile.read_bytes()))
        controller = backend.Controller(**arguments)
        self.addCleanup(controller.join)
        original = controller.node_acquisition.run
        def acquire(request, directory, _run, check_stop):
            case.job = directory
            return original(request, directory, case.runner, check_stop)
        controller.node_acquisition.run = acquire
        controller.acquire_node(case.request)
        controller.join()
        self.assertEqual(controller.job["status"], "succeeded", controller.job)
        source_job = copy.deepcopy(controller.job)
        binary = b"\x7fELF\x00inert-native-controller-fixture"
        payload = asset({"build/Release/addon.node": binary})
        request = {"request_id": "d" * 32,
            "source": {"job_id": source_job["id"], "receipt_sha256": source_job["result"]["receipt_sha256"]},
            "target": target, "payloads": [{"package_location": "node_modules/dependency",
                "package_archive_sha256": backend.digest(case.dependency),
                "asset_id": "native-asset-" + backend.digest(payload), "url": "https://source.example/release.tar.gz",
                "files": [{"member": "build/Release/addon.node", "destination": "build/Release/addon.node"}]}]}
        run = controller.runner.run
        calls = []
        entered, release = threading.Event(), threading.Event()
        release.set()
        self.addCleanup(release.set)
        def execute(argv, **kwargs):
            if argv[0] == str(case.node):
                calls.append(list(argv))
                entered.set()
                while not release.wait(0.01):
                    controller._check_stop()
                return 0, backend.encoded({k: v for k, v in target.items() if k != "node_sha256"})
            if "--fetch" in argv:
                calls.append(list(argv))
                settings = json.loads(Path(argv[-2]).read_bytes())
                directory = Path(settings["directory"])
                (directory / "artifact.mcpb").write_bytes(payload)
                (directory / "report.json").write_bytes(backend.encoded({"primary": {
                    "url": settings["request"]["url"], "sha256": backend.digest(payload), "byte_length": len(payload),
                    "final_origin": "https://source.example", "redirect_count": 0}}))
                return 0, b"private fixture output"
            return run(argv, capture=kwargs.get("capture", False))
        controller.runner.run = execute
        return case, controller, request, source_job, calls, entered, release

    def derive(self, controller, request):
        admitted = controller.acquire_node_prebuilt(request)
        controller.join()
        job = copy.deepcopy(controller.jobs[admitted["job"]["id"]])
        self.assertEqual(job["status"], "succeeded", job)
        return job

    def selection(self, job):
        result = job["result"]
        source = {"kind": "artifact", "entrypoint": {"executable": "index.js", "arguments": []}}
        declaration = {"document_kind": "local_software_declaration",
                       "components": [{"id": "server", "runtime": "node", "source": source}]}
        plan = {"schema_version": 1, "package": {"publisher": "fixture", "name": "chosen", "version": "1"},
            "package_content_digest": "1" * 64, "document_kind": "local_software_declaration",
            "declaration_digest": "2" * 64, "strategy": "selected", "component": "server", "export": "service",
            "artifact_digest": result["primary"]["sha256"],
            "closure": {name[8:]: sha for name, sha in result["files"].items() if name.startswith("closure/")},
            "entrypoint": source["entrypoint"], "mcp_name": "chosen-server"}
        return {"profile": "node", "staged_id": job["id"], "component": "server",
            "package_json": backend.encoded({"fixture_plan": plan}).decode(),
            "declaration_json": backend.encoded(declaration).decode(),
            "acquired_source": {"job_id": job["id"], "receipt_sha256": result["receipt_sha256"]}}


class PrebuiltControllerTests(ControllerFixture, unittest.TestCase):
    def test_derived_source_reaches_existing_node_check_and_install(self):
        case, c, request, original, calls, _, _ = self.build()
        before = recovery.inventory(case.staging / original["id"])
        job = self.derive(c, request)
        self.assertEqual(job["family"], "selected-node-prebuilt")
        record, _ = backend.load_acquisition_recovery(c.workspace, job["id"], job["recovery_request"]["sha256"])
        self.assertEqual(record["arguments"], request)
        self.assertEqual(set(record["host"]["assets"]), backend.ACQUISITION_ASSETS["node-prebuilt"])
        selected = self.selection(job)
        checked = c.selected_check(selected)
        path, effect, plan = backend.selected_proposal(c.workspace, checked["validation_id"], checked["effect_sha256"], c.selected_binding)
        self.assertEqual(effect["request"]["profile"], "node")
        self.assertEqual(effect["request"]["acquired_source"], selected["acquired_source"])
        self.assertIn("native-payloads", plan["closure"])
        self.assertIn("native-source-receipt", plan["closure"])
        self.assertEqual((path / "closure/native-source-receipt").read_bytes(),
                         (case.staging / original["id"] / "acquisition.json").read_bytes())
        installed = c.selected_install({key: checked[key] for key in ("validation_id", "effect_sha256")})["job"]
        c.join()
        self.assertEqual(c.jobs[installed["id"]]["status"], "succeeded", c.jobs[installed["id"]])
        self.assertEqual(installed["family"], "selected-node")
        self.assertEqual(recovery.inventory(case.staging / original["id"]), before)
        self.assertEqual(len(calls), 2)

    def test_replay_reuses_job_and_cross_kind_or_changed_requests_refuse(self):
        _, c, request, original, calls, _, _ = self.build()
        job = self.derive(c, request)
        self.assertEqual(c.acquire_node_prebuilt(request)["job"], job)
        self.assertEqual(len(calls), 2)
        altered = copy.deepcopy(request)
        altered["payloads"][0]["url"] = "https://source.example/other.tar.gz"
        for value in (altered, {**request, "request_id": original["request_id"]}):
            with self.assertRaisesRegex(backend.SetupError, "acquisition_request_changed"):
                c.acquire_node_prebuilt(value)
        self.assertEqual(len(c.jobs), 2)
        self.assertEqual(len(calls), 2)

    def test_original_authority_is_current_same_controller_unrecovered_and_successful(self):
        for mutation in ("foreign", "receipt", "recovered", "unconfirmed", "derived"):
            with self.subTest(mutation=mutation):
                _, c, request, original, calls, _, _ = self.build()
                if mutation == "foreign":
                    request["source"]["job_id"] = "0" * 32
                elif mutation == "receipt":
                    request["source"]["receipt_sha256"] = "0" * 64
                elif mutation == "recovered":
                    c.jobs[original["id"]]["recovered"] = True
                elif mutation == "unconfirmed":
                    c.jobs[original["id"]]["status"] = "unconfirmed"
                else:
                    job = self.derive(c, request)
                    request = {**request, "request_id": "e" * 32,
                               "source": {"job_id": job["id"], "receipt_sha256": job["result"]["receipt_sha256"]}}
                    calls.clear()
                admitted = c.acquire_node_prebuilt(request)["job"]
                c.join()
                row = c.jobs[admitted["id"]]
                self.assertEqual(row["status"], "unconfirmed", row)
                self.assertIn(row["error"], ("node_prebuilt_source_unavailable", "selected_acquired_source_unavailable"))
                self.assertNotIn("result", row)
                self.assertEqual(calls, [])

    def test_recovery_observes_derived_bytes_without_restoring_effect_authority(self):
        case, c, request, _, calls, _, _ = self.build()
        job = self.derive(c, request)
        c.close_admission(); c.join()
        old = recovery.inventory(c.workspace)
        restored = backend.Controller(None, None, case.root / "recovered", (), sys.executable, None, None,
            recover_workspace=c.workspace, recover_request=job["id"], recover_request_sha256=job["recovery_request"]["sha256"])
        self.addCleanup(restored.join)
        with patch.object(subprocess, "Popen", side_effect=AssertionError("historical code executed")):
            observed = restored.reconcile({"job_id": job["id"]})["observation"]
        self.assertEqual(observed["status"], "matched", observed)
        self.assertEqual(observed["result"], job["result"])
        self.assertEqual(restored.job["status"], "unconfirmed")
        self.assertEqual(restored.job["error"], "historical_outcome_unknown")
        self.assertNotIn("result", restored.job)
        self.assertTrue(restored.state()["node_prebuilt_acquisition"]["readback_only"])
        for action, value in ((restored.acquire_node_prebuilt, request), (restored.selected_check, self.selection(job))):
            with self.assertRaisesRegex(backend.SetupError, "recovery_read_only"):
                action(value)
        self.assertEqual(recovery.inventory(c.workspace), old)
        self.assertEqual(len(calls), 2)


class PrebuiltMcpTests(ControllerFixture, unittest.IsolatedAsyncioTestCase):
    async def event(self, event):
        self.assertTrue(await asyncio.to_thread(event.wait, 5), "fixture gate timed out")

    async def test_actual_catalog_defaults_and_enabled_protocol_route(self):
        for enabled in (False, True):
            with self.subTest(enabled=enabled):
                _, c, request, _, calls, _, _ = self.build(enabled)
                owner = adapter.SetupAdapter(c)
                try:
                    async with Client(adapter.create_server(owner)) as client:
                        tools = {tool.name: tool for tool in (await client.list_tools()).tools}
                        self.assertEqual("setup_acquire_node_prebuilt" in tools, enabled)
                        state = (await client.call_tool("setup_state", {})).structured_content
                        self.assertEqual("node_prebuilt_acquisition" in state, enabled)
                        if not enabled:
                            response = await client.call_tool("setup_acquire_node_prebuilt", request)
                            self.assertTrue(response.is_error)
                            self.assertEqual(calls, [])
                            continue
                        tool = tools["setup_acquire_node_prebuilt"]
                        schema = tool.input_schema
                        member = schema["properties"]["payloads"]["items"]["properties"]["files"]["items"]
                        self.assertEqual(set(member["properties"]), {"member", "destination"})
                        result = await client.call_tool("setup_acquire_node_prebuilt", request)
                        self.assertFalse(result.is_error, result)
                        self.assertEqual(result.structured_content["job"]["status"], "succeeded", result)
                        self.assertFalse(c.operation_lock.locked())
                        self.assertFalse(c._job_threads[result.structured_content["job"]["id"]].is_alive())
                finally:
                    await owner.aclose()

    async def test_original_tool_waits_same_job_replay_and_explicit_stop(self):
        _, c, request, _, calls, entered, release = self.build()
        release.clear()
        owner = adapter.SetupAdapter(c)
        first = repeated = None
        try:
            async with Client(adapter.create_server(owner)) as client:
                first = asyncio.create_task(client.call_tool("setup_acquire_node_prebuilt", request))
                await self.event(entered)
                repeated = asyncio.create_task(client.call_tool("setup_acquire_node_prebuilt", request))
                await asyncio.sleep(0.03)
                self.assertFalse(first.done())
                self.assertFalse(repeated.done())
                identifier = c.job["id"]
                stopped = await client.call_tool("setup_stop", {"job_id": identifier})
                self.assertFalse(stopped.is_error, stopped)
                first_result, second_result = await asyncio.wait_for(asyncio.gather(first, repeated), 5)
                self.assertEqual(first_result.structured_content, second_result.structured_content)
                job = first_result.structured_content["job"]
                self.assertEqual(job["status"], "unconfirmed")
                self.assertEqual(job["error"], "operation_stopped")
                self.assertEqual(job["stop"]["status"], "settled")
                self.assertEqual(len(calls), 1)
                self.assertFalse(c.operation_lock.locked())
        finally:
            release.set()
            await asyncio.gather(*(item for item in (first, repeated) if item is not None), return_exceptions=True)
            await owner.aclose()

    async def test_caller_cancellation_joins_without_stopping_or_returning_late_success(self):
        _, c, request, _, calls, entered, release = self.build()
        release.clear()
        owner = adapter.SetupAdapter(c)
        task = asyncio.create_task(owner.dispatch("/api/acquire_node_prebuilt", request))
        try:
            await self.event(entered)
            task.cancel()
            await asyncio.sleep(0)
            self.assertFalse(task.done())
            self.assertNotIn("stop", c.job)
            release.set()
            with self.assertRaises(asyncio.CancelledError):
                await asyncio.wait_for(task, 5)
            self.assertEqual(c.job["status"], "succeeded", c.job)
            self.assertFalse(c.operation_lock.locked())
            replay = await owner.dispatch("/api/acquire_node_prebuilt", request)
            self.assertEqual(replay["job"], c.job)
            self.assertEqual(len(calls), 2)
        finally:
            release.set()
            await asyncio.gather(task, return_exceptions=True)
            await owner.aclose()


if __name__ == "__main__":
    unittest.main()
