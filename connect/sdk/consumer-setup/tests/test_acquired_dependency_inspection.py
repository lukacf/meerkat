"""Retained dependency inspection preserves the exact acquisition owner boundary."""
import copy
import json
import subprocess
import sys
import tarfile
import unittest
from unittest.mock import patch

import test_acquisition_recovery as recovery
import test_node_acquisition as node_tests
from test_acquired_inspection import InspectionFixture
from test_recovered_acquired_inspection import RecoveryFixture
from test_selected_setup import adapter, backend


class DependencyFixture(InspectionFixture):
    def dependency(self, kind="node", payload=None):
        original = backend.Controller
        with patch.object(backend, "Controller", side_effect=lambda *a, **kw:
                          original(*a, **kw, allow_acquired_inspection=True)):
            case, controller, owner = recovery.AcquisitionRecoveryTests.package_controller(self, kind)
        if payload is not None:
            if kind == "node":
                case.dependency = node_tests.W.package("dependency", "1.0.0", payload)
            else:
                case.report["install"] = []
                case.payloads = {}
                case.add_wheel("outside_server", requested=True)
                case.add_wheel("dependency", extra=payload)
        controller._acquire(kind, owner, case.request)
        controller.join()
        self.assertEqual(controller.job["status"], "succeeded", controller.job)
        result = controller.job["result"]
        row = next(row for row in result["artifacts"] if row != result["primary"])
        request = {"job_id": controller.job["id"], "receipt_sha256": result["receipt_sha256"],
                   "archive_sha256": row["sha256"], "after": None}
        name = "closure/" + row["filename"] if kind == "python" else "closure/archive-" + row["sha256"]
        return case, controller, request, case.staging / controller.job["id"] / name


