"""Ordinary MCP adapter checks with the real SDK and inert package-owner fixtures."""
from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager
import copy
import importlib.util
import json
from pathlib import Path
import signal
import subprocess
import sys
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, patch

from mcp import Client, StdioServerParameters, types


ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


adapter = load("consumer_setup_mcp", ROOT / "mcp_adapter.py")
fixtures = load("consumer_setup_fixtures", ROOT / "tests/test_backend.py")
fixtures.backend = adapter.backend
reconciliation = load("consumer_setup_reconciliation_fixtures", ROOT / "tests/test_reconciliation.py")
reconciliation.backend = adapter.backend
reconciliation.fixtures.backend = adapter.backend
CANARY = "PRIVATE_EXCEPTION_MUST_NOT_ESCAPE"


async def eventually(predicate, timeout=5):
    async with asyncio.timeout(timeout):
        while not predicate():
            await asyncio.sleep(0.01)


@asynccontextmanager
async def connection(controller):
    owner = adapter.SetupAdapter(controller)
    try:
        async with Client(adapter.create_server(owner)) as client:
            yield client, owner
    finally:
        await owner.aclose()


class AdapterTests(fixtures.FixtureMixin, unittest.IsolatedAsyncioTestCase):
    async def call(self, client, name, arguments=None):
        result = await client.call_tool(name, arguments)
        self.assertFalse(result.is_error, result)
        self.assertEqual(json.loads(result.content[0].text), result.structured_content)
        return result.structured_content

    async def test_enabled_tool_inventory_projects_object_roots_but_keeps_strict_entry(self):
        # Enable every local registration branch. Dispatch is inert: this checks
        # the real MCP surface and its entry validator, not package execution.
        controller = SimpleNamespace(families={"services": {}, "agno": {}}, recovery_only=False,
            python_acquisition=object(), node_acquisition=object(), node_search=object(),
            mcpb_acquisition=object(), compose_acquisition=object(),
            compose=SimpleNamespace(host={"allowed_actions": ["prepare", "start", "observe", "resume",
                "stop", "remove", "prepare_successor", "adopt_successor", "reconcile_successor", "rebind"]}),
            local_recipes={"scope": adapter.backend.LOCAL_RECIPES_RETAIN_SCOPE}, allow_remote_recipes=True,
            routes_sha256="b" * 64, prior_request_sha256="b" * 64, allow_acquired_inspection=True,
            acquired_inspection_raw=True, selected={"validators": {"python": {}, "node": {}, "mcpb": {}},
                "mcpb_inspection": True, "plugin_inspection": True, "package_inspector": {}}, scope=lambda: adapter.backend.SCOPE,
            state=lambda: {"job_history": {"recovery_selection": True}})
        owner = adapter.SetupAdapter(controller, connection_reader=AsyncMock(), connection_scope="fixture")
        owner.guides = {"fixture": {}}
        owner.dispatch = AsyncMock(return_value={"entered": True})
        job, digest = "a" * 32, "b" * 64
        export = {"publisher": "fixture", "name": "example", "version": "1", "digest": digest,
                  "declaration_json": "{}"}
        inspect = {"job_id": job, "receipt_sha256": digest, "after": None}
        bundle = {"job_id": job, "receipt_sha256": digest, "files": []}
        selected = {"profile": "python", "staged_id": job, "component": "example",
                    "package_json": "{}", "declaration_json": "{}"}
        async with Client(adapter.create_server(owner)) as client:
            tools = {tool.name: tool for tool in (await client.list_tools()).tools}
            cases = {"setup_export_recipe": export, "setup_inspect_acquired": inspect,
                     "setup_inspect_mcpb": bundle, "setup_selected_check": selected}
            self.assertTrue(cases.keys() <= tools.keys())
            self.assertIn("setup_inspect_package", tools)
            for tool in tools.values():
                self.assertEqual(tool.input_schema["type"], "object", tool.name)
                self.assertIs(tool.input_schema["additionalProperties"], False, tool.name)
                self.assertFalse({"oneOf", "anyOf", "allOf", "enum", "not"} & tool.input_schema.keys(), tool.name)
            for name, arguments in cases.items():
                owner.dispatch.reset_mock()
                self.assertEqual(await self.call(client, name, arguments), {"entered": True})
                owner.dispatch.assert_awaited_once_with("/api/" + name.removeprefix("setup_"), arguments)
            # Listing is a detached display: even changing a returned nested
            # property cannot change the strict validator captured at startup.
            tools["setup_selected_check"].input_schema["properties"]["profile"] = {}
            invalid = [
                ("setup_export_recipe", dict(export, activation_json="{}")),
                ("setup_inspect_acquired", dict(inspect, member="file.py", offset=0)),
                ("setup_inspect_acquired", {"job_id": job, "receipt_sha256": digest}),
                ("setup_inspect_mcpb", dict(bundle, staged_id=job, artifact_sha256=digest)),
                ("setup_inspect_mcpb", {"files": []}),
                ("setup_selected_check", dict(selected, activation_json="{}")),
                ("setup_selected_check", dict(selected, package_sha256=digest, selection_sha256=digest)),
                ("setup_selected_check", dict(selected, profile="mcpb", configuration_json="{}")),
                ("setup_selected_check", {"profile": "mcpb", "staged_id": job, "component": "example",
                    "package_sha256": digest, "selection_sha256": digest}),
                ("setup_selected_check", {"profile": "python", "staged_id": job, "component": "example",
                    "package_sha256": digest, "selection_sha256": digest,
                    "acquired_source": {"job_id": job, "receipt_sha256": digest}}),
            ]
            for name, arguments in invalid:
                owner.dispatch.reset_mock()
                result = await client.call_tool(name, arguments)
                self.assertTrue(result.is_error, name)
                self.assertEqual(result.structured_content, {"error": "invalid_arguments"})
                owner.dispatch.assert_not_awaited()
        # Recovery still accepts only the retained job form, never staged input.
        controller.recovery_only = True
        async with Client(adapter.create_server(owner)) as client:
            owner.dispatch.reset_mock()
            self.assertEqual(await self.call(client, "setup_inspect_mcpb", bundle), {"entered": True})
            owner.dispatch.assert_awaited_once()
            owner.dispatch.reset_mock()
            result = await client.call_tool("setup_inspect_mcpb",
                {"staged_id": job, "artifact_sha256": digest, "files": []})
            self.assertTrue(result.is_error)
            self.assertEqual(result.structured_content, {"error": "invalid_arguments"})
            owner.dispatch.assert_not_awaited()

    async def test_catalog_real_mcp_before_prepare_preserves_all_choices(self):
        catalog_fixture = load("consumer_catalog_fixture", ROOT / "tests/test_catalog.py")
        catalog_fixture.backend = adapter.backend
        expected = catalog_fixture.entries(); catalog_fixture.pin(self, expected)
        controller = self.controller(families=("services",))
        async with connection(controller) as (client, owner):
            listed = await self.call(client, "setup_catalog", {})
            self.assertEqual([row["identity"] for row in listed["entries"]], [row["identity"] for row in expected])
            for row in expected:
                actual = await self.call(client, "setup_catalog_entry", row["identity"])
                self.assertEqual(actual["entry"], row)
            for payload in ({"name": "slack"}, dict(expected[0]["identity"], family="services")):
                self.assertTrue((await client.call_tool("setup_catalog_entry", payload)).is_error)
            self.assertEqual(controller.runner.calls, [])
            self.assertEqual(controller.families["services"]["status"], "unprepared")
            self.assertIsNone(controller.job)

    async def test_recovered_session_only_lists_and_checks_original_request(self):
        original = self.controller(runner_factory=reconciliation.ReconciliationRunner)
        original.prepare({"family": "agno"}); original.join()
        request = self.checked(original)
        original.runner.nonzero_after_publish = True
        job = original.install(request)["job"]; original.join(); original.close_admission()
        recovered = self.controller(runner_factory=reconciliation.ReconciliationRunner,
            verifier=self.bundle / "install-library-consumer.py",
            verifier_sha256=self.manifest["files"]["install-library-consumer.py"]["sha256"],
            recover_workspace=original.workspace, recover_request=job["id"],
            recover_request_sha256=job["recovery_request"]["sha256"])
        async with connection(recovered) as (client, owner):
            tools = await client.list_tools()
            self.assertEqual({t.name for t in tools.tools}, {"setup_recovery_manifest", "setup_state", "setup_reconcile"})
            before = await self.call(client, "setup_state")
            self.assertEqual(before["mode"], "recovery")
            self.assertEqual(before["job"]["error"], "historical_outcome_unknown")
            self.assertEqual(recovered.runner.calls, [])
            for name, args in [("setup_catalog", {}), ("setup_catalog_entry", {"publisher": "fixture", "name": "slack", "version": "1"}),
                               ("setup_prepare", {"family": "agno"}), ("setup_help", {"family": "agno", "profile": "searxng"}),
                               ("setup_check", self.request()), ("setup_install", request),
                               ("setup_jobs", {"before": None}), ("setup_job", {"job_id": job["id"]})]:
                refused = await client.call_tool(name, args)
                self.assertTrue(refused.is_error)
            observed = await self.call(client, "setup_reconcile", {"job_id": job["id"]})
            self.assertEqual(observed["observation"]["status"], "matched")
            self.assertEqual(recovered.job["status"], "unconfirmed")
            self.assertEqual(len(recovered.runner.calls), 1)
            self.assertIn("--verify-existing", recovered.runner.calls[0][0])

    async def test_recovery_selection_lists_and_checks_each_request_without_mutation_tools(self):
        original = self.controller(runner_factory=reconciliation.ReconciliationRunner)
        original.prepare({"family": "agno"})
        original.join()
        identifiers = []
        for _ in range(2):
            identifiers.append(original.install(self.checked(original))["job"]["id"])
            original.join()
        original.close_admission()
        path = self.root / "recovery-selection.json"
        path.write_bytes(adapter.backend.encoded({"schema_version": 1, "workspace": str(original.workspace),
            "requests": [original.jobs[identifier]["recovery_request"] for identifier in identifiers]}))
        recovered = self.controller(runner_factory=reconciliation.ReconciliationRunner,
            verifier=self.bundle / "install-library-consumer.py",
            verifier_sha256=self.manifest["files"]["install-library-consumer.py"]["sha256"],
            recover_manifest=path, recover_manifest_sha256=adapter.backend.digest(path.read_bytes()))
        async with connection(recovered) as (client, _):
            tools = (await client.list_tools()).tools
            self.assertEqual({tool.name for tool in tools}, {"setup_recovery_manifest", "setup_state", "setup_jobs", "setup_job", "setup_reconcile"})
            self.assertTrue(all(tool.annotations.read_only_hint for tool in tools))
            page = await self.call(client, "setup_jobs", {"before": None})
            self.assertEqual([row["id"] for row in page["jobs"]], identifiers[::-1])
            self.assertEqual(recovered.runner.calls, [])
            for identifier in identifiers:
                before = await self.call(client, "setup_job", {"job_id": identifier})
                self.assertEqual(before["job"]["error"], "historical_outcome_unknown")
                self.assertNotIn("result", before["job"])
                observed = await self.call(client, "setup_reconcile", {"job_id": identifier})
                self.assertEqual(observed["observation"]["status"], "matched")
                after = await self.call(client, "setup_job", {"job_id": identifier})
                self.assertEqual(after["job"], before["job"])
                self.assertEqual(after["reconciliation"]["observation"], observed["observation"])
            self.assertTrue((await client.call_tool("setup_prepare", {"family": "agno"})).is_error)
            self.assertTrue((await client.call_tool("setup_job", {"job_id": "f" * 32})).is_error)
            self.assertTrue(all("--verify-existing" in argv for argv, _ in recovered.runner.calls))

    async def test_real_sdk_flow_receipt_and_exact_install_replay(self):
        controller = self.controller(families=("agno",))
        async with connection(controller) as (client, owner):
            listed = await client.list_tools()
            self.assertEqual({tool.name for tool in listed.tools},
                             {"setup_recovery_manifest", "setup_state", "setup_jobs", "setup_job", "setup_stop", "setup_catalog", "setup_catalog_entry", "setup_prepare", "setup_retry_prepare", "setup_help", "setup_check", "setup_install", "setup_reconcile"})
            for tool in listed.tools:
                self.assertFalse(tool.input_schema["additionalProperties"])
                self.assertNotIn("workspace", tool.input_schema["properties"])
                if "family" in tool.input_schema["properties"]:
                    self.assertEqual(tool.input_schema["properties"]["family"]["enum"], ["agno"])
            before = await self.call(client, "setup_state")
            self.assertEqual(before["families"][0]["status"], "unprepared")
            await self.call(client, "setup_prepare", {"family": "agno"})
            await eventually(lambda: controller.state()["job"]["status"] != "running")
            help_value = await self.call(client, "setup_help", {"family": "agno", "profile": "searxng"})
            self.assertEqual(help_value["profile"], "searxng")
            request = self.request()
            validation = await self.call(client, "setup_check", request)
            install = dict(request, validation_id=validation["validation_id"],
                           configuration_sha256=validation["configuration_sha256"])
            first = await self.call(client, "setup_install", install)
            await eventually(lambda: controller.state()["job"]["status"] != "running")
            state = await self.call(client, "setup_state")
            self.assertEqual(state["job"]["status"], "succeeded")
            self.assertTrue(Path(state["job"]["result"]["receipt_path"]).is_file())
            self.assertTrue(Path(state["job"]["result"]["mcp_path"]).is_file())
            self.assertIn("mcpServers", state["job"]["result"]["mcp_configuration"])
            replay = await self.call(client, "setup_install", install)
            self.assertEqual(first["job"]["id"], replay["job"]["id"])
            self.assertEqual(replay["job"], state["job"])
            installs = [args for args, _ in controller.runner.calls if "--config" in args and "--destination" in args]
            self.assertEqual(len(installs), 1)
            changed = copy.deepcopy(install)
            changed["values"]["timeout"] = 11
            refusal = await client.call_tool("setup_install", changed)
            self.assertTrue(refusal.is_error)
            self.assertEqual(refusal.structured_content, {"error": "validation_changed"})
        self.assertTrue(controller.closing)
        self.assertTrue(owner._close_task.done())

    async def test_history_keeps_older_install_and_cancelled_check_observable(self):
        controller = self.controller(runner_factory=reconciliation.ReconciliationRunner,
            verifier=self.bundle / "install-library-consumer.py",
            verifier_sha256=self.manifest["files"]["install-library-consumer.py"]["sha256"])
        controller.prepare({"family": "agno"}); controller.join()
        request = self.checked(controller)
        controller.runner.nonzero_after_publish = True
        first = controller.install(request)["job"]["id"]; controller.join()
        controller.runner.nonzero_after_publish = False
        second = controller.install(self.checked(controller))["job"]["id"]; controller.join()
        historical = copy.deepcopy(controller.jobs[first])
        current = copy.deepcopy(controller.job)
        async with connection(controller) as (client, owner):
            tools = {tool.name: tool for tool in (await client.list_tools()).tools}
            for name in ("setup_jobs", "setup_job"):
                self.assertTrue(tools[name].annotations.read_only_hint)
                self.assertTrue(tools[name].annotations.idempotent_hint)
                self.assertFalse(tools[name].annotations.open_world_hint)
            before = list(controller.runner.calls)
            page = await self.call(client, "setup_jobs", {"before": None})
            self.assertEqual([row["id"] for row in page["jobs"]][:2], [second, first])
            self.assertEqual((await self.call(client, "setup_job", {"job_id": first}))["job"], historical)
            self.assertEqual(controller.runner.calls, before)
            controller.runner.verify_block = threading.Event()
            check = asyncio.create_task(client.call_tool("setup_reconcile", {"job_id": first}))
            await eventually(controller.runner.verify_entered.is_set)
            try:
                self.assertTrue((await self.call(client, "setup_state"))["operation_in_progress"])
                value = await self.call(client, "setup_job", {"job_id": first})
                self.assertEqual(value["reconciliation"]["observation"]["status"], "checking")
                check.cancel()
            finally:
                controller.runner.verify_block.set()
            with self.assertRaises(asyncio.CancelledError):
                await check
            await eventually(lambda: not owner._inflight)
            before = list(controller.runner.calls)
            value = await self.call(client, "setup_job", {"job_id": first})
            self.assertEqual(value["job"], historical)
            self.assertEqual(value["reconciliation"]["observation"]["status"], "matched")
            value["reconciliation"]["observation"]["result"].clear()
            self.assertTrue((await self.call(client, "setup_job", {"job_id": first}))["reconciliation"]["observation"]["result"])
            self.assertEqual((await self.call(client, "setup_state"))["job"], current)
            self.assertEqual(controller.runner.calls, before)

    async def test_history_pagination_and_foreign_session_refusal(self):
        controller = self.controller()
        for index in range(53):
            identifier = f"{index:032x}"
            controller.jobs[identifier] = {"id": identifier, "kind": "install", "family": "agno", "status": "unconfirmed"}
        async with connection(controller) as (client, _):
            first = await self.call(client, "setup_jobs", {"before": None})
            second = await self.call(client, "setup_jobs", {"before": first["next_before"]})
            self.assertEqual((len(first["jobs"]), len(second["jobs"])), (50, 3))
            self.assertIsNone(second["next_before"])
            self.assertEqual(len({row["id"] for row in first["jobs"] + second["jobs"]}), 53)
            self.assertIsNone((await self.call(client, "setup_state"))["job"])
            self.assertEqual(controller.runner.calls, [])
        async with connection(self.controller()) as (client, _):
            self.assertEqual(await self.call(client, "setup_jobs", {"before": None}), {"jobs": [], "next_before": None})
            for name, arguments in (("setup_job", {"job_id": identifier}), ("setup_jobs", {"before": identifier})):
                refused = await client.call_tool(name, arguments)
                self.assertTrue(refused.is_error)
                self.assertEqual(refused.structured_content, {"error": "unknown_job"})

    async def test_boundaries_prevent_dispatch(self):
        controller = self.controller(families=("agno",))
        async with connection(controller) as (client, _):
            bad = [
                ("setup_prepare", {"family": "services"}),
                ("setup_prepare", {"family": "agno", "workspace": "/tmp/elsewhere"}),
                ("setup_prepare", {"family": "agno", "command": "execute"}),
                ("setup_state", {"secret": CANARY}),
                ("setup_jobs", {}),
                ("setup_jobs", {"before": "invalid"}),
                ("setup_jobs", {"before": None, "workspace": CANARY}),
                ("setup_job", {"job_id": None}),
                ("setup_job", {"job_id": "0" * 32, "destination": CANARY}),
                ("setup_reconcile", {"job_id": "0" * 32, "destination": CANARY}),
                ("setup_check", dict(self.request(), values={"nested": {"secret": CANARY}})),
                ("setup_check", dict(self.request(), values={"host": "a" * 2049})),
                ("setup_check", dict(self.request(), operations=["search_web"] * 2)),
                ("setup_check", dict(self.request(), values={str(i): "x" * 2048 for i in range(40)})),
            ]
            with patch.object(controller, "dispatch", side_effect=AssertionError("must not dispatch")) as dispatch:
                for name, arguments in bad:
                    result = await client.call_tool(name, arguments)
                    self.assertTrue(result.is_error)
                    self.assertEqual(result.structured_content, {"error": "invalid_arguments"})
                    self.assertNotIn(CANARY, result.model_dump_json())
                dispatch.assert_not_called()
            self.assertEqual(controller.runner.calls, [])

    async def test_reconcile_real_sdk_after_lost_install_response_and_cancelled_request(self):
        controller = self.controller(runner_factory=reconciliation.ReconciliationRunner,
            verifier=self.bundle / "install-library-consumer.py",
            verifier_sha256=self.manifest["files"]["install-library-consumer.py"]["sha256"])
        controller.prepare({"family": "agno"}); controller.join()
        request = self.checked(controller)
        controller.runner.nonzero_after_publish = True
        job_id = controller.install(request)["job"]["id"]; controller.join()
        historical = copy.deepcopy(controller.jobs[job_id])
        async with connection(controller) as (client, owner):
            observed = await self.call(client, "setup_reconcile", {"job_id": job_id})
            self.assertEqual(observed["observation"]["status"], "matched")
            self.assertEqual((await self.call(client, "setup_state"))["job"], historical)
            controller.runner.verify_block = threading.Event()
            controller.runner.verify_entered.clear()
            call = asyncio.create_task(client.call_tool("setup_reconcile", {"job_id": job_id}))
            await eventually(controller.runner.verify_entered.is_set)
            call.cancel()
            controller.runner.verify_block.set()
            with self.assertRaises(asyncio.CancelledError):
                await call
            await eventually(lambda: not owner._inflight)
            state = await self.call(client, "setup_state")
            self.assertEqual(state["reconciliation"]["observation"]["status"], "matched")
            self.assertEqual(state["job"], historical)
            self.assertEqual((await self.call(client, "setup_install", request))["job"], historical)
            invalid = await client.call_tool("setup_reconcile", {"job_id": "0" * 32})
            self.assertEqual(invalid.structured_content, {"error": "unknown_installation"})
        installs = [args for args, _ in controller.runner.calls
                    if "--config" in args and "--destination" in args and "--verify-existing" not in args]
        self.assertEqual(len(installs), 1)

    async def test_errors_are_value_free_and_config_references_are_names(self):
        controller = self.prepared("services")
        async with connection(controller) as (client, _):
            for error in (OSError(CANARY), KeyError(CANARY), json.JSONDecodeError(CANARY, CANARY, 0)):
                with patch.object(controller, "dispatch", side_effect=error):
                    result = await client.call_tool("setup_state")
                self.assertEqual(result.structured_content, {"error": "request_failed"})
                self.assertNotIn(CANARY, result.model_dump_json())
            request = {"family": "services", "profile": "slack", "operations": ["auth_test"],
                       "values": {"token_env": "literal-token-with-dashes", "timeout_seconds": 2}}
            denied = await client.call_tool("setup_check", request)
            self.assertEqual(denied.structured_content, {"error": "environment_reference_name_required"})
            request["values"]["token_env"] = "SLACK_TOKEN"
            allowed = await self.call(client, "setup_check", request)
            self.assertEqual(allowed["scope"], "configuration_only")

    async def test_cancellation_joins_dispatch_and_existing_install_survives(self):
        controller = self.prepared()
        install = self.checked(controller)
        controller.runner.block = threading.Event()
        controller.runner.entered.clear()
        entered, release = threading.Event(), threading.Event()
        original = controller.dispatch
        def blocked_return(route, arguments):
            value = original(route, arguments)
            entered.set()
            release.wait(5)
            return value
        owner = adapter.SetupAdapter(controller)
        with patch.object(controller, "dispatch", side_effect=blocked_return):
            call = asyncio.create_task(owner.dispatch("/api/install", install))
            await eventually(entered.is_set)
            await eventually(controller.runner.entered.is_set)
            call.cancel()
            await asyncio.sleep(0)
            self.assertFalse(call.done())
            call.cancel()
            release.set()
            with self.assertRaises(asyncio.CancelledError):
                await call
        closing = asyncio.create_task(owner.aclose())
        await eventually(lambda: controller.closing)
        self.assertFalse(closing.done())
        closing.cancel()
        await asyncio.sleep(0)
        closing.cancel()
        controller.runner.block.set()
        with self.assertRaises(asyncio.CancelledError):
            await closing
        await owner.aclose()
        self.assertEqual(controller.state()["job"]["status"], "succeeded")
        self.assertTrue(all(not thread.is_alive() for thread in controller.threads))
        with self.assertRaises(adapter.backend.SetupError) as error:
            await owner.dispatch("/api/state", {})
        self.assertEqual(error.exception.code, "host_closing")
        installs = [args for args, _ in controller.runner.calls if "--config" in args and "--destination" in args]
        self.assertEqual(len(installs), 1)

    async def test_sdk_request_cancellation_preserves_job_observation(self):
        controller = self.prepared()
        install = self.checked(controller)
        entered, release = threading.Event(), threading.Event()
        original = controller.dispatch
        def blocked_return(route, arguments):
            value = original(route, arguments)
            if route == "/api/install":
                entered.set()
                release.wait(5)
            return value
        async with connection(controller) as (client, owner):
            with patch.object(controller, "dispatch", side_effect=blocked_return):
                call = asyncio.create_task(client.call_tool("setup_install", install))
                await eventually(entered.is_set)
                call.cancel()
                release.set()
                with self.assertRaises(asyncio.CancelledError):
                    await call
                await eventually(lambda: not owner._inflight)
            await eventually(lambda: controller.state()["job"]["status"] != "running")
            replay = await self.call(client, "setup_install", install)
            self.assertEqual(replay["job"]["status"], "succeeded")
            self.assertEqual(len([args for args, _ in controller.runner.calls if "--config" in args and "--destination" in args]), 1)

    async def test_stdio_sdk_roundtrip_and_eof_close(self):
        output = self.root / "stdio-complete.json"
        params = StdioServerParameters(command=sys.executable,
            args=["-I", str(Path(__file__).resolve()), "--stdio-fixture", str(output)])
        async with Client(params) as client:
            state = await self.call(client, "setup_state")
            self.assertEqual(state["families"][0]["id"], "agno")
            await self.call(client, "setup_prepare", {"family": "agno"})
            async with asyncio.timeout(5):
                while (await self.call(client, "setup_state"))["job"]["status"] == "running":
                    await asyncio.sleep(0.01)
            page = await self.call(client, "setup_jobs", {"before": None})
            self.assertEqual(len(page["jobs"]), 1)
            observed = await self.call(client, "setup_job", {"job_id": page["jobs"][0]["id"]})
            self.assertEqual(observed["job"]["status"], "succeeded")
        self.assertEqual(json.loads(output.read_text()), {"closing": True, "status": "succeeded", "workers_joined": True})

    async def test_stdio_eof_and_signals_wait_for_admitted_install(self):
        for stop in ("eof", "signals"):
            with self.subTest(stop=stop):
                output = self.root / (stop + ".json")
                process = subprocess.Popen([sys.executable, "-I", str(Path(__file__).resolve()),
                    "--pending-fixture", str(output)], stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                try:
                    # Child startup includes imports and durable request fsyncs.
                    # Keep the shutdown assertions below on their original budget.
                    await eventually(lambda: output.with_suffix(".entered").exists(), timeout=15)
                    if stop == "eof":
                        process.stdin.close()
                    else:
                        process.send_signal(signal.SIGTERM)
                        await asyncio.sleep(0.05)
                        process.send_signal(signal.SIGINT)
                    await asyncio.sleep(0.1)
                    self.assertIsNone(process.poll())
                    output.with_suffix(".release").touch()
                    await eventually(lambda: process.poll() is not None)
                    self.assertEqual(process.returncode, 0)
                    self.assertEqual(process.stdout.read(), b"")
                    self.assertEqual(process.stderr.read(), b"")
                    self.assertEqual(json.loads(output.read_text()),
                                     {"closing": True, "status": "succeeded", "workers_joined": True})
                finally:
                    output.with_suffix(".release").touch()
                    if process.poll() is None:
                        process.kill()
                    process.wait(timeout=5)
                    for stream in (process.stdin, process.stdout, process.stderr):
                        stream.close()

    async def test_malformed_stdio_rejection_joins_admitted_install(self):
        for name, payload in (("utf8", b"\xff" + CANARY.encode() + b"\n"),
                              ("size", b"x" * (2 * adapter.backend.MAX_BODY + 1) + b"\n")):
            with self.subTest(name=name):
                output = self.root / (name + ".json")
                process = subprocess.Popen([sys.executable, "-I", str(Path(__file__).resolve()),
                    "--invalid-fixture", str(output)], stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                try:
                    await eventually(lambda: output.with_suffix(".entered").exists(), timeout=15)
                    await asyncio.to_thread(process.stdin.write, payload)
                    process.stdin.close()
                    await asyncio.sleep(0.1)
                    self.assertIsNone(process.poll())
                    output.with_suffix(".release").touch()
                    await eventually(lambda: process.poll() is not None)
                    self.assertEqual(process.returncode, 0)
                    self.assertEqual(process.stdout.read(), b"")
                    error = process.stderr.read()
                    self.assertEqual(error, b"consumer setup unavailable; preserve the workspace; check host configuration and optional MCP SDK\n")
                    self.assertEqual(json.loads(output.read_text()), {"closing": True,
                        "status": "succeeded", "workers_joined": True, "exit_code": 2})
                finally:
                    output.with_suffix(".release").touch()
                    if process.poll() is None:
                        process.kill()
                    process.wait(timeout=5)
                    for stream in (process.stdin, process.stdout, process.stderr):
                        stream.close()


def stdio_fixture(output, pending=False, invalid=False):
    fixture = AdapterTests("test_stdio_sdk_roundtrip_and_eof_close")
    fixture.setUp()
    try:
        controller = fixture.prepared() if pending else fixture.controller(families=("agno",))
        if pending:
            block = threading.Event()
            controller.runner.block = block
            controller.runner.entered.clear()
            controller.install(fixture.checked(controller))
            def release():
                import time
                while not output.with_suffix(".release").exists():
                    time.sleep(0.01)
                block.set()
            threading.Thread(target=release, daemon=True).start()
        async def run():
            # Give stdio() the first loop turn to install signal handlers.
            task = asyncio.create_task(adapter.stdio(adapter.SetupAdapter(controller)))
            await asyncio.sleep(0)
            if pending:
                output.with_suffix(".entered").touch()
            try:
                await task
            except asyncio.CancelledError:
                pass
        result = {}
        if invalid:
            output.with_suffix(".entered").touch()
            with patch.object(adapter.backend, "Controller", return_value=controller):
                result["exit_code"] = adapter.main(["--bundle", str(fixture.bundle),
                    "--contents-sha256", fixture.sha, "--workspace", str(controller.workspace),
                    "--families", "agno", "--python", sys.executable])
        else:
            asyncio.run(run())
        output.write_text(json.dumps({**result, "closing": controller.closing,
            "status": controller.state()["job"]["status"],
            "workers_joined": all(not thread.is_alive() for thread in controller.threads)}))
    finally:
        fixture.tearDown()


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] in ("--stdio-fixture", "--pending-fixture", "--invalid-fixture"):
        stdio_fixture(Path(sys.argv[2]), pending=sys.argv[1] != "--stdio-fixture", invalid=sys.argv[1] == "--invalid-fixture")
    else:
        unittest.main()
