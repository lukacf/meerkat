"""Live host-reader boundary through the real MCP SDK, without service calls."""
import asyncio
from contextlib import asynccontextmanager
import json
import sys
import unittest
from unittest.mock import patch

from mcp import Client

from test_mcp_adapter import adapter, eventually, fixtures, load, reconciliation, ROOT


def snapshot(scope="audience-a", *, revision=1, account="work-a"):
    """Existing UI projection shape; controls/diagnostics must stay private."""
    status = {"text": "Host observed connected", "tone": "ready"}
    return {"scope": scope, "revision": revision, "observedAt": "2026-10-06T04:00:00Z",
        "tasks": [{"id": "PRIVATE_TASK"}], "announcement": "PRIVATE_ANNOUNCEMENT",
        "frontends": [], "packages": [], "catalog": [],
        "connections": [{"id": "slack-" + account, "packageId": "services", "service": "slack",
            "account": account, "credentialScope": "channels:history chat:write",
            "status": status, "pendingTaskIds": ["PRIVATE_TASK"],
            "controls": [{"id": "PRIVATE_CONTROL"}], "diagnostics": "PRIVATE_DIAGNOSTICS",
            "grants": [{"kind": "service", "id": "grant-" + account, "label": "Read chosen channel",
                "status": status, "account": account, "usedBy": "Original setup task",
                "operations": ["slack_read_interval"], "resources": ["channel:C-allowed"],
                "lifetime": "This task", "standingAuthority": "No standing permission",
                "controls": [{"id": "PRIVATE_GRANT_CONTROL"}]}]}]}


@asynccontextmanager
async def connection(controller, reader=None, scope=None):
    owner = adapter.SetupAdapter(controller, connection_reader=reader, connection_scope=scope)
    try:
        async with Client(adapter.create_server(owner)) as client:
            yield client, owner
    finally:
        await owner.aclose()


