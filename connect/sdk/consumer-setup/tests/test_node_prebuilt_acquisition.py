"""Native payload composition exercises the real archive owner with inert I/O."""
from __future__ import annotations

import copy
import io
import json
from pathlib import Path
import subprocess
import tarfile
import unittest
from unittest.mock import patch

import test_node_acquisition as N


M = N.load("node_prebuilt_acquisition_test_owner", N.ROOT / "sdk/consumer-setup/node_prebuilt_acquisition.py")
B = N.B


def asset(files, *, link=None):
    data = io.BytesIO()
    with tarfile.open(fileobj=data, mode="w:gz") as archive:
        for name, raw in files.items():
            member = tarfile.TarInfo(name)
            member.size = len(raw)
            archive.addfile(member, io.BytesIO(raw))
        if link:
            member = tarfile.TarInfo(link)
            member.type = tarfile.SYMTYPE
            member.linkname = "/private/should-not-read"
            archive.addfile(member)
    return data.getvalue()


class NativeFixture:
    def setUp(self):
        N.NodeAcquisitionTests.setUp(self)
        self.node_owner = N.NodeAcquisitionTests.owner(self)
        self.source_job, self.source_request = self.job, copy.deepcopy(self.request)
        self.source_result = self.node_owner.run(self.request, self.job,
            lambda *args, **kwargs: N.NodeAcquisitionTests.runner(self, *args, **kwargs), lambda: None)
        self.source_record = {"schema_version": 1, "kind": B.ACQUISITION_REQUEST, "family": "selected-node",
            "workspace": str(self.workspace), "staging_root": str(self.staging), "job_id": self.source_job.name,
            "arguments": self.source_request, "host": {"profile_sha256": self.node_owner.profile_sha,
                "assets": {key: M.sha(self.node_owner.frozen[key]) for key in
                    ("profile.json", "worker.py", "installer.py", "npm.tgz")},
                "selected_profile_sha256": "c" * 64}}
        self.selected_node = self.root / "selected-node"
        self.selected_node.write_bytes(b"different selected consumer executable, never run by fixture\n")
        self.selected_node.chmod(0o700)
        self.target = {"node_sha256": M.sha(self.selected_node.read_bytes()), "node_version": "v24.19.0",
                       "node_abi": "137", "platform": "linux", "arch": "x64",
                       "libc": {"family": "glibc", "version": "2.39"}}
        self.prebuilt_profile_path = self.root / "prebuilt-profile.json"
        self.prebuilt_profile = {"schema_version": 1, "scope": M.SCOPE,
            "python": {"path": str(self.python), "sha256": M.sha(self.python.read_bytes())},
            "allowed_origins": ["https://source.example"], "allowed_redirect_origins": ["https://assets.example"],
            "target": self.target}
        self.binary = b"\x7fELF\x00inert-addon-never-executed"
        self.payload = asset({"build/Release/addon.node": self.binary})
        self.request = {"request_id": "d" * 32,
            "source": {"job_id": self.source_job.name, "receipt_sha256": self.source_result["receipt_sha256"]},
            "target": self.target, "payloads": [{"package_location": "node_modules/dependency",
                "package_archive_sha256": M.sha(self.dependency), "asset_id": "native-asset-" + M.sha(self.payload),
                "url": "https://source.example/exact-release.tar.gz", "files": [{"member": "build/Release/addon.node",
                    "destination": "build/Release/addon.node"}]}]}
        self.job = self.workspace / "acquisitions" / ("e" * 32)
        self.job.mkdir()
        self.calls, self.source_reads = [], 0

    def owner(self):
        self.prebuilt_profile_path.write_bytes(M.wire(self.prebuilt_profile))
        owner = M.NodePrebuiltAcquisition(self.prebuilt_profile_path, M.sha(self.prebuilt_profile_path.read_bytes()),
            self.staging, io=self.io, node_owner=self.node_owner, source_reader=self.source_reader,
            selected_node=self.selected_node)
        owner.bind(self.workspace)
        return owner

    def source_reader(self, reference):
        self.source_reads += 1
        if reference != {"job_id": self.source_job.name, "receipt_sha256": self.source_result["receipt_sha256"]}:
            raise B.SetupError("selected_acquired_source_unavailable", 409)
        return N.M.readback(self.io, self.source_record, self.source_job)

    def runner(self, argv, **kwargs):
        self.calls.append((argv, kwargs))
        if argv[0] == str(self.selected_node):
            self.assertEqual(argv[1], "-p")
            return 0, M.wire({key: value for key, value in self.target.items() if key != "node_sha256"})
        self.assertEqual(argv[0], str(self.python))
        self.assertEqual(argv[1:4], ["-I", "-S", "-B"])
        self.assertEqual(argv[-3], "--fetch")
        raw = Path(argv[-2]).read_bytes()
        self.assertEqual(M.sha(raw), argv[-1])
        settings = json.loads(raw)
        directory = Path(settings["directory"])
        (directory / "artifact.mcpb").write_bytes(self.payload)
        (directory / "report.json").write_bytes(M.wire({"primary": {
            "url": settings["request"]["url"], "sha256": M.sha(self.payload), "byte_length": len(self.payload),
            "final_origin": "https://assets.example", "redirect_count": 1}}))
        return 0, b"private response must not appear in receipt"

    def execute(self, owner=None, runner=None, stop=lambda: None):
        (self.job / "request.json").write_bytes(M.wire(self.request))
        return (owner or self.owner()).run(self.request, self.job, runner or self.runner, stop)

    def record(self, owner):
        return {"schema_version": 1, "kind": B.ACQUISITION_REQUEST, "family": "selected-node-prebuilt",
            "workspace": str(self.workspace), "staging_root": str(self.staging), "job_id": self.job.name,
            "arguments": copy.deepcopy(self.request), "host": {"profile_sha256": owner.profile_sha,
                "assets": {name: M.sha(raw) for name, raw in owner.frozen.items()},
                "selected_profile_sha256": "c" * 64}}

    def assert_unpublished(self):
        self.assertFalse((self.staging / self.job.name / "acquisition.json").exists())


