"""Exercise real retained-acquisition ownership and the canonical-reader boundary.

Only the HTTPS fetch and canonical ZIP reader are replaced. These tests claim
request custody, replay and recovery semantics, not network or ZIP correctness.
"""
import asyncio
import copy
from pathlib import Path
import sys
import unittest

from test_mcpb_inspection import InspectionFixture, InspectionRunner, adapter, backend


class AcquisitionRunner(InspectionRunner):
    def run(self, argv, capture=False, inspection_timeout=None):
        if "--fetch" not in argv:
            return super().run(argv, capture=capture, inspection_timeout=inspection_timeout)
        self.calls.append((list(argv), capture))
        settings = backend.decode(Path(argv[-2]).read_bytes())
        directory = Path(settings["directory"])
        request = settings["request"]
        (directory / "artifact.mcpb").write_bytes(b"inert archive")
        (directory / "report.json").write_bytes(backend.encoded({"primary": {
            "url": request["url"], "sha256": request["artifact_sha256"], "byte_length": 13,
            "final_origin": "https://artifacts.example", "redirect_count": 0}}))
        return 0, b""


class AcquiredFixture(InspectionFixture):
    def enabled(self, **changes):
        self.network_profile = self.root / "network.json"
        self.network_profile.write_bytes(backend.encoded({"schema_version": 1,
            "scope": "trusted_public_mcpb_acquisition",
            "python": {"path": str(Path(sys.executable).resolve()),
                       "sha256": backend.digest(Path(sys.executable).read_bytes())},
            "allowed_origins": ["https://artifacts.example"], "allowed_redirect_origins": []}))
        return self.controller(runner_factory=AcquisitionRunner,
            mcpb_acquisition_profile=self.network_profile,
            mcpb_acquisition_profile_sha256=backend.digest(self.network_profile.read_bytes()),
            allow_acquired_inspection=True, **changes)

    def acquire(self, c):
        request = {"request_id": "c" * 32, "url": "https://artifacts.example/tool.mcpb",
                   "artifact_sha256": backend.digest(b"inert archive")}
        c.acquire_mcpb(request)
        c.join()
        self.assertEqual(c.job["status"], "succeeded", c.job)
        self.assertEqual(c.acquire_mcpb(request)["job"]["id"], c.job["id"])
        self.assertEqual(len(c.runner.calls), 1)
        with self.assertRaisesRegex(backend.SetupError, "acquisition_request_changed"):
            c.acquire_mcpb({**request, "url": "https://artifacts.example/other"})
        return {"job_id": c.job["id"], "receipt_sha256": c.job["result"]["receipt_sha256"], "files": []}

    def recovered(self, c, **changes):
        return self.controller(runner_factory=AcquisitionRunner,
            recover_workspace=c.workspace, recover_request=c.job["id"],
            recover_request_sha256=c.job["recovery_request"]["sha256"],
            allow_acquired_inspection=True, **changes)