class ConnectionTests(fixtures.FixtureMixin, unittest.IsolatedAsyncioTestCase):
    async def call(self, client, arguments=None):
        result = await client.call_tool("setup_connections", {} if arguments is None else arguments)
        self.assertEqual(json.loads(result.content[0].text), result.structured_content)
        return result

    async def test_real_mcp_live_filtered_projection_and_embedding_consumer(self):
        current = snapshot()
        calls = []

        async def reader():
            calls.append(current["revision"])
            return current

        controller = self.controller(families=("services",))
        owner = adapter.SetupAdapter(controller, connection_reader=reader, connection_scope="audience-a")
        with patch.dict(sys.modules, {"mcp_adapter": adapter}):
            consumer = load("connection_consumer", ROOT / "examples/inspect_connections.py")
        try:
            first = await consumer.inspect_connections(owner)
            self.assertEqual(first["status"], "observed")
            self.assertEqual(first["scope"], "host_connection_candidates_only")
            candidate = first["connections"][0]
            self.assertEqual(candidate["observedStatus"], "Host observed connected")
            self.assertEqual(candidate["credentialScope"], "channels:history chat:write")
            self.assertEqual(candidate["grants"][0]["operations"], ["slack_read_interval"])
            self.assertNotIn("PRIVATE_", json.dumps(first))
            self.assertNotIn("ready", candidate)
            current["connections"][0]["account"] = "changed-account"
            current["revision"] = 2
            second = await consumer.inspect_connections(owner)
            self.assertEqual(second["observation"]["revision"], 2)
            self.assertEqual(second["connections"][0]["account"], "changed-account")
            self.assertEqual(first["connections"][0]["account"], "work-a")
            self.assertEqual(calls, [1, 2])
            self.assertIsNone(owner._close_task)  # The consumer borrows host custody.
            self.assertEqual(controller.runner.calls, [])
            self.assertIsNone(controller.job)
        finally:
            await owner.aclose()

    async def test_host_filtered_readers_cannot_be_overridden_by_agent(self):
        for scope, account in (("audience-a", "work-a"), ("audience-b", "work-b")):
            with self.subTest(scope=scope):
                calls = []

                async def reader():
                    calls.append(scope)
                    return snapshot(scope, account=account)

                async with connection(self.controller(), reader, scope) as (client, _):
                    tool = next(t for t in (await client.list_tools()).tools if t.name == "setup_connections")
                    self.assertEqual(tool.input_schema, {"type": "object", "properties": {}, "additionalProperties": False})
                    self.assertTrue(tool.annotations.read_only_hint)
                    for arguments in ({"scope": "audience-other"}, {"account": "other"}, {"query": "gmail"}):
                        self.assertTrue((await self.call(client, arguments)).is_error)
                    self.assertEqual(calls, [])
                    result = await self.call(client)
                    self.assertFalse(result.is_error)
                    self.assertEqual([row["account"] for row in result.structured_content["connections"]], [account])
                    self.assertEqual(calls, [scope])

    async def test_absent_reader_and_recovery_do_not_advertise_inventory(self):
        async with connection(self.controller()) as (client, owner):
            self.assertNotIn("setup_connections", {t.name for t in (await client.list_tools()).tools})
            self.assertTrue((await self.call(client)).is_error)
            with self.assertRaises(adapter.backend.SetupError) as caught:
                await owner.dispatch("/api/connections", {})
            self.assertEqual(caught.exception.code, "connection_reader_unavailable")
        original = self.controller(runner_factory=reconciliation.ReconciliationRunner)
        original.prepare({"family": "agno"}); original.join()
        job = original.install(self.checked(original))["job"]; original.join(); original.close_admission()
        recovered = self.controller(runner_factory=reconciliation.ReconciliationRunner,
            verifier=self.bundle / "install-library-consumer.py",
            verifier_sha256=self.manifest["files"]["install-library-consumer.py"]["sha256"],
            recover_workspace=original.workspace, recover_request=job["id"],
            recover_request_sha256=job["recovery_request"]["sha256"])

        async def forbidden():
            self.fail("Recovery must not consult a reader")

        async with connection(recovered, forbidden, "audience-a") as (client, owner):
            self.assertNotIn("setup_connections", {t.name for t in (await client.list_tools()).tools})
            self.assertTrue((await self.call(client)).is_error)
            with self.assertRaises(adapter.backend.SetupError):
                await owner.dispatch("/api/connections", {})
            self.assertEqual(recovered.runner.calls, [])

    async def test_valid_empty_denied_and_error_are_distinct_live_results(self):
        mode = "empty"
        calls = []

        async def reader():
            calls.append(mode)
            if mode in ("denied", "unavailable"):
                raise adapter.ConnectionReadUnavailable(mode)
            if mode == "error":
                raise RuntimeError("PRIVATE_EXCEPTION")
            value = snapshot()
            value["connections"] = []
            return value

        controller = self.controller()
        async with connection(controller, reader, "audience-a") as (client, _):
            empty = await self.call(client)
            self.assertFalse(empty.is_error)
            self.assertEqual(empty.structured_content["connections"], [])
            for mode, code in (("denied", "connection_read_denied"), ("unavailable", "connection_read_unavailable"),
                               ("error", "connection_read_unavailable")):
                result = await self.call(client)
                self.assertTrue(result.is_error)
                self.assertEqual(result.structured_content, {"error": code})
                self.assertNotIn("PRIVATE_EXCEPTION", str(result))
            self.assertEqual(calls, ["empty", "denied", "unavailable", "error"])
            self.assertEqual(controller.runner.calls, [])

    async def test_closed_projection_refuses_scope_secrets_types_and_excess(self):
        variants = []
        changed = snapshot("audience-other"); variants.append((changed, "connection_scope_changed"))
        changed = snapshot(); changed["connections"][0]["token"] = "PRIVATE_TOKEN"; variants.append((changed, "connection_projection_invalid"))
        changed = snapshot(); changed["revision"] = True; variants.append((changed, "connection_projection_invalid"))
        changed = snapshot(); changed["connections"] *= 2; variants.append((changed, "connection_projection_invalid"))
        changed = snapshot(); changed["connections"][0]["grants"][0]["resources"] = ["x" * 2048] * 256
        variants.append((changed, "connection_projection_invalid"))
        changed = snapshot(); changed["connections"][0]["account"] = "\ud800"; variants.append((changed, "connection_projection_invalid"))
        for value, code in variants:
            with self.subTest(code=code):
                async def reader():
                    return value
                async with connection(self.controller(), reader, "audience-a") as (client, _):
                    result = await self.call(client)
                    self.assertTrue(result.is_error)
                    self.assertEqual(result.structured_content, {"error": code})
                    self.assertNotIn("PRIVATE_TOKEN", str(result))

    async def test_cancelled_read_joins_cleanup_and_discards_suppressed_late_result(self):
        entered, cancelled, release, settled = (asyncio.Event() for _ in range(4))

        async def reader():
            entered.set()
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                cancelled.set()
                await release.wait()
                return snapshot()
            finally:
                settled.set()

        owner = adapter.SetupAdapter(self.controller(), connection_reader=reader, connection_scope="audience-a")
        task = asyncio.create_task(owner.dispatch("/api/connections", {}))
        try:
            await asyncio.wait_for(entered.wait(), 2)
            task.cancel()
            await asyncio.wait_for(cancelled.wait(), 2)
            self.assertFalse(task.done())
            release.set()
            with self.assertRaises(asyncio.CancelledError):
                await task
            self.assertTrue(settled.is_set())
            self.assertEqual(owner._connection_reads, set())
            self.assertEqual(owner.controller.runner.calls, [])
        finally:
            release.set()
            await owner.aclose()

    async def test_real_mcp_cancellation_settles_reader_and_next_read_is_live(self):
        entered, settled = asyncio.Event(), asyncio.Event()
        calls = 0

        async def reader():
            nonlocal calls
            calls += 1
            if calls == 1:
                entered.set()
                try:
                    await asyncio.Event().wait()
                finally:
                    settled.set()
            return snapshot(revision=calls)

        async with connection(self.controller(), reader, "audience-a") as (client, owner):
            task = asyncio.create_task(client.call_tool("setup_connections", {}))
            await asyncio.wait_for(entered.wait(), 2)
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await task
            await asyncio.wait_for(settled.wait(), 2)
            await eventually(lambda: not owner._connection_reads)
            result = await self.call(client)
            self.assertFalse(result.is_error)
            self.assertEqual(result.structured_content["observation"]["revision"], 2)
            self.assertEqual(calls, 2)

    async def test_close_cancels_joins_reader_and_refuses_new_admission(self):
        entered, cancelled, release = (asyncio.Event() for _ in range(3))

        async def reader():
            entered.set()
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                cancelled.set()
                await release.wait()
                return snapshot()

        owner = adapter.SetupAdapter(self.controller(), connection_reader=reader, connection_scope="audience-a")
        task = asyncio.create_task(owner.dispatch("/api/connections", {}))
        await asyncio.wait_for(entered.wait(), 2)
        closing = asyncio.create_task(owner.aclose())
        try:
            await asyncio.wait_for(cancelled.wait(), 2)
            self.assertFalse(closing.done())
            with self.assertRaises(adapter.backend.SetupError) as caught:
                await owner.dispatch("/api/connections", {})
            self.assertEqual(caught.exception.code, "host_closing")
            release.set()
            with self.assertRaises(adapter.backend.SetupError) as caught:
                await task
            self.assertEqual(caught.exception.code, "host_closing")
            await closing
            self.assertEqual(owner._connection_reads, set())
        finally:
            release.set()
            await owner.aclose()


if __name__ == "__main__":
    unittest.main()
