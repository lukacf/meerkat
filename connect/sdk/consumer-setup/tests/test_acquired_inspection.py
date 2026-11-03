"""Acquired archive research uses existing owners and the real process boundary."""
import json
import os
import subprocess
import sys
import unittest
from unittest.mock import patch

import test_acquisition_recovery as recovery_tests
import test_python_acquisition as python_tests
from test_selected_setup import adapter, backend


class InspectionFixture:
    def acquired(self, kind="python", enabled=True, extra=None):
        original = backend.Controller
        with patch.object(backend, "Controller", side_effect=lambda *a, **kw:
                          original(*a, **kw, allow_acquired_inspection=enabled)):
            case, controller, owner = recovery_tests.AcquisitionRecoveryTests.package_controller(self, kind)
        if extra is not None:
            case.report["install"] = []
            case.payloads = {}
            case.add_wheel("outside_server", requested=True, extra=extra)
        controller._acquire(kind, owner, case.request)
        controller.join()
        self.assertEqual(controller.job["status"], "succeeded", controller.job)
        request = {"job_id": controller.job["id"], "receipt_sha256": controller.job["result"]["receipt_sha256"], "after": None}
        return case, controller, request


class InspectionTests(InspectionFixture, unittest.TestCase):
    def test_real_worker_lists_and_reads_only_verified_primary(self):
        for kind in ("python", "node"):
            with self.subTest(kind=kind):
                case, c, request = self.acquired(kind)
                before = recovery_tests.inventory(case.staging)
                result = c.inspect_acquired(request)
                self.assertEqual(result["scope"], "acquired_primary_archive_only")
                self.assertEqual(result["archive_sha256"], c.job["result"]["primary"]["sha256"])
                self.assertEqual(result["profile"], kind)
                self.assertTrue(result["members"])
                self.assertIsNone(result["next_after"])
                member = next(row for row in result["members"] if row["path"].endswith("METADATA" if kind == "python" else "package.json"))
                read = c.inspect_acquired({**{k: request[k] for k in ("job_id", "receipt_sha256")}, "member": member["path"], "offset": 0})
                self.assertEqual(read["member_sha256"], member["sha256"])
                self.assertEqual(read["member_bytes"], member["bytes"])
                self.assertTrue(read["text"])
                self.assertEqual(recovery_tests.inventory(case.staging), before)
                self.assertEqual(list((c.workspace / "tmp").iterdir()), [])

    def test_wrong_job_receipt_and_changed_inputs_refuse_before_worker(self):
        case, c, request = self.acquired()
        for bad in ({**request, "job_id": "0" * 32}, {**request, "receipt_sha256": "0" * 64},
                    {**request, "member": "../private"}, {**request, "after": 7}):
            with patch.object(c.runner, "run_compose", side_effect=AssertionError("worker entered")):
                with self.assertRaises(backend.SetupError):
                    c.inspect_acquired(bad)
        artifact = case.staging / c.job["id"] / "artifact.whl"
        artifact.write_bytes(artifact.read_bytes() + b"changed")
        with patch.object(c.runner, "run_compose", side_effect=AssertionError("worker entered")):
            with self.assertRaisesRegex(backend.SetupError, "acquired_inspection_unavailable"):
                c.inspect_acquired(request)

    def test_optin_and_succeeded_job_required(self):
        _, c, request = self.acquired(enabled=False)
        with self.assertRaisesRegex(backend.SetupError, "acquired_inspection_unavailable"):
            c.inspect_acquired(request)
        _, c, request = self.acquired()
        for status in ("running", "unconfirmed"):
            c.jobs[request["job_id"]]["status"] = status
            with self.assertRaises(backend.SetupError):
                c.inspect_acquired(request)

    def test_final_readback_detects_worker_time_mutation_and_custom_run_is_respected(self):
        case, c, request = self.acquired()
        original = c.runner.run_compose
        def changing(*args, **kwargs):
            result = original(*args, **kwargs)
            receipt = case.staging / c.job["id"] / "acquisition.json"
            receipt.write_bytes(receipt.read_bytes() + b"\n")
            return result
        with patch.object(c.runner, "run_compose", side_effect=changing):
            with self.assertRaisesRegex(backend.SetupError, "acquired_inspection_unavailable"):
                c.inspect_acquired(request)
        _, c, request = self.acquired()
        with patch.object(c.runner, "run", return_value=(0, b"{}")) as run:
            with self.assertRaisesRegex(backend.SetupError, "acquired_inspection_unavailable"):
                c.inspect_acquired(request)
            run.assert_called_once()

    def test_pages_and_utf8_chunks_preserve_member_identity(self):
        text = "x" * 32767 + "caf\u00e9\n" * 200
        extra = {"docs/%03d.txt" % n: str(n).encode() for n in range(70)}
        extra["docs/long.txt"] = text.encode()
        _, c, request = self.acquired(extra=extra)
        page = c.inspect_acquired(request)
        self.assertEqual(len(page["members"]), 64)
        later = c.inspect_acquired({**request, "after": page["next_after"]})
        self.assertIsNone(later["next_after"])
        names = [r["path"] for r in page["members"] + later["members"]]
        self.assertEqual(names, sorted(set(names)))
        args = {k: request[k] for k in ("job_id", "receipt_sha256")}
        parts, offset = [], 0
        while offset is not None:
            result = c.inspect_acquired({**args, "member": "docs/long.txt", "offset": offset})
            self.assertEqual(result["member_sha256"], backend.digest(text.encode()))
            self.assertLessEqual(len(result["text"].encode()), 32768)
            parts.append(result["text"])
            offset = result["next_offset"]
        self.assertEqual("".join(parts), text)

    def test_binary_controls_large_text_and_bad_offsets_do_not_leak(self):
        extra = {"docs/binary.txt": b"PRIVATE\x00DATA", "docs/large.txt": b"x" * (1024 * 1024 + 1),
                 "docs/utf8.txt": "caf\u00e9".encode()}
        _, c, request = self.acquired(extra=extra)
        args = {k: request[k] for k in ("job_id", "receipt_sha256")}
        for member, offset in (("docs/binary.txt", 0), ("docs/large.txt", 0), ("docs/utf8.txt", 4)):
            with self.assertRaisesRegex(backend.SetupError, "^acquired_member_unreadable$"):
                c.inspect_acquired({**args, "member": member, "offset": offset})
        for member in ("../secret", "/etc/passwd", "missing"):
            with self.assertRaisesRegex(backend.SetupError, "^acquired_inspection_unavailable$"):
                c.inspect_acquired({**args, "member": member, "offset": 0})

    def test_original_job_not_latest_and_worker_mutation_refuse(self):
        _, c, request = self.acquired()
        c.job = {"kind": "prepare", "id": "0" * 32}
        self.assertEqual(c.inspect_acquired(request)["job_id"], request["job_id"])
        worker = c.workspace / "acquired-inspection.py"
        worker.write_bytes(worker.read_bytes() + b"# changed\n")
        with patch.object(c.runner, "run_compose", side_effect=AssertionError("entered")):
            with self.assertRaisesRegex(backend.SetupError, "acquired_inspection_unavailable"):
                c.inspect_acquired(request)

    def test_worker_timeout_and_output_limit_join_child(self):
        for body in ("import os,time\nopen('inspector.pid','w').write(str(os.getpid()))\ntime.sleep(20)",
                     "import os,sys\nopen('inspector.pid','w').write(str(os.getpid()))\nsys.stdout.write('x'*262145)"):
            _, c, request = self.acquired()
            # Controlled replacement of the trusted worker tests process custody.
            c.acquired_inspection_raw = body.encode()
            (c.workspace / "acquired-inspection.py").write_bytes(c.acquired_inspection_raw)
            with patch.object(backend, "INSPECTION_TIMEOUT", 0.2):
                with self.assertRaisesRegex(backend.SetupError, "acquired_inspection_unavailable"):
                    c.inspect_acquired(request)
            pid = int((c.workspace / "inspector.pid").read_text())
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)
            self.assertFalse(c.operation_lock.locked())

    def test_worker_uses_full_owner_validation_before_listing(self):
        case, c, request = self.acquired()
        filename, raw, _ = python_tests.W.wheel(mutate_record=lambda rows: rows[0].__setitem__(1, "sha256-invalid"))
        artifact = case.root / "invalid.whl"
        artifact.write_bytes(raw)
        inputs = {"installer": str(case.installer), "installer_sha256": backend.digest(case.installer.read_bytes()),
                  "artifact": str(artifact), "archive_sha256": backend.digest(raw), "profile": "python",
                  "filename": filename, "selection": {"after": None}}
        result = subprocess.run([sys.executable, "-I", "-B", str(c.workspace / "acquired-inspection.py")],
                                input=backend.encoded(inputs), capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(json.loads(result.stdout), {"error": "acquired_inspection_unavailable"})

    def test_changed_live_python_bytes_refuse_before_process_entry(self):
        for kind in ("python", "node"):
            case, c, request = self.acquired(kind)
            owner = getattr(c, kind + "_acquisition")
            expected = owner.python_raw if kind == "python" else owner.inputs[owner.python]
            runtime = case.root / "selected-python-copy"
            runtime.write_bytes(expected)
            runtime.chmod(0o700)
            owner.python = runtime
            if kind == "node":
                owner.inputs[runtime] = expected
            c.python = str(runtime)
            runtime.write_bytes(b"#!/bin/sh\nexit 99\n")
            with patch.object(c.runner, "run_compose", side_effect=AssertionError("changed runtime entered")) as run:
                with self.assertRaisesRegex(backend.SetupError, "acquired_inspection_unavailable"):
                    c.inspect_acquired(request)
                run.assert_not_called()


class McpInspectionTests(InspectionFixture, unittest.IsolatedAsyncioTestCase):
    async def test_advertisement_and_recovery_remain_separate(self):
        from mcp import Client
        _, c, request = self.acquired()
        owner = adapter.SetupAdapter(c)
        try:
            async with Client(adapter.create_server(owner)) as client:
                self.assertIn("setup_inspect_acquired", {t.name for t in (await client.list_tools()).tools})
                result = await client.call_tool("setup_inspect_acquired", request)
                self.assertFalse(result.is_error, result)
                self.assertEqual(result.structured_content["scope"], "acquired_primary_archive_only")
        finally:
            await owner.aclose()
        recovered = recovery_tests.AcquisitionRecoveryTests.recovered(self, c)
        owner = adapter.SetupAdapter(recovered)
        try:
            async with Client(adapter.create_server(owner)) as client:
                self.assertEqual({t.name for t in (await client.list_tools()).tools}, {"setup_recovery_manifest", "setup_state", "setup_reconcile"})
                with self.assertRaisesRegex(backend.SetupError, "acquired_inspection_unavailable"):
                    recovered.inspect_acquired(request)
        finally:
            await owner.aclose()


if __name__ == "__main__":
    unittest.main()
