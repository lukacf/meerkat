"""Shared-controller tests with real MCP/HTTP and inert package-owner fixtures."""
from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager, redirect_stderr, redirect_stdout
from http.client import HTTPConnection
import importlib.util
import io
import json
from pathlib import Path
import re
import signal
import socket
import subprocess
import sys
import threading
import time
import unittest
from unittest.mock import patch

from mcp import Client, StdioServerParameters


ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


adapter = load("companion_mcp", ROOT / "mcp_adapter.py")
backend = adapter.backend
fixtures = load("companion_fixtures", ROOT / "tests/test_backend.py")
fixtures.backend = backend
reconciliation = load("companion_readback", ROOT / "tests/test_reconciliation.py")
reconciliation.backend = backend
reconciliation.fixtures.backend = backend


async def eventually(predicate, seconds=5):
    async with asyncio.timeout(seconds):
        while not predicate():
            await asyncio.sleep(.01)


def http(server, route="state", value=None, changes=None):
    headers = {"Host": server.host, "Origin": server.origin,
               "X-Toolkit-Session": server.capability, "Content-Type": "application/json"}
    for key, value_change in (changes or {}).items():
        if value_change is None:
            headers.pop(key, None)
        else:
            headers[key] = value_change
    connection = HTTPConnection(*server.server_address, timeout=4)
    try:
        connection.request("POST", "/api/" + route, json.dumps(value or {}), headers)
        response = connection.getresponse()
        return response.status, json.loads(response.read())
    finally:
        connection.close()


def assert_port_closed(case, address):
    with case.assertRaises(OSError):
        with socket.create_connection(address, timeout=.3):
            pass


@asynccontextmanager
async def connected(controller):
    browser = backend.BrowserCompanion(controller)
    owner = adapter.SetupAdapter(controller, browser=browser)
    try:
        async with Client(adapter.create_server(owner)) as client:
            yield client, browser, owner
    finally:
        await owner.aclose()


