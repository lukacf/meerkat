"""Node acquisition borrows setup admission, replay, cancellation and real MCP."""
import asyncio
import copy
import gzip
import io
from pathlib import Path
import subprocess
import sys
import tarfile
import unittest

from test_acquisition_jobs import Acquisition
from test_selected_setup import SelectedFixture, adapter, backend


class NodeAcquisition(Acquisition):
    def arguments(self, request):
        backend.keys(request, ("request_id", "package", "version"))
        return copy.deepcopy(request)


class NodeFixture(SelectedFixture):
    request = {"request_id": "a" * 32, "package": "@example/server", "version": "1.0.0"}
    python_request = {"request_id": "a" * 32, "distribution": "example-server", "version": "1.0.0"}

    def tearDown(self):
        for controller in self.controllers:
            for name in ("node_acquisition", "python_acquisition"):
                owner = getattr(controller, name)
                if isinstance(owner, Acquisition):
                    owner.release.set()
        super().tearDown()

    def enabled(self):
        controller = self.controller()
        controller.node_acquisition = NodeAcquisition()
        controller.python_acquisition = Acquisition()
        self.addCleanup(controller.node_acquisition.release.set)
        self.addCleanup(controller.python_acquisition.release.set)
        (controller.workspace / "acquisitions").mkdir()
        return controller

    def host_profiles(self):
        """Inert trusted tool bytes exercise startup custody, not npm execution."""
        npm = self.root / "npm.tgz"
        raw = io.BytesIO()
        with tarfile.open(fileobj=raw, mode="w") as archive:
            for name, data in {"package.json": b'{"name":"npm","version":"12.2.0"}',
                               "bin/npm-cli.js": b"// inert fixture\n"}.items():
                member = tarfile.TarInfo("package/" + name)
                member.size = len(data)
                archive.addfile(member, io.BytesIO(data))
        npm.write_bytes(gzip.compress(raw.getvalue(), mtime=0))
        runtime_sha = backend.digest(Path(sys.executable).resolve().read_bytes())
        node_profile = self.root / "node-acquisition.json"
        node_profile.write_bytes(backend.encoded({
            "schema_version": 1, "scope": "trusted_npm_registry_acquisition",
            "python_sha256": runtime_sha, "node_sha256": runtime_sha,
            "npm": {"path": str(npm), "sha256": backend.digest(npm.read_bytes())}}))
        pip = self.root / "pip.pyz"
        pip.write_bytes(b"inert pinned pip fixture\n")
        python_profile = self.root / "python-acquisition.json"
        python_profile.write_bytes(backend.encoded({
            "schema_version": 1, "scope": "trusted_pypi_wheel_acquisition",
            "python_sha256": runtime_sha,
            "pip": {"path": str(pip), "sha256": backend.digest(pip.read_bytes())}}))
        return {"node_acquisition_profile": node_profile,
                "node_acquisition_profile_sha256": backend.digest(node_profile.read_bytes()),
                "python_acquisition_profile": python_profile,
                "python_acquisition_profile_sha256": backend.digest(python_profile.read_bytes())}


