"""Real MCP SDK projection of the inert Compose owner subprocesses."""
import asyncio
from contextlib import asynccontextmanager
import json
import unittest

from mcp import Client
from test_compose_bridge import ComposeFixture, adapter


@asynccontextmanager
async def connected(controller):
    owner = adapter.SetupAdapter(controller)
    try:
        async with Client(adapter.create_server(owner)) as client:
            yield client
    finally:
        await owner.aclose()


class ComposeMCPTests(ComposeFixture, unittest.IsolatedAsyncioTestCase):
    async def call(self, client, name, args):
        response = await client.call_tool(name, args)
        self.assertFalse(response.is_error, response)
        return response.structured_content

    async def settle(self, owner):
        await asyncio.to_thread(owner.join)
        return owner.job

    async def test_remove_accepts_only_explicit_canonical_namespace_through_mcp(self):
        namespace = "vendor:" + "x" * 185
        self.profile_path.write_text(json.dumps({"volumes": {namespace: {}}}))
        self.host["backend_profile"]["sha256"] = adapter.backend.digest(self.profile_path.read_bytes())
        self.host["delete_state_namespaces"] = [namespace]
        self.configure(state_namespaces=[namespace])
        owner = self.controller(); plan = self.prepared(owner)
        async with connected(owner) as client:
            tools = {tool.name: tool for tool in (await client.list_tools()).tools}
            self.assertEqual(tools["setup_compose_remove"].input_schema["properties"]["delete_state"]["items"]["maxLength"], 192)
            observed = await self.call(client, "setup_compose_observe", {})
            request = {"request_id": "7" * 32, "plan_sha256": plan,
                       "observation_sha256": observed["backend"]["observation_sha256"], "delete_state": [namespace]}
            before = self.calls()
            refused = await client.call_tool("setup_compose_remove", {**request, "delete_state": ["foreign"]})
            self.assertTrue(refused.is_error)
            self.assertEqual(self.calls(), before)
            await self.call(client, "setup_compose_remove", request)
            result = await self.settle(owner)
            self.assertEqual(result["status"], "succeeded", result)
            self.assertEqual(self.calls().count("effect:remove"), 1)

    async def test_successor_schema_is_host_pinned_and_rebind_is_mutating(self):
        initial = self.controller(); plan = self.prepared(initial)
        self.successor_host(plan); owner = self.controller()
        async with connected(owner) as client:
            tools = {tool.name: tool for tool in (await client.list_tools()).tools}
            expected = {
                "prepare_successor": {"request_id", "predecessor_plan_sha256", "inputs_sha256"},
                "adopt_successor": {"request_id", "predecessor_plan_sha256", "transition_sha256", "observation_sha256"},
                "reconcile_successor": {"request_id", "transition_sha256", "observation_sha256"},
                "rebind": {"request_id", "plan_sha256", "transition_sha256"}}
            for action, fields in expected.items():
                tool = tools["setup_compose_" + action]
                self.assertEqual(set(tool.input_schema["properties"]), fields)
                self.assertFalse(tool.annotations.read_only_hint)
            state = await self.call(client, "setup_state", {})
            await self.call(client, "setup_compose_prepare_successor", {"request_id": "a" * 32,
                "predecessor_plan_sha256": plan, "inputs_sha256": state["compose"]["successor"]["inputs_sha256"]})
            job = await self.settle(owner); self.assertEqual(job["status"], "succeeded", job)
            proposal = job["result"]["backend"]
            observation = await self.call(client, "setup_compose_observe", {})
            await self.call(client, "setup_compose_adopt_successor", {"request_id": "b" * 32,
                "predecessor_plan_sha256": plan, "transition_sha256": proposal["transition_sha256"],
                "observation_sha256": observation["backend"]["observation_sha256"]})
            job = await self.settle(owner); self.assertEqual(job["result"]["runtime_bindings"], [])
            await self.call(client, "setup_compose_rebind", {"request_id": "c" * 32,
                "plan_sha256": proposal["successor"]["plan_sha256"], "transition_sha256": proposal["transition_sha256"]})
            job = await self.settle(owner)
            self.assertEqual(job["result"]["runtime_bindings"][0]["plan_sha256"], proposal["successor"]["plan_sha256"])

    async def test_real_mcp_prepare_lifecycle_and_bindings(self):
        owner = self.controller()
        async with connected(owner) as client:
            names = {tool.name for tool in (await client.list_tools()).tools}
            self.assertTrue({"setup_compose_" + name for name in owner.compose.host["allowed_actions"]} <= names)
            self.assertNotIn("setup_selected_install", names)
            request = {"request_id": "1" * 32, **self.stage()}
            prepared = await self.call(client, "setup_compose_prepare", request)
            job = await self.settle(owner)
            detail = await self.call(client, "setup_job", {"job_id": prepared["job"]["id"]})
            self.assertEqual(detail["job"], job)
            self.assertFalse(detail["job"]["stop_supported"])
            self.assertNotIn("plan", detail["job"]["result"]["backend"])
            self.assertIn("summary", detail["job"]["result"]["backend"])
            self.assertNotIn("exact configuration", json.dumps(detail))
            self.assertNotIn("PRIVATE_COMPOSE_JSON", json.dumps(detail))
            plan = job["result"]["backend"]["plan_sha256"]
            await self.call(client, "setup_compose_start", {"request_id": "2" * 32, "plan_sha256": plan})
            await self.settle(owner)
            observed = await self.call(client, "setup_compose_observe", {})
            self.assertTrue(observed["backend"]["available"]["db"])
            self.assertEqual(observed["runtime_bindings"][0]["plan_sha256"], plan)
            refused = await client.call_tool("setup_stop", {"job_id": owner.job["id"]})
            self.assertTrue(refused.is_error)
            self.assertEqual(refused.structured_content, {"error": "job_stop_not_supported"})
            replay = await self.call(client, "setup_compose_prepare", request)
            self.assertEqual(replay["job"], prepared["job"] | detail["job"])
            self.assertEqual(self.calls().count("prepare"), 1)

    async def test_capabilities_schema_and_unconfirmed_are_not_promoted(self):
        self.host["allowed_actions"] = ["prepare", "start", "observe"]
        owner = self.controller(); plan = self.prepared(owner)
        self.configure(unconfirmed=True)
        server = adapter.create_server(adapter.SetupAdapter(owner))
        self.assertNotIn(adapter.backend.SCOPE, server.create_initialization_options().instructions)
        async with connected(owner) as client:
            tools = {t.name: t for t in (await client.list_tools()).tools}
            self.assertNotIn(adapter.backend.SCOPE, tools["setup_state"].description)
            self.assertNotIn("setup_compose_remove", tools)
            self.assertNotIn("workspace", tools["setup_compose_start"].input_schema["properties"])
            rejected = await client.call_tool("setup_compose_start", {"request_id": "x", "plan_sha256": plan})
            self.assertTrue(rejected.is_error)
            await self.call(client, "setup_compose_start", {"request_id": "3" * 32, "plan_sha256": plan})
            await self.settle(owner)
            state = await self.call(client, "setup_state", {})
            self.assertEqual(state["job"]["status"], "unconfirmed")
            self.assertEqual(state["job"]["result"]["backend"]["status"], "unconfirmed")
            self.assertEqual(state["compose"]["scope"], "execute_agent_selected_local_container_plan")

    async def test_lost_client_does_not_cancel_admitted_worker(self):
        owner = self.controller(); plan = self.prepared(owner)
        self.configure(delay=.15)
        async with connected(owner) as client:
            accepted = await self.call(client, "setup_compose_start", {"request_id": "4" * 32, "plan_sha256": plan})
            self.assertEqual(accepted["job"]["status"], "running")
        self.assertTrue(owner.closing)
        self.assertEqual(owner.job["status"], "succeeded")
        self.assertEqual(self.calls().count("effect:start"), 1)

    async def test_fixed_runtime_diagnostic_visible_on_job_and_replay(self):
        owner = self.controller(); plan = self.prepared(owner)
        self.configure(exit_code=1, stderr="engine_observation_unavailable\n")
        async with connected(owner) as client:
            request = {"request_id": "6" * 32, "plan_sha256": plan}
            first = await self.call(client, "setup_compose_start", request)
            await self.settle(owner)
            detail = await self.call(client, "setup_job", {"job_id": first["job"]["id"]})
            self.assertEqual(detail["job"]["error"], "compose_operation_unconfirmed")
            self.assertEqual(detail["job"]["runtime_error"], "engine_observation_unavailable")
            self.assertEqual(detail["job"]["status"], "unconfirmed")
            self.assertEqual((await self.call(client, "setup_compose_start", request))["job"], detail["job"])
            refused = await client.call_tool("setup_compose_observe", {})
            self.assertEqual(refused.structured_content, {"error": "compose_host_reopen_required"})
            self.assertEqual(self.calls().count("start"), 1)


if __name__ == "__main__": unittest.main()