class CompanionTests(fixtures.FixtureMixin, unittest.IsolatedAsyncioTestCase):
    async def call(self, client, name, arguments=None):
        result = await asyncio.wait_for(client.call_tool(name, arguments or {}), 5)
        self.assertFalse(result.is_error, result)
        return result.structured_content

    async def test_actual_mcp_and_http_share_validation_install_job_and_receipt(self):
        controller = self.controller(families=("agno",))
        async with connected(controller) as (client, browser, _):
            self.assertIs(browser.server.controller, controller)
            await self.call(client, "setup_prepare", {"family": "agno"})
            await eventually(lambda: controller.job["status"] != "running")
            status, state = await asyncio.to_thread(http, browser.server)
            self.assertEqual(status, 200)
            self.assertEqual(state, await self.call(client, "setup_state"))
            status, checked = await asyncio.to_thread(http, browser.server, "check", self.request())
            self.assertEqual(status, 200)
            selection = {**self.request(), "validation_id": checked["validation_id"],
                         "configuration_sha256": checked["configuration_sha256"]}
            controller.runner.block = threading.Event()
            controller.runner.entered.clear()
            try:
                mcp, (status, browser_result) = await asyncio.gather(
                    self.call(client, "setup_install", selection),
                    asyncio.to_thread(http, browser.server, "install", selection))
                self.assertEqual(status, 200)
                self.assertEqual(mcp["job"]["id"], browser_result["job"]["id"])
                await eventually(controller.runner.entered.is_set)
                self.assertEqual(controller.job["status"], "running")
            finally:
                controller.runner.block.set()
            await eventually(lambda: controller.job["status"] != "running")
            status, state = await asyncio.to_thread(http, browser.server)
            self.assertEqual(status, 200)
            self.assertEqual(state, await self.call(client, "setup_state"))
            self.assertEqual(state["job"]["status"], "succeeded")
            self.assertEqual((await self.call(client, "setup_install", selection))["job"], state["job"])
            result = state["job"]["result"]
            self.assertEqual(json.loads(Path(result["mcp_path"]).read_text()), result["mcp_configuration"])
            installs = [args for args, _ in controller.runner.calls if "--config" in args and "--destination" in args]
            self.assertEqual(len(installs), 1)
        self.assertTrue(controller.closing)
        self.assertFalse(browser.thread.is_alive())
        assert_port_closed(self, browser.server.server_address)

    async def test_two_companions_keep_origins_capabilities_and_controllers_separate(self):
        a, b = self.controller(families=("agno",)), self.controller(families=("services",))
        async with connected(a) as (client, first, _), connected(b) as (_, second, _):
            self.assertNotEqual(first.server.capability, second.server.capability)
            with self.assertRaises(backend.SetupError) as mismatch:
                adapter.SetupAdapter(a, browser=second)
            self.assertEqual(mismatch.exception.code, "invalid_startup")
            for changes in ({"X-Toolkit-Session": second.server.capability},
                            {"Origin": second.server.origin}, {"Host": second.server.host},
                            {"Origin": None}, {"X-Toolkit-Session": None}, {"Sec-Fetch-Site": "cross-site"}):
                status, value = await asyncio.to_thread(http, first.server, "prepare", {"family": "agno"}, changes)
                self.assertEqual(status, 403, changes)
                self.assertEqual(value, {"error": "request_not_allowed"})
            self.assertIsNone(a.job)
            self.assertIsNone(b.job)
            self.assertEqual(a.runner.calls, [])
            self.assertEqual(b.runner.calls, [])
            state = await self.call(client, "setup_state")
            tools = (await client.list_tools()).model_dump_json()
            for secret in (first.server.capability, second.server.capability):
                self.assertNotIn(secret, json.dumps(state))
                self.assertNotIn(secret, tools)
            launcher = Path(first.launcher)
            self.assertEqual(launcher.stat().st_mode & 0o777, 0o600)
            self.assertIn(first.server.origin + "/#session=" + first.server.capability, launcher.read_text())

    async def test_http_disconnect_and_http_only_close_do_not_close_mcp_owner(self):
        controller = self.controller(families=("agno",))
        async with connected(controller) as (client, browser, _):
            address = browser.server.server_address
            raw = ("POST /api/state HTTP/1.0\r\nHost: " + browser.server.host +
                   "\r\nOrigin: " + browser.server.origin + "\r\nX-Toolkit-Session: " + browser.server.capability +
                   "\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}").encode()
            with socket.create_connection(address, timeout=1) as connection:
                connection.sendall(raw)
            self.assertIsNone((await self.call(client, "setup_state"))["job"])
            await asyncio.gather(asyncio.to_thread(browser.close), asyncio.to_thread(browser.close))
            self.assertFalse(controller.closing)
            self.assertFalse(browser.thread.is_alive())
            assert_port_closed(self, address)
            await self.call(client, "setup_prepare", {"family": "agno"})
            await eventually(lambda: controller.job["status"] != "running")
            self.assertEqual(controller.job["status"], "succeeded")

    async def test_owner_close_joins_http_mcp_and_worker_despite_repeated_cancellation(self):
        controller = self.prepared()
        controller.runner.block = threading.Event()
        controller.runner.entered.clear()
        entered = [threading.Event(), threading.Event()]
        release = [threading.Event(), threading.Event()]
        dispatch_lock = threading.Lock()
        calls = 0
        original = controller.dispatch

        def gated(route, value):
            nonlocal calls
            if route == "/api/help":
                with dispatch_lock:
                    index = calls; calls += 1
                entered[index].set()
                if not release[index].wait(5):
                    raise RuntimeError("fixture gate expired")
            return original(route, value)

        async with connected(controller) as (client, browser, owner):
            selection = self.checked(controller)
            await self.call(client, "setup_install", selection)
            await eventually(controller.runner.entered.is_set)
            http_call = mcp_call = closing = None
            try:
                with patch.object(controller, "dispatch", side_effect=gated), patch.object(browser, "close", wraps=browser.close) as close:
                    http_call = asyncio.create_task(asyncio.to_thread(http, browser.server, "help", {"family": "agno", "profile": "searxng"}))
                    await eventually(entered[0].is_set)
                    mcp_call = asyncio.create_task(client.call_tool("setup_help", {"family": "agno", "profile": "searxng"}))
                    await eventually(entered[1].is_set)
                    closing = asyncio.create_task(owner.aclose())
                    await eventually(lambda: controller.closing)
                    closing.cancel(); await asyncio.sleep(.02); closing.cancel()
                    self.assertFalse(closing.done())
                    with self.assertRaises(backend.SetupError):
                        await owner.dispatch("/api/prepare", {"family": "agno"})
                    release[0].set()
                    self.assertEqual((await asyncio.wait_for(http_call, 2))[0], 409)
                    self.assertFalse(closing.done())
                    release[1].set()
                    self.assertTrue((await asyncio.wait_for(mcp_call, 2)).is_error)
                    self.assertFalse(closing.done())
                    controller.runner.block.set()
                    with self.assertRaises(asyncio.CancelledError):
                        await asyncio.wait_for(closing, 3)
                    self.assertEqual(close.call_count, 1)
                    self.assertTrue(all(not worker.is_alive() for worker in controller.threads))
                    self.assertFalse(browser.thread.is_alive())
                    assert_port_closed(self, browser.server.server_address)
            finally:
                for gate in release:
                    gate.set()
                controller.runner.block.set()
                await asyncio.gather(*[task for task in (http_call, mcp_call, closing) if task], return_exceptions=True)

    async def test_recovery_readback_is_shared_without_restoring_install_authority(self):
        old = self.controller(runner_factory=reconciliation.ReconciliationRunner)
        old.prepare({"family": "agno"}); old.join()
        request = self.checked(old)
        old.runner.nonzero_after_publish = True
        job = old.install(request)["job"]; old.join(); old.close_admission()
        current = self.controller(runner_factory=reconciliation.ReconciliationRunner,
            verifier=self.bundle / "install-library-consumer.py",
            verifier_sha256=self.manifest["files"]["install-library-consumer.py"]["sha256"],
            recover_workspace=old.workspace, recover_request=job["id"],
            recover_request_sha256=job["recovery_request"]["sha256"])
        async with connected(current) as (client, browser, _):
            self.assertEqual({tool.name for tool in (await client.list_tools()).tools}, {"setup_recovery_manifest", "setup_state", "setup_reconcile"})
            status, refused = await asyncio.to_thread(http, browser.server, "install", request)
            self.assertEqual((status, refused), (409, {"error": "recovery_read_only"}))
            observed = await self.call(client, "setup_reconcile", {"job_id": job["id"]})
            self.assertEqual(observed["observation"]["status"], "matched")
            status, state = await asyncio.to_thread(http, browser.server)
            self.assertEqual(status, 200)
            self.assertEqual(state["reconciliation"]["observation"], observed["observation"])
            self.assertEqual(state["job"]["status"], "unconfirmed")
            self.assertEqual(state["job"]["error"], "historical_outcome_unknown")