class DependencyInspectionTests(DependencyFixture, unittest.TestCase):
    def test_both_dependency_formats_use_real_worker_and_preserve_primary(self):
        for kind in ("node", "python"):
            with self.subTest(kind=kind):
                case, c, request, _ = self.dependency(kind)
                before, calls = recovery.inventory(case.staging), len(case.calls)
                capabilities = c.state()["acquired_inspection"]
                self.assertEqual(capabilities["dependency_scope"], "acquired_dependency_archive_only")
                self.assertEqual(capabilities["archive_selection"], "optional_archive_sha256_from_job_artifacts")
                result = c.inspect_acquired(request)
                self.assertEqual(result["scope"], "acquired_dependency_archive_only")
                self.assertEqual(result["archive_sha256"], request["archive_sha256"])
                self.assertEqual(result["job_id"], request["job_id"])
                member = next(row for row in result["members"]
                              if row["path"].endswith("package.json" if kind == "node" else "METADATA"))
                read = c.inspect_acquired({**{k: v for k, v in request.items() if k != "after"},
                                           "member": member["path"], "offset": 0})
                self.assertIn("dependency", read["text"])
                self.assertEqual(read["member_sha256"], member["sha256"])
                primary = {k: v for k, v in request.items() if k != "archive_sha256"}
                implicit = c.inspect_acquired(primary)
                explicit = c.inspect_acquired({**primary, "archive_sha256": c.job["result"]["primary"]["sha256"]})
                self.assertEqual(implicit, explicit)
                self.assertEqual(explicit["scope"], "acquired_primary_archive_only")
                self.assertEqual(recovery.inventory(case.staging), before)
                self.assertEqual(len(case.calls), calls)
                self.assertEqual(list((c.workspace / "tmp").iterdir()), [])

    def test_native_payload_is_inventory_evidence_not_execution_or_text(self):
        binary = b"\x7fELF\x00inert-native-payload"
        source = b"throw new Error('dependency must never execute');\n"
        _, c, request, _ = self.dependency(payload={"build/Release/addon.node": binary, "index.js": source})
        result = c.inspect_acquired(request)
        found = next(row for row in result["members"] if row["path"] == "build/Release/addon.node")
        self.assertEqual(found, {"path": "build/Release/addon.node", "bytes": len(binary), "sha256": backend.digest(binary)})
        with self.assertRaisesRegex(backend.SetupError, "^acquired_member_unreadable$"):
            c.inspect_acquired({**{k: v for k, v in request.items() if k != "after"},
                                "member": found["path"], "offset": 0})

    def test_unknown_or_foreign_hash_never_enters_worker(self):
        _, c, request, _ = self.dependency()
        _, other, foreign, _ = self.dependency(payload={"index.js": b"// Different dependency bytes.\n"})
        for current, bad in ((c, {**request, "archive_sha256": "0" * 64}),
                             (c, {**request, "archive_sha256": foreign["archive_sha256"]}),
                             (other, request),
                             (c, {**request, "receipt_sha256": foreign["receipt_sha256"]})):
            with patch.object(current.runner, "run_compose", side_effect=AssertionError("worker entered")) as run:
                with self.assertRaisesRegex(backend.SetupError, "^acquired_inspection_unavailable$"):
                    current.inspect_acquired(bad)
                run.assert_not_called()
            self.assertFalse(current.operation_lock.locked())

    def test_selector_is_digest_only_and_request_shapes_remain_exclusive(self):
        _, c, request, _ = self.dependency()
        bad_values = [None, True, 1, [], "../archive", "/etc/passwd", "a" * 63, "A" * 64, "a" * 65]
        bad_requests = [{**request, "archive_sha256": value} for value in bad_values]
        bad_requests += [{**request, "filename": "dependency.tgz"}, {**request, "member": "package.json", "offset": 0}]
        with patch.object(c.runner, "run_compose", side_effect=AssertionError("worker entered")) as run:
            for bad in bad_requests:
                with self.subTest(bad=bad), self.assertRaises(backend.SetupError):
                    c.inspect_acquired(bad)
            run.assert_not_called()

    def test_changed_dependency_or_symlink_refuses_before_worker(self):
        for change in ("bytes", "leaf-link", "parent-link"):
            with self.subTest(change=change):
                case, c, request, archive = self.dependency()
                if change == "bytes":
                    archive.write_bytes(archive.read_bytes() + b"changed")
                elif change == "leaf-link":
                    retained = case.root / "same-bytes.tgz"
                    retained.write_bytes(archive.read_bytes())
                    archive.unlink()
                    archive.symlink_to(retained)
                else:
                    retained = case.root / "same-closure"
                    archive.parent.rename(retained)
                    archive.parent.symlink_to(retained, target_is_directory=True)
                with patch.object(c.runner, "run_compose", side_effect=AssertionError("worker entered")) as run:
                    with self.assertRaisesRegex(backend.SetupError, "^acquired_inspection_unavailable$"):
                        c.inspect_acquired(request)
                    run.assert_not_called()

    def test_mutation_during_dependency_read_withholds_output(self):
        _, c, request, archive = self.dependency()
        run = c.runner.run_compose
        def changing(*args, **kwargs):
            result = run(*args, **kwargs)
            archive.write_bytes(archive.read_bytes() + b"changed")
            return result
        with patch.object(c.runner, "run_compose", side_effect=changing):
            with self.assertRaisesRegex(backend.SetupError, "^acquired_inspection_unavailable$"):
                c.inspect_acquired(request)
        self.assertFalse(c.operation_lock.locked())

    def test_dependency_pages_chunks_and_size_limits(self):
        body = "x" * 32767 + "caf\u00e9\n" * 200
        payload = {"docs/%03d.txt" % n: str(n).encode() for n in range(70)}
        payload.update({"docs/long.txt": body.encode(), "docs/too-large.txt": b"x" * (1024 * 1024 + 1)})
        _, c, request, _ = self.dependency(payload=payload)
        first = c.inspect_acquired(request)
        second = c.inspect_acquired({**request, "after": first["next_after"]})
        names = [r["path"] for r in first["members"] + second["members"]]
        self.assertEqual(len(first["members"]), 64)
        self.assertEqual(names, sorted(set(names)))
        self.assertIsNone(second["next_after"])
        args = {k: v for k, v in request.items() if k != "after"}
        parts, offset = [], 0
        while offset is not None:
            result = c.inspect_acquired({**args, "member": "docs/long.txt", "offset": offset})
            parts.append(result["text"])
            offset = result["next_offset"]
            self.assertLessEqual(len(result["text"].encode()), 32768)
            self.assertEqual(result["member_sha256"], backend.digest(body.encode()))
        self.assertEqual("".join(parts), body)
        with self.assertRaisesRegex(backend.SetupError, "^acquired_member_unreadable$"):
            c.inspect_acquired({**args, "member": "docs/too-large.txt", "offset": 0})
        for member in ("../private", "/etc/passwd", "docs/../package.json"):
            with self.assertRaisesRegex(backend.SetupError, "^acquired_inspection_unavailable$"):
                c.inspect_acquired({**args, "member": member, "offset": 0})

    def test_worker_rejects_malicious_dependency_archives_and_wrong_digest(self):
        case, c, _, _ = self.dependency()
        baseline = [("package/package.json", b'{"name":"dependency","version":"1.0.0"}', tarfile.REGTYPE, "")]
        additions = [("package/../escape", b"private", tarfile.REGTYPE, ""),
                     ("package/link", b"", tarfile.SYMTYPE, "/private"),
                     ("package/link", b"", tarfile.LNKTYPE, "package/package.json")]
        for entry in additions + [None]:
            raw = node_tests.W.tarball(baseline + ([entry] if entry else []))
            artifact = case.root / "malformed.tgz"
            artifact.write_bytes(raw)
            settings = {"installer": str(case.installer), "installer_sha256": backend.digest(case.installer.read_bytes()),
                        "artifact": str(artifact), "archive_sha256": backend.digest(raw) if entry else "0" * 64,
                        "archive_role": "dependency", "profile": "node", "filename": "dependency.tgz", "selection": {"after": None}}
            result = subprocess.run([sys.executable, "-I", "-B", str(c.workspace / "acquired-inspection.py")],
                                    input=backend.encoded(settings), capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 2)
            self.assertEqual(json.loads(result.stdout), {"error": "acquired_inspection_unavailable"})

    def test_worker_response_cannot_change_archive_identity_or_scope(self):
        _, c, request, _ = self.dependency()
        run = c.runner.run_compose
        for key, wrong in (("scope", "acquired_primary_archive_only"), ("archive_sha256", "0" * 64),
                           ("filename", "unrelated.tgz")):
            def changing(*args, **kwargs):
                code, raw = run(*args, **kwargs)
                value = json.loads(raw)
                value[key] = wrong
                return code, backend.encoded(value)
            with self.subTest(key=key), patch.object(c.runner, "run_compose", side_effect=changing):
                with self.assertRaisesRegex(backend.SetupError, "^acquired_inspection_unavailable$"):
                    c.inspect_acquired(request)