class AcquiredTests(AcquiredFixture, unittest.TestCase):
    def test_replay_then_inspection_uses_retained_job_and_same_canonical_reader(self):
        c = self.enabled()
        request = self.acquire(c)
        result = c.inspect_mcpb(request)
        self.assertEqual(result["job_id"], c.job["id"])
        sent = backend.decode(c.runner.inspections[0]["raw"])
        self.assertEqual(sent["directory"], "./" + c.job["id"])
        self.assertEqual(sent["artifact_sha256"], c.job["result"]["primary"]["sha256"])
        self.assertEqual(c.runner.inspections[0]["argv"][-1], "mcpb-archive")
        self.assertEqual(c.validations, {})

    def test_recovery_requires_observation_and_never_restores_network_or_install(self):
        c = self.enabled()
        request = self.acquire(c)
        recovered = self.recovered(c)
        with self.assertRaisesRegex(backend.SetupError, "mcpb_inspection_unavailable"):
            recovered.inspect_mcpb(request)
        observation = recovered.reconcile({"job_id": c.job["id"]})["observation"]
        self.assertEqual(observation["status"], "matched")
        self.assertEqual(recovered.runner.calls, [])
        self.assertEqual(recovered.runner.inspections, [])
        self.assertEqual(recovered.inspect_mcpb(request)["job_id"], c.job["id"])
        self.assertEqual(recovered.job["status"], "unconfirmed")
        self.assertIsNone(recovered.mcpb_acquisition)
        with self.assertRaisesRegex(backend.SetupError, "recovery_read_only"):
            recovered.acquire_mcpb({})
        with self.assertRaisesRegex(backend.SetupError, "recovery_read_only"):
            recovered.inspect_mcpb(self.request)
        with self.assertRaises(backend.SetupError):
            recovered.selected_install({"validation_id": "0" * 32, "effect_sha256": "0" * 64})

    def test_wrong_job_receipt_mixed_forms_and_mutated_evidence_cannot_enter_reader(self):
        c = self.enabled()
        request = self.acquire(c)
        for bad in ({**request, "job_id": "0" * 32}, {**request, "receipt_sha256": "0" * 64},
                    {**request, "staged_id": c.job["id"]}, {**request, "artifact_sha256": "0" * 64}):
            with self.subTest(request=bad), self.assertRaises(backend.SetupError):
                c.inspect_mcpb(bad)
        targets = [self.staging / c.job["id"] / "artifact.mcpb",
                   self.staging / c.job["id"] / "acquisition.json",
                   c.workspace / "requests" / (c.job["id"] + ".json"),
                   c.workspace / "mcpb-acquisition" / "worker.py"]
        for path in targets:
            original = path.read_bytes()
            path.chmod(0o600)
            path.write_bytes(original + b"changed")
            with self.subTest(path=path), self.assertRaises(backend.SetupError):
                c.inspect_mcpb(request)
            path.write_bytes(original)
        self.assertEqual(c.runner.inspections, [])

    def test_reader_time_mutation_refuses_and_releases_operation_lock(self):
        c = self.enabled()
        request = self.acquire(c)
        artifact = self.staging / c.job["id"] / "artifact.mcpb"
        c.runner.mutate = lambda _: artifact.write_bytes(b"changed")
        with self.assertRaisesRegex(backend.SetupError, "mcpb_inspection_unavailable"):
            c.inspect_mcpb(request)
        self.assertFalse(c.operation_lock.locked())

    def test_recovery_profile_changes_refuse_before_workspace_creation(self):
        c = self.enabled()
        self.acquire(c)
        self.profile["mcpb_inspection"] = False
        self.write_profile()
        with self.assertRaisesRegex(backend.SetupError, "recovery_profile_changed"):
            self.recovered(c, workspace=self.root / "refused")
        self.assertFalse((self.root / "refused").exists())


class AcquiredMcpTests(AcquiredFixture, unittest.IsolatedAsyncioTestCase):
    async def test_actual_mcp_dispatch_and_recovery_tool_surface(self):
        from mcp import Client
        c = self.enabled()
        request = self.acquire(c)
        recovered = self.recovered(c)
        owner = adapter.SetupAdapter(recovered)
        try:
            async with Client(adapter.create_server(owner)) as client:
                names = {tool.name for tool in (await client.list_tools()).tools}
                self.assertIn("setup_inspect_mcpb", names)
                self.assertNotIn("setup_inspect_acquired", names)
                self.assertNotIn("setup_acquire_mcpb", names)
                self.assertNotIn("setup_selected_install", names)
                before = await client.call_tool("setup_inspect_mcpb", request)
                self.assertTrue(before.is_error)
                observation = await client.call_tool("setup_reconcile", {"job_id": c.job["id"]})
                self.assertFalse(observation.is_error)
                observed = await client.call_tool("setup_inspect_mcpb", request)
                self.assertFalse(observed.is_error)
                bad = await client.call_tool("setup_inspect_mcpb", self.request)
                self.assertTrue(bad.is_error)
        finally:
            await owner.aclose()