class NodeAcquisitionJobs(NodeFixture, unittest.TestCase):
    def test_both_real_owners_bind_one_job_directory_and_exact_host_profiles(self):
        profiles = self.host_profiles()
        controller = self.controller(**profiles)
        for kind in ("python", "node"):
            owner = getattr(controller, kind + "_acquisition")
            owner.verify()
            self.assertEqual(owner.workspace, controller.workspace)
            self.assertEqual(owner.staging, self.staging)
            self.assertEqual((owner.assets / "profile.json").read_bytes(),
                             profiles[kind + "_acquisition_profile"].read_bytes())
            self.assertEqual(controller.state()[kind + "_acquisition"]["host_profile_sha256"],
                             profiles[kind + "_acquisition_profile_sha256"])
        self.assertEqual(list((controller.workspace / "acquisitions").iterdir()), [])
        self.assertEqual(controller.runner.calls, [])
        self.assertEqual(controller.jobs, {})

    def test_node_profile_digest_mismatch_refuses_before_workspace_creation(self):
        profiles = self.host_profiles()
        profiles["node_acquisition_profile_sha256"] = "0" * 64
        with self.assertRaisesRegex(backend.SetupError, "invalid_node_acquisition_profile"):
            self.controller(**profiles)
        self.assertFalse((self.root / ("work-" + str(self.sequence))).exists())

    def test_actual_mcp_cli_accepts_host_profile_and_joins_clean_eof(self):
        profiles = self.host_profiles()
        workspace = self.root / "cli-workspace"
        args = [sys.executable, "-I", "-B", str(Path(adapter.__file__)),
                "--workspace", str(workspace), "--families", "", "--python", sys.executable,
                "--node", sys.executable, "--npm-cli", sys.executable,
                "--selected-install-profile", str(self.profile_path),
                "--selected-install-profile-sha256", self.profile_sha]
        for key, value in profiles.items():
            args.extend(["--" + key.replace("_", "-"), str(value)])
        result = subprocess.run(args, input=b"", capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, b"")
        self.assertEqual((workspace / "node-acquisition/profile.json").read_bytes(),
                         profiles["node_acquisition_profile"].read_bytes())
        self.assertEqual(list((workspace / "acquisitions").iterdir()), [])

    def test_disabled_by_default_and_no_staging_or_jobs(self):
        controller = self.controller()
        with self.assertRaisesRegex(backend.SetupError, "node_acquisition_unavailable"):
            controller.acquire_node(self.request)
        self.assertNotIn("node_acquisition", controller.state())
        self.assertEqual(controller.jobs, {})
        self.assertFalse((controller.workspace / "acquisitions").exists())

    def test_profile_requires_selected_node_authority_before_workspace_creation(self):
        self.profile["validators"].pop("node")
        self.write_profile()
        for settings in ({"node_acquisition_profile": self.root / "absent.json"},
                         {"node_acquisition_profile_sha256": "0" * 64}):
            with self.subTest(settings=settings), self.assertRaisesRegex(
                    backend.SetupError, "invalid_node_acquisition_profile"):
                self.controller(**settings)
            self.assertFalse((self.root / ("work-" + str(self.sequence))).exists())

    def test_replay_is_bound_to_kind_and_arguments_and_request_is_retained(self):
        controller = self.enabled()
        first = controller.dispatch("/api/acquire_node", self.request)["job"]
        self.assertTrue(controller.node_acquisition.entered.wait(2))
        observed, directory = controller.node_acquisition.calls[0]
        self.assertEqual(backend.decode((directory / "request.json").read_bytes()), self.request)
        self.assertEqual(observed, self.request)
        self.assertEqual(first["kind"], "acquire")
        self.assertEqual(first["family"], "selected-node")
        self.assertEqual(controller.acquire_node(self.request)["job"]["id"], first["id"])
        for operation, request in ((controller.acquire_python, self.python_request),
                                   (controller.acquire_node, {**self.request, "version": "2.0.0"})):
            with self.assertRaisesRegex(backend.SetupError, "acquisition_request_changed"):
                operation(request)
        controller.node_acquisition.release.set()
        controller.join()
        final = controller.acquire_node(self.request)["job"]
        self.assertEqual(final["status"], "succeeded")
        self.assertEqual(len(controller.node_acquisition.calls), 1)
        self.assertFalse(controller.reconciliation_available(final))
        self.assertEqual(controller.validations, {})
        self.assertEqual(controller.install_intents, {})

    def test_stopped_node_job_is_joined_and_replay_does_not_retry(self):
        controller = self.enabled()
        first = controller.acquire_node(self.request)["job"]
        self.assertTrue(controller.node_acquisition.entered.wait(2))
        controller.stop({"job_id": first["id"]})
        controller.join()
        final = controller.acquire_node(self.request)["job"]
        self.assertEqual(final["status"], "unconfirmed")
        self.assertEqual(final["error"], "operation_stopped")
        self.assertEqual(final["stop"]["status"], "settled")
        self.assertEqual(len(controller.node_acquisition.calls), 1)
        self.assertFalse(controller.operation_lock.locked())

    def test_languages_share_operation_lock_and_closing_admission(self):
        controller = self.enabled()
        controller.acquire_node(self.request)
        self.assertTrue(controller.node_acquisition.entered.wait(2))
        with self.assertRaisesRegex(backend.SetupError, "operation_in_progress"):
            controller.acquire_python({**self.python_request, "request_id": "b" * 32})
        self.assertEqual(len(controller.acquisition_requests), 1)
        controller.close_admission()
        with self.assertRaisesRegex(backend.SetupError, "host_closing"):
            controller.acquire_node({**self.request, "request_id": "c" * 32})
        controller.node_acquisition.release.set()
        controller.join()

    def test_languages_share_one_128_request_cap_and_existing_replay_still_works(self):
        controller = self.enabled()
        controller.node_acquisition.release.set()
        controller.python_acquisition.release.set()
        for index in range(128):
            operation, template = ((controller.acquire_node, self.request) if index % 2 == 0
                                   else (controller.acquire_python, self.python_request))
            operation({**template, "request_id": f"{index:032x}"})
            controller.join()
        for operation, template in ((controller.acquire_node, self.request),
                                    (controller.acquire_python, self.python_request)):
            with self.assertRaisesRegex(backend.SetupError, "new_workspace_required"):
                operation({**template, "request_id": "f" * 32})
        self.assertEqual(len(controller.jobs), 128)
        self.assertEqual(controller.acquire_node({**self.request, "request_id": "0" * 32})["job"]["status"], "succeeded")