class RecoveredDependencyTests(DependencyFixture, RecoveryFixture, unittest.TestCase):
    def test_reconciled_dependency_read_does_not_restore_install_authority(self):
        for kind in ("node", "python"):
            with self.subTest(kind=kind):
                case, original, request, _ = self.dependency(kind)
                current = self.recovery(case, original)
                historical = copy.deepcopy(current.job)
                with self.assertRaisesRegex(backend.SetupError, "^acquired_inspection_unavailable$"):
                    current.inspect_acquired(request)
                current.reconcile({"job_id": request["job_id"]})
                result = current.inspect_acquired(request)
                self.assertEqual(result["scope"], "acquired_dependency_archive_only")
                self.assertEqual(result["archive_sha256"], request["archive_sha256"])
                self.assertEqual(current.job, historical)
                self.assertEqual(current.job["status"], "unconfirmed")
                with self.assertRaisesRegex(backend.SetupError, "^recovery_read_only$"):
                    current.admit_setup()


class McpDependencyTests(DependencyFixture, unittest.IsolatedAsyncioTestCase):
    async def test_actual_mcp_schema_and_dependency_read(self):
        from mcp import Client
        _, c, request, _ = self.dependency()
        owner = adapter.SetupAdapter(c)
        try:
            async with Client(adapter.create_server(owner)) as client:
                tool = next(t for t in (await client.list_tools()).tools if t.name == "setup_inspect_acquired")
                self.assertIn("archive_sha256", tool.input_schema["properties"])
                result = await client.call_tool("setup_inspect_acquired", request)
                self.assertFalse(result.is_error, result)
                self.assertEqual(result.structured_content["archive_sha256"], request["archive_sha256"])
                self.assertEqual(result.structured_content["scope"], "acquired_dependency_archive_only")
                unknown = await client.call_tool("setup_inspect_acquired", {**request, "archive_sha256": "0" * 64})
                self.assertTrue(unknown.is_error)
                self.assertEqual(len(c.jobs), 1)
        finally:
            await owner.aclose()


if __name__ == "__main__":
    unittest.main()
