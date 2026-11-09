"""Real MCP/HTTP controller ownership rendered by the actual JS DOM companion.

Only package fetch is inert; no network download, install or package code runs.
"""
import asyncio
from contextlib import asynccontextmanager
import json
import os
from pathlib import Path
import shutil
import threading
import unittest

from mcp import Client
from test_mcpb_acquired_inspection import AcquiredFixture, AcquisitionRunner, adapter, backend


class BrowserRunner(AcquisitionRunner):
    def __init__(self, workspace):
        super().__init__(workspace)
        self.fetch_entered = threading.Event()
        self.fetch_release = threading.Event()

    def run(self, argv, capture=False, inspection_timeout=None):
        if "--fetch" in argv:
            request = backend.decode(Path(argv[-2]).read_bytes())["request"]
            if request["url"].endswith("/slow.mcpb"):
                self.fetch_entered.set()
                if not self.fetch_release.wait(8):
                    raise AssertionError("test gate timed out")
        return super().run(argv, capture=capture, inspection_timeout=inspection_timeout)


class BrowserTests(AcquiredFixture, unittest.IsolatedAsyncioTestCase):
    def controller(self, **changes):
        changes["runner_factory"] = BrowserRunner
        return super().controller(**changes)

    async def wait_for(self, predicate):
        async with asyncio.timeout(8):
            while not predicate():
                await asyncio.sleep(.01)

    @asynccontextmanager
    async def surfaces(self, controller):
        browser = backend.BrowserCompanion(controller)
        owner = adapter.SetupAdapter(controller, browser=browser)
        child = None
        try:
            async with Client(adapter.create_server(owner)) as client:
                child = await asyncio.create_subprocess_exec(
                    shutil.which("node"), str(Path(__file__).with_name("mcpb_browser_driver.mjs")),
                    stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE, env={"PATH": "/usr/bin:/bin"})
                async def render(command):
                    child.stdin.write((json.dumps(command) + "\n").encode())
                    await child.stdin.drain()
                    raw = await asyncio.wait_for(child.stdout.readline(), 8)
                    self.assertTrue(raw, "browser driver exited before response")
                    return json.loads(raw)
                first = await render({"origin": browser.server.origin, "capability": browser.server.capability})
                self.assertEqual(first["problem"], "")
                yield client, render
                self.assertEqual(await render({"action": "close"}), {"closed": True})
                self.assertEqual(await asyncio.wait_for(child.wait(), 5), 0)
                self.assertEqual(await child.stderr.read(), b"")
        finally:
            controller.runner.fetch_release.set()
            if child and child.returncode is None:
                child.terminate()
                try:
                    await asyncio.wait_for(child.wait(), 5)
                except TimeoutError:
                    child.kill()
                    await asyncio.wait_for(child.wait(), 5)
            await owner.aclose()
            self.assertFalse(browser.thread.is_alive())

    async def call(self, client, name, arguments):
        value = await asyncio.wait_for(client.call_tool(name, arguments), 8)
        self.assertFalse(value.is_error, value)
        return value.structured_content

    async def test_mcp_jobs_stop_history_and_manifest_recovery_render_without_reexecution(self):
        c = self.enabled()
        trace = {}
        slow = {"request_id": "1" * 32, "url": "https://artifacts.example/slow.mcpb",
                "artifact_sha256": backend.digest(b"inert archive")}
        good = {**slow, "request_id": "2" * 32, "url": "https://artifacts.example/good.mcpb"}
        async with self.surfaces(c) as (client, render):
            first = await self.call(client, "setup_acquire_mcpb", slow)
            await self.wait_for(c.runner.fetch_entered.is_set)
            trace["running"] = await render({"action": "refresh"})
            self.assertIn("MCPB bundle acquisition is running", trace["running"]["job"])
            self.assertFalse(trace["running"]["stop_disabled"])
            trace["stop_requested"] = await render({"action": "stop-job"})
            self.assertIn("Stop requested", trace["stop_requested"]["stop"])
            stop_calls = [call for call in trace["stop_requested"]["calls"] if call["path"] == "/api/stop"]
            self.assertEqual(stop_calls, [{"path": "/api/stop", "payload": {"job_id": first["job"]["id"]}, "status": 200}])
            c.runner.fetch_release.set()
            await self.wait_for(lambda: c.job["status"] != "running")
            trace["stopped"] = await render({"action": "refresh"})
            self.assertIn("acquisition outcome remains unconfirmed", trace["stopped"]["stop"])
            second = await self.call(client, "setup_acquire_mcpb", good)
            await self.wait_for(lambda: c.job["status"] != "running")
            self.assertEqual(c.job["status"], "succeeded", c.job)
            trace["succeeded"] = await render({"action": "refresh"})
            self.assertIn("MCPB bundle acquisition completed", trace["succeeded"]["job"])
            self.assertIn("Bundle bytes13", trace["succeeded"]["result"])
            self.assertNotIn("https://artifacts", trace["succeeded"]["result"])
            self.assertEqual((await self.call(client, "setup_acquire_mcpb", good))["job"]["id"], second["job"]["id"])
            trace["earlier"] = await render({"action": "select", "id": first["job"]["id"]})
            self.assertIn("unconfirmed", trace["earlier"]["job"])
            self.assertTrue(trace["earlier"]["result_hidden"])
            self.assertEqual(len(c.runner.calls), 2)
        original = {str(p.relative_to(c.workspace)): p.read_bytes()
                    for p in c.workspace.rglob("*") if p.is_file()}
        manifest = self.root / "recovery.json"
        manifest.write_bytes(backend.encoded({"schema_version": 1, "workspace": str(c.workspace),
            "requests": [c.jobs[j["job"]["id"]]["recovery_request"] for j in (first, second)]}))
        recovered = self.controller(runner_factory=BrowserRunner,
            recover_manifest=manifest, recover_manifest_sha256=backend.digest(manifest.read_bytes()))
        async with self.surfaces(recovered) as (client, render):
            trace["recovery_initial"] = await render({"action": "refresh"})
            self.assertIn("historical acquisition outcome is unknown", trace["recovery_initial"]["job"])
            self.assertTrue(trace["recovery_initial"]["stop_hidden"])
            trace["recovery_matched"] = await render({"action": "reconcile"})
            self.assertIn("historical job outcome above is unchanged", trace["recovery_matched"]["reconcile"])
            self.assertFalse(trace["recovery_matched"]["result_hidden"])
            self.assertEqual(recovered.job["status"], "unconfirmed")
            names = {tool.name for tool in (await client.list_tools()).tools}
            self.assertNotIn("setup_acquire_mcpb", names)
            self.assertNotIn("setup_selected_install", names)
            self.assertEqual(recovered.runner.calls, [])
        self.assertEqual(original, {str(p.relative_to(c.workspace)): p.read_bytes()
                         for p in c.workspace.rglob("*") if p.is_file()})
        for value in trace.values():
            self.assertFalse(any("acquire" in call["path"] or "install" in call["path"] for call in value["calls"]))
        if os.environ.get("TK_MCPB_BROWSER_EVIDENCE"):
            Path(os.environ["TK_MCPB_BROWSER_EVIDENCE"]).write_text(json.dumps(trace, indent=2) + "\n")