class StartupTests(fixtures.FixtureMixin, unittest.TestCase):
    def capture_server(self, captured):
        original = backend.Server
        def create(*args, **kwargs):
            server = original(*args, **kwargs)
            captured.append(server)
            return server
        return create

    def test_bind_launcher_and_thread_failures_close_http_resources(self):
        occupied = socket.socket()
        occupied.bind(("127.0.0.1", 0)); occupied.listen()
        controller = self.controller()
        try:
            with self.assertRaises(OSError):
                backend.BrowserCompanion(controller, occupied.getsockname()[1])
            self.assertFalse((controller.workspace / "start.html").exists())
            self.assertFalse(controller.closing)
        finally:
            occupied.close()
        for failure in ("launcher", "thread"):
            controller = self.controller()
            captured = []
            if failure == "launcher":
                (controller.workspace / "start.html").write_text("retained existing file")
            with patch.object(backend, "Server", side_effect=self.capture_server(captured)):
                if failure == "thread":
                    with patch.object(backend.threading.Thread, "start", side_effect=RuntimeError("fixture startup failure")):
                        with self.assertRaises(Exception):
                            backend.BrowserCompanion(controller)
                else:
                    with self.assertRaises(FileExistsError):
                        backend.BrowserCompanion(controller)
            self.assertEqual(len(captured), 1)
            self.assertEqual(captured[0].socket.fileno(), -1)
            assert_port_closed(self, captured[0].server_address)
            self.assertFalse(controller.closing)

    def test_invalid_browser_port_precedes_controller_effects(self):
        for port in ("-1", "65536", "not-a-port"):
            stdout, stderr = io.StringIO(), io.StringIO()
            with patch.object(backend, "Controller") as create, redirect_stdout(stdout), redirect_stderr(stderr):
                code = adapter.main(["--workspace", str(self.root / "must-not-exist"), "--families", "agno",
                                     "--python", sys.executable, "--browser-port", port])
            self.assertEqual(code, 2)
            create.assert_not_called()
            self.assertFalse((self.root / "must-not-exist").exists())
            self.assertEqual(stdout.getvalue(), "")

    def test_main_failure_after_browser_start_joins_companion_and_redacts_error(self):
        controller = self.controller()
        companions = []
        original = backend.BrowserCompanion
        def create(*args, **kwargs):
            browser = original(*args, **kwargs); companions.append(browser); return browser
        async def fail(_owner):
            raise RuntimeError("PRIVATE_STARTUP_DETAIL")
        stdout, stderr = io.StringIO(), io.StringIO()
        with patch.object(backend, "Controller", return_value=controller), patch.object(backend, "BrowserCompanion", side_effect=create), patch.object(adapter, "stdio", side_effect=fail), redirect_stdout(stdout), redirect_stderr(stderr):
            code = adapter.main(["--workspace", str(controller.workspace), "--families", "agno",
                                 "--python", sys.executable, "--browser-port", "0"])
        self.assertEqual(code, 2)
        self.assertTrue(controller.closing)
        self.assertEqual(len(companions), 1)
        self.assertFalse(companions[0].thread.is_alive())
        assert_port_closed(self, companions[0].server.server_address)
        self.assertEqual(stdout.getvalue(), "")
        self.assertNotIn("PRIVATE_STARTUP_DETAIL", stderr.getvalue())
        self.assertNotIn(companions[0].server.capability, stderr.getvalue())