class NodeAcquisitionMcp(NodeFixture, unittest.IsolatedAsyncioTestCase):
    async def test_real_mcp_lists_exact_schema_routes_node_and_preserves_python(self):
        from mcp import Client

        controller = self.enabled()
        controller.node_acquisition.release.set()
        controller.python_acquisition.release.set()
        owner = adapter.SetupAdapter(controller)
        try:
            async with Client(adapter.create_server(owner)) as client:
                tools = {tool.name: tool for tool in (await client.list_tools()).tools}
                tool = tools["setup_acquire_node"]
                self.assertFalse(tool.annotations.read_only_hint)
                self.assertTrue(tool.annotations.idempotent_hint)
                self.assertTrue(tool.annotations.open_world_hint)
                self.assertEqual(set(tool.input_schema["properties"]), {"request_id", "package", "version"})
                self.assertIn("setup_acquire_python", tools)
                self.assertTrue((await client.call_tool("setup_acquire_node", {**self.request, "registry": "https://other.invalid"})).is_error)
                self.assertEqual(controller.jobs, {})
                result = await client.call_tool("setup_acquire_node", self.request)
                self.assertFalse(result.is_error, result)
                await asyncio.to_thread(controller.join)
                job = result.structured_content["job"]
                observed = await client.call_tool("setup_job", {"job_id": job["id"]})
                self.assertEqual(observed.structured_content["job"]["status"], "succeeded")
                replay = await client.call_tool("setup_acquire_node", self.request)
                self.assertEqual(replay.structured_content["job"]["id"], job["id"])
                self.assertTrue((await client.call_tool("setup_acquire_python", self.python_request)).is_error)
                python = await client.call_tool("setup_acquire_python", {**self.python_request, "request_id": "b" * 32})
                self.assertFalse(python.is_error, python)
                await asyncio.to_thread(controller.join)
                self.assertEqual(len(controller.node_acquisition.calls), 1)
                self.assertEqual(len(controller.python_acquisition.calls), 1)
        finally:
            await owner.aclose()

    async def test_disabled_tools_are_not_advertised(self):
        from mcp import Client

        owner = adapter.SetupAdapter(self.controller())
        try:
            async with Client(adapter.create_server(owner)) as client:
                tools = {tool.name for tool in (await client.list_tools()).tools}
                self.assertNotIn("setup_acquire_node", tools)
                self.assertNotIn("setup_acquire_python", tools)
        finally:
            await owner.aclose()


if __name__ == "__main__":
    unittest.main()