class NodePrebuiltAcquisitionTests(NativeFixture, unittest.TestCase):
    def test_derived_receipt_preserves_source_and_readback_is_inert(self):
        owner = self.owner()
        source_path = self.staging / self.source_job.name
        before = {str(path.relative_to(source_path)): path.read_bytes()
                  for path in source_path.rglob("*") if path.is_file()}
        result = self.execute(owner)
        self.assertEqual(result["scope"], M.RESULT_SCOPE)
        self.assertEqual(result["source_receipt_sha256"], self.source_result["receipt_sha256"])
        self.assertEqual(result["artifacts"], self.source_result["artifacts"])
        self.assertEqual(result["native_target"], self.target)
        self.assertEqual(self.calls[0][0][0], str(self.selected_node))
        self.assertNotEqual(self.selected_node, self.node_owner.node)
        self.assertEqual(len(self.calls), 2)
        self.assertEqual(self.source_reads, 3)
        self.assertTrue(all(0 < options["inspection_timeout"] <= 120 for _, options in self.calls))
        self.assertEqual(before, {str(path.relative_to(source_path)): path.read_bytes()
                                 for path in source_path.rglob("*") if path.is_file()})
        derived = self.staging / self.job.name
        self.assertEqual((derived / "closure/native-source-receipt").read_bytes(), before["acquisition.json"])
        manifest = json.loads((derived / "closure/native-payloads").read_bytes())
        self.assertEqual(manifest["payloads"][0]["files"][0]["sha256"], M.sha(self.binary))
        self.assertNotIn("sha256", self.request["payloads"][0]["files"][0])
        files = {name: (derived / name).read_bytes() for name in result["files"]}
        closure = owner.parser["npm_archive_closure"](files)
        self.assertIn("node_modules/dependency/build/Release/addon.node", closure["paths"])
        with patch.object(subprocess, "Popen", side_effect=AssertionError("readback executed")), \
                patch.object(M, "_http_namespace", side_effect=AssertionError("readback reloaded code")):
            self.assertEqual(M.readback(self.io, self.record(owner), self.job), result)
        self.assertNotIn("private response", json.dumps(result))

    def test_argument_refusals_enter_no_worker_or_source_reader(self):
        owner = self.owner()
        mutations = [lambda r: r.update(extra=True), lambda r: r["source"].update(path="/tmp/other"),
            lambda r: r["target"].update(node_abi="127"),
            lambda r: r["payloads"][0].update(url="https://other.example/a"),
            lambda r: r["payloads"][0].update(url="https://source.example/a?token=secret"),
            lambda r: r["payloads"][0].update(asset_id="../asset"),
            lambda r: r["payloads"][0].update(package_location="node_modules/../../secret"),
            lambda r: r["payloads"][0]["files"][0].update(member="../secret"),
            lambda r: r["payloads"][0]["files"][0].update(destination="node_modules/other/a.node"),
            lambda r: r["payloads"].append(copy.deepcopy(r["payloads"][0]))]
        for mutate in mutations:
            request = copy.deepcopy(self.request)
            mutate(request)
            with self.subTest(request=request), self.assertRaises(B.SetupError):
                owner.run(request, self.job, self.runner, lambda: None)
        self.assertEqual(self.calls, [])
        self.assertEqual(self.source_reads, 0)
        self.assert_unpublished()

    def test_host_profile_requires_explicit_supported_target_and_origins(self):
        mutations = [lambda p: p.update(scope="trusted_public_mcpb_acquisition"),
            lambda p: p.update(allowed_origins=[]), lambda p: p.update(allowed_origins=["https://source.example/"]),
            lambda p: p["target"].update(node_sha256="0" * 64),
            lambda p: p["target"].update(arch="arm64"),
            lambda p: p["target"].update(libc={"family": "musl", "version": "1.2"}),
            lambda p: p["python"].update(sha256="0" * 64)]
        for mutate in mutations:
            profile = copy.deepcopy(self.prebuilt_profile)
            mutate(profile)
            self.prebuilt_profile_path.write_bytes(M.wire(profile))
            with self.subTest(profile=profile), self.assertRaisesRegex(B.SetupError, "invalid_node_prebuilt_acquisition_profile"):
                M.NodePrebuiltAcquisition(self.prebuilt_profile_path, M.sha(self.prebuilt_profile_path.read_bytes()),
                    self.staging, io=self.io, node_owner=self.node_owner, source_reader=self.source_reader,
                    selected_node=self.selected_node)
        self.assertEqual(self.calls, [])
        self.assert_unpublished()

    def test_foreign_source_receipt_refuses_before_target_or_fetch(self):
        self.request["source"]["receipt_sha256"] = "0" * 64
        with self.assertRaisesRegex(B.SetupError, "selected_acquired_source_unavailable"):
            self.execute()
        self.assertEqual(self.calls, [])
        self.assert_unpublished()

    def test_exact_selected_runtime_mismatch_refuses_before_download(self):
        def wrong(argv, **kwargs):
            code, raw = self.runner(argv, **kwargs)
            observed = json.loads(raw)
            observed["node_abi"] = "127"
            return code, M.wire(observed)
        with self.assertRaisesRegex(B.SetupError, "node_prebuilt_runtime_unsupported"):
            self.execute(runner=wrong)
        self.assertEqual(len(self.calls), 1)
        self.assert_unpublished()

    def test_source_changes_during_download_never_publish_derived_success(self):
        def mutate(argv, **kwargs):
            result = self.runner(argv, **kwargs)
            if argv[0] == str(self.python):
                (self.staging / self.source_job.name / "artifact.tgz").write_bytes(b"changed")
            return result
        with self.assertRaises(B.SetupError):
            self.execute(runner=mutate)
        self.assert_unpublished()

    def test_host_tool_change_during_download_refuses(self):
        def mutate(argv, **kwargs):
            result = self.runner(argv, **kwargs)
            if argv[0] == str(self.python):
                self.selected_node.write_bytes(b"changed selected runtime")
            return result
        with self.assertRaisesRegex(B.SetupError, "node_prebuilt_acquisition_host_changed"):
            self.execute(runner=mutate)
        self.assert_unpublished()

    def test_deadline_after_download_prevents_receipt(self):
        now = [0.0]
        def slow(argv, **kwargs):
            result = self.runner(argv, **kwargs)
            if argv[0] == str(self.python):
                now[0] = 121.0
            return result
        with patch.object(M.time, "monotonic", side_effect=lambda: now[0]):
            with self.assertRaisesRegex(B.SetupError, "node_prebuilt_acquisition_timeout"):
                self.execute(runner=slow)
        self.assert_unpublished()

    def test_stop_after_download_prevents_receipt(self):
        stopped = [False]
        def run(argv, **kwargs):
            result = self.runner(argv, **kwargs)
            if argv[0] == str(self.python):
                stopped[0] = True
            return result
        def stop():
            if stopped[0]:
                raise B.SetupError("operation_stopped", 409)
        with self.assertRaisesRegex(B.SetupError, "operation_stopped"):
            self.execute(runner=run, stop=stop)
        self.assert_unpublished()

    def test_stop_prevents_runtime_and_fetch(self):
        def stop():
            raise B.SetupError("operation_stopped", 409)
        with self.assertRaisesRegex(B.SetupError, "operation_stopped"):
            self.execute(stop=stop)
        self.assertEqual(self.calls, [])
        self.assert_unpublished()

    def test_archive_links_refuse(self):
        owner = self.owner()
        self.payload = asset({"build/Release/addon.node": self.binary}, link="dangerous")
        self.request["payloads"][0]["asset_id"] = "native-asset-" + M.sha(self.payload)
        with self.assertRaisesRegex(B.SetupError, "node_prebuilt_acquisition_unconfirmed"):
            self.execute(owner)
        self.assert_unpublished()

    def test_wrong_download_digest_refuses(self):
        self.request["payloads"][0]["asset_id"] = "native-asset-" + "0" * 64
        with self.assertRaisesRegex(B.SetupError, "node_prebuilt_acquisition_unconfirmed"):
            self.execute()
        self.assert_unpublished()

    def test_source_archive_mapping_and_payload_completeness_are_validated(self):
        self.request["payloads"][0]["files"][0]["destination"] = "index.js"
        self.request["payloads"][0]["files"].append({"member": "extra.node", "destination": "extra.node"})
        with self.assertRaisesRegex(B.SetupError, "node_prebuilt_acquisition_unconfirmed"):
            self.execute()
        self.assert_unpublished()

    def test_late_worker_proof_mutation_refuses_success(self):
        owner = self.owner()
        original_reader = owner.source_reader
        reads = [0]
        def read(reference):
            result = original_reader(reference)
            reads[0] += 1
            if reads[0] == 2:
                report = self.job / self.request["payloads"][0]["asset_id"] / "report.json"
                report.write_bytes(b"{}")
            return result
        owner.source_reader = read
        with self.assertRaisesRegex(B.SetupError, "node_prebuilt_acquisition_unconfirmed"):
            self.execute(owner)
        self.assert_unpublished()

    def test_failed_worker_retains_no_success_and_no_diagnostics(self):
        def fail(argv, **kwargs):
            if argv[0] == str(self.python):
                return 75, b"signed redirect URL and other private diagnostics"
            return self.runner(argv, **kwargs)
        with self.assertRaisesRegex(B.SetupError, "^node_prebuilt_download_failed$"):
            self.execute(runner=fail)
        self.assert_unpublished()

    def test_readback_rejects_payload_mutation_and_symlink_without_execution(self):
        owner = self.owner()
        self.execute(owner)
        path = self.staging / self.job.name / ("closure/" + self.request["payloads"][0]["asset_id"])
        original = path.read_bytes()
        for change in ("bytes", "link"):
            if change == "bytes":
                path.write_bytes(b"changed")
            else:
                path.unlink()
                path.symlink_to(self.root / "missing")
            with self.subTest(change=change), patch.object(subprocess, "Popen", side_effect=AssertionError("executed")):
                with self.assertRaisesRegex(B.SetupError, "node_prebuilt_acquisition_readback_unconfirmed"):
                    M.readback(self.io, self.record(owner), self.job)
            path.unlink()
            path.write_bytes(original)

    def test_readback_does_not_regrant_historical_source_or_probe_runtime(self):
        owner = self.owner()
        result = self.execute(owner)
        self.selected_node.write_bytes(b"current executable changed")
        with patch.object(owner, "source_reader", side_effect=AssertionError("historical authority consulted")), \
                patch.object(subprocess, "Popen", side_effect=AssertionError("runtime executed")):
            self.assertEqual(M.readback(self.io, self.record(owner), self.job), result)
        self.assertNotIn("installed", result)

    def test_readback_rederives_member_hash_even_if_receipt_was_rewritten(self):
        owner = self.owner()
        self.execute(owner)
        destination = self.staging / self.job.name
        manifest_path = destination / "closure/native-payloads"
        manifest = json.loads(manifest_path.read_bytes())
        manifest["payloads"][0]["files"][0]["sha256"] = "0" * 64
        manifest_path.write_bytes(M.wire(manifest))
        receipt_path = destination / "acquisition.json"
        receipt = json.loads(receipt_path.read_bytes())
        receipt["files"]["closure/native-payloads"] = M.sha(manifest_path.read_bytes())
        receipt_path.write_bytes(M.wire(receipt))
        with self.assertRaisesRegex(B.SetupError, "node_prebuilt_acquisition_readback_unconfirmed"):
            M.readback(self.io, self.record(owner), self.job)

    def test_retained_file_limit_stops_before_later_reads(self):
        owner = self.owner()
        result = self.execute(owner)
        destination = self.staging / self.job.name
        receipt = (destination / "closure/native-source-receipt").read_bytes()
        source = json.loads(receipt)
        names = list(source["files"])
        first = destination / names[0]
        original_read, reads = self.io.read_file, []
        def bounded_read(root, name, limit):
            if root == destination and name in source["files"]:
                reads.append(name)
            return original_read(root, name, limit)
        # Fit the first base file but not the second. The remaining closure
        # must never be loaded even though every individual input is bounded.
        limit = len(receipt) + (destination / "closure/native-payloads").stat().st_size + first.stat().st_size
        with patch.object(self.io, "read_file", side_effect=bounded_read), patch.object(M, "TOTAL_LIMIT", limit):
            with self.assertRaisesRegex(B.SetupError, "node_prebuilt_acquisition_readback_unconfirmed"):
                M.readback(self.io, self.record(owner), self.job)
        self.assertEqual(reads, names[:2])
        self.assertEqual(result["scope"], M.RESULT_SCOPE)

    def test_run_retained_source_limit_stops_before_later_reads(self):
        owner = self.owner()
        source_root = self.staging / self.source_job.name
        receipt = (source_root / "acquisition.json").read_bytes()
        source = json.loads(receipt)
        names = list(source["files"])
        original_read, reads = self.io.read_file, []
        def bounded_read(root, name, limit):
            if root == source_root and name in source["files"]:
                reads.append(name)
            return original_read(root, name, limit)
        limit = len(receipt) + (source_root / names[0]).stat().st_size
        # Isolate this owner's cumulative bound from the original owner's
        # independently tested same-job reobservation.
        with patch.object(owner, "source_reader", return_value=self.source_result), \
                patch.object(self.io, "read_file", side_effect=bounded_read), patch.object(M, "TOTAL_LIMIT", limit):
            with self.assertRaisesRegex(B.SetupError, "node_prebuilt_acquisition_unconfirmed"):
                self.execute(owner)
        self.assertEqual(reads, names[:2])
        self.assertEqual(self.calls, [])
        self.assert_unpublished()


if __name__ == "__main__":
    unittest.main()