class ProcessTests(fixtures.FixtureMixin, unittest.IsolatedAsyncioTestCase):
    async def test_actual_stdio_and_http_observe_and_replay_one_install(self):
        output = self.root / "live.json"
        params = StdioServerParameters(command=sys.executable, args=["-I", "-B", str(Path(__file__).resolve()),
                                       "--live-fixture", str(output)])
        async with Client(params) as client:
            await eventually(lambda: output.with_suffix(".ready").exists())
            info = json.loads(output.with_suffix(".ready").read_text())
            url = re.search(r'href="(http://127\.0\.0\.1:\d+)/#session=([^\"]+)"', Path(info["launcher"]).read_text())
            self.assertIsNotNone(url)
            class Endpoint:
                origin, capability = url.groups()
                host = origin.removeprefix("http://")
                server_address = ("127.0.0.1", int(host.rsplit(":", 1)[1]))
            initial = await client.call_tool("setup_state", {})
            self.assertFalse(initial.is_error)
            status, observed = await asyncio.to_thread(http, Endpoint)
            self.assertEqual(status, 200)
            self.assertEqual(observed, initial.structured_content)
            checked = await client.call_tool("setup_check", self.request())
            self.assertFalse(checked.is_error)
            selection = {**self.request(), "validation_id": checked.structured_content["validation_id"],
                         "configuration_sha256": checked.structured_content["configuration_sha256"]}
            mcp, (status, via_http) = await asyncio.gather(client.call_tool("setup_install", selection),
                                                        asyncio.to_thread(http, Endpoint, "install", selection))
            self.assertFalse(mcp.is_error)
            self.assertEqual(status, 200)
            self.assertEqual(mcp.structured_content["job"]["id"], via_http["job"]["id"])
            async with asyncio.timeout(5):
                while True:
                    state = (await client.call_tool("setup_state", {})).structured_content
                    if state["job"]["status"] != "running":
                        break
                    await asyncio.sleep(.01)
            self.assertEqual(state["job"]["status"], "succeeded")
            status, replay = await asyncio.to_thread(http, Endpoint, "install", selection)
            self.assertEqual(status, 200)
            self.assertEqual(replay["job"], state["job"])
            self.assertEqual((await client.call_tool("setup_state", {})).structured_content, (await asyncio.to_thread(http, Endpoint))[1])
        await eventually(output.exists)
        final = json.loads(output.read_text())
        self.assertEqual(final, {"exit_code": 0, "closing": True, "status": "succeeded",
                                 "workers_joined": True, "browser_joined": True, "installations": 1})
        assert_port_closed(self, Endpoint.server_address)

    async def test_eof_and_repeated_signals_join_http_dispatch_and_install_worker(self):
        for stop in ("eof", "signals"):
            with self.subTest(stop=stop):
                output = self.root / (stop + ".json")
                process = subprocess.Popen([sys.executable, "-I", "-B", str(Path(__file__).resolve()),
                                            "--process-fixture", str(output)],
                                           stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                pending = None
                try:
                    await eventually(lambda: output.with_suffix(".ready").exists(), 10)
                    info = json.loads(output.with_suffix(".ready").read_text())
                    launcher = Path(info["launcher"]).read_text()
                    url = re.search(r'href="(http://127\.0\.0\.1:\d+)/#session=([^\"]+)"', launcher)
                    self.assertIsNotNone(url)
                    class Endpoint:
                        origin, capability = url.groups()
                        host = origin.removeprefix("http://")
                        server_address = ("127.0.0.1", int(host.rsplit(":", 1)[1]))
                    pending = asyncio.create_task(asyncio.to_thread(http, Endpoint, "help", {"family": "agno", "profile": "searxng"}))
                    await eventually(lambda: output.with_suffix(".http").exists())
                    if stop == "eof":
                        process.stdin.close()
                    else:
                        process.send_signal(signal.SIGTERM)
                        await asyncio.sleep(.03)
                        process.send_signal(signal.SIGINT)
                    await eventually(lambda: output.with_suffix(".closing").exists())
                    await asyncio.sleep(.05)
                    self.assertIsNone(process.poll())
                    first_release, second_release = ((".worker-release", ".http-release") if stop == "eof"
                                                     else (".http-release", ".worker-release"))
                    output.with_suffix(first_release).touch()
                    await asyncio.sleep(.05)
                    self.assertIsNone(process.poll())
                    output.with_suffix(second_release).touch()
                    await asyncio.wait_for(asyncio.to_thread(process.wait), 5)
                    self.assertEqual(process.returncode, 0)
                    self.assertEqual(process.stdout.read(), b"")
                    stderr = process.stderr.read().decode()
                    metadata = json.loads(stderr)
                    self.assertEqual(set(metadata), {"origin", "launcher", "scope"})
                    self.assertNotIn(Endpoint.capability, stderr)
                    self.assertEqual(metadata["origin"], Endpoint.origin)
                    result = json.loads(output.read_text())
                    self.assertEqual(result, {"exit_code": 0, "closing": True, "status": "succeeded",
                                              "workers_joined": True, "browser_joined": True})
                    assert_port_closed(self, Endpoint.server_address)
                finally:
                    output.with_suffix(".worker-release").touch()
                    output.with_suffix(".http-release").touch()
                    if process.poll() is None:
                        process.kill()
                    await asyncio.to_thread(process.wait)
                    if pending:
                        await asyncio.gather(pending, return_exceptions=True)
                    for stream in (process.stdin, process.stdout, process.stderr):
                        stream.close()


def process_fixture(output, live=False):
    """Test-only child: real main/stdio/HTTP, bounded inert installer gates."""
    fixture = StartupTests("test_invalid_browser_port_precedes_controller_effects")
    fixture.setUp()
    companions = []
    controller = fixture.prepared()
    if not live:
        controller.runner.block = threading.Event()
        controller.install(fixture.checked(controller))
    original_dispatch, original_close = controller.dispatch, controller.close_admission
    original_browser, original_stdio = backend.BrowserCompanion, adapter.stdio

    def wait_file(path):
        deadline = time.monotonic() + 12
        while not path.exists() and time.monotonic() < deadline:
            time.sleep(.01)

    def worker_release():
        wait_file(output.with_suffix(".worker-release")); controller.runner.block.set()

    def dispatch(route, value):
        if not live and route == "/api/help":
            output.with_suffix(".http").touch()
            wait_file(output.with_suffix(".http-release"))
        return original_dispatch(route, value)

    def close_admission():
        original_close(); output.with_suffix(".closing").touch()

    def browser(*args, **kwargs):
        result = original_browser(*args, **kwargs); companions.append(result); return result

    async def stdio(owner):
        task = asyncio.create_task(original_stdio(owner))
        await asyncio.sleep(0)
        temporary = output.with_suffix(".ready-tmp")
        temporary.write_text(json.dumps({"launcher": str(companions[0].launcher)}))
        temporary.replace(output.with_suffix(".ready"))
        await task

    watcher = None if live else threading.Thread(target=worker_release)
    if watcher is not None:
        watcher.start()
    try:
        with patch.object(backend, "Controller", return_value=controller), patch.object(backend, "BrowserCompanion", side_effect=browser), patch.object(adapter, "stdio", side_effect=stdio), patch.object(controller, "dispatch", side_effect=dispatch), patch.object(controller, "close_admission", side_effect=close_admission):
            code = adapter.main(["--workspace", str(controller.workspace), "--families", "agno",
                                 "--python", sys.executable, "--browser-port", "0"])
        output.write_text(json.dumps({"exit_code": code, "closing": controller.closing,
            "status": controller.job["status"], "workers_joined": all(not t.is_alive() for t in controller.threads),
            "browser_joined": all(not b.thread.is_alive() for b in companions),
            **({"installations": sum("--config" in args and "--destination" in args for args, _ in controller.runner.calls)} if live else {})}))
    finally:
        output.with_suffix(".worker-release").touch(); output.with_suffix(".http-release").touch()
        if watcher is not None:
            watcher.join(timeout=2)
        fixture.tearDown()


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] in ("--process-fixture", "--live-fixture"):
        process_fixture(Path(sys.argv[2]), live=sys.argv[1] == "--live-fixture")
    else:
        unittest.main()
