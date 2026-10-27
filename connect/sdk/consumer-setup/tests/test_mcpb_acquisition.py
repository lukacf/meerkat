"""MCPB acquisition retains exact bytes without parsing or executing the bundle."""
from __future__ import annotations

import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[3]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


M = load("mcpb_acquisition_test_owner", ROOT / "sdk/consumer-setup/mcpb_acquisition.py")
B = load("mcpb_acquisition_test_backend", ROOT / "sdk/consumer-setup/backend.py")


class AcquisitionFixture:
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="toolkit-mcpb-acquisition-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.staging, self.workspace = self.root / "staging", self.root / "workspace"
        self.staging.mkdir()
        self.workspace.mkdir()
        (self.workspace / "acquisitions").mkdir()
        self.python = Path(sys.executable).resolve()
        self.profile_path = self.root / "profile.json"
        self.profile = {"schema_version": 1, "scope": M.SCOPE,
                        "python": {"path": str(self.python), "sha256": M.sha(self.python.read_bytes())},
                        "allowed_origins": ["https://source.example"],
                        "allowed_redirect_origins": ["https://assets.example"]}
        self.io = SimpleNamespace(Error=B.SetupError, read_file=B.read_file, write_new=B.write_new,
                                  canonical_directory=B.canonical_directory, decode=B.decode, keys=B.keys)
        self.payload = b"deliberately not a ZIP: acquisition does not own bundle meaning"
        self.request = {"request_id": "a" * 32, "url": "https://source.example/releases/package.mcpb",
                        "artifact_sha256": M.sha(self.payload)}
        self.job = self.workspace / "acquisitions" / ("b" * 32)
        self.job.mkdir()
        (self.job / "request.json").write_bytes(M.wire(self.request))
        self.calls = []

    def owner(self):
        self.profile_path.write_bytes(M.wire(self.profile))
        owner = M.McpbAcquisition(self.profile_path, M.sha(self.profile_path.read_bytes()), self.staging, io=self.io)
        owner.bind(self.workspace)
        return owner

    def report(self):
        return {"primary": {"url": self.request["url"], "sha256": M.sha(self.payload),
                            "byte_length": len(self.payload), "final_origin": "https://assets.example",
                            "redirect_count": 1}}

    def runner(self, argv, **kwargs):
        self.calls.append((argv, kwargs))
        (self.job / "artifact.mcpb").write_bytes(self.payload)
        (self.job / "report.json").write_bytes(M.wire(self.report()))
        return 0, b"untrusted output must not be projected"

    def execute(self, owner=None, runner=None, stop=lambda: None):
        return (owner or self.owner()).run(self.request, self.job, runner or self.runner, stop)

    def record(self, owner):
        return {"schema_version": 1, "kind": B.ACQUISITION_REQUEST, "family": "selected-mcpb",
                "workspace": str(self.workspace), "staging_root": str(self.staging), "job_id": self.job.name,
                "arguments": self.request,
                "host": {"profile_sha256": owner.profile_sha,
                         "assets": {name: M.sha(raw) for name, raw in owner.frozen.items()},
                         "selected_profile_sha256": "c" * 64}}

    def assert_unpublished(self):
        self.assertFalse((self.staging / self.job.name / "acquisition.json").exists())


class McpbAcquisitionTests(AcquisitionFixture, unittest.TestCase):
    def test_exact_bytes_and_readback_without_zip_or_runtime_execution(self):
        owner = self.owner()
        result = self.execute(owner)
        self.assertEqual(result["scope"], "retained_mcpb_artifact_bytes")
        self.assertEqual(result["files"], {"artifact.mcpb": M.sha(self.payload)})
        self.assertEqual(result["primary"], self.report()["primary"])
        self.assertEqual(set(owner.frozen), {"profile.json", "worker.py"})
        self.assertEqual(self.calls[0][0][1:4], ["-I", "-S", "-B"])
        self.assertEqual(self.calls[0][0][4], str(owner.assets / "worker.py"))
        self.assertLessEqual(self.calls[0][1]["inspection_timeout"], 120)
        self.assertNotIn("untrusted", json.dumps(result))
        destination = self.staging / self.job.name
        self.assertEqual((destination / "artifact.mcpb").read_bytes(), self.payload)
        self.assertEqual(sorted(p.name for p in destination.iterdir()), ["acquisition.json", "artifact.mcpb"])
        with patch.object(subprocess, "Popen", side_effect=AssertionError("readback executed")):
            self.assertEqual(M.readback(self.io, self.record(owner), self.job), result)

    def test_invalid_profile_shapes_and_origins_are_rejected(self):
        mutations = [lambda p: p.update(schema_version=True), lambda p: p.update(scope="other"),
                     lambda p: p.update(staging_root=str(self.staging)), lambda p: p.update(allowed_origins=[]),
                     lambda p: p.update(allowed_origins=["https://source.example/"]),
                     lambda p: p.update(allowed_origins=["https://user@source.example"]),
                     lambda p: p.update(allowed_origins=["https://source.example:443"]),
                     lambda p: p.update(allowed_origins=["https://*.example"]),
                     lambda p: p.update(allowed_origins=["https://source.example"] * 2),
                     lambda p: p.update(allowed_redirect_origins="https://assets.example"),
                     lambda p: p["python"].update(sha256="0" * 64),
                     lambda p: p["python"].update(path="relative/python")]
        for mutate in mutations:
            value = copy.deepcopy(self.profile)
            mutate(value)
            self.profile_path.write_bytes(M.wire(value))
            with self.subTest(value=value), self.assertRaisesRegex(B.SetupError, "invalid_mcpb_acquisition_profile"):
                M.McpbAcquisition(self.profile_path, M.sha(self.profile_path.read_bytes()), self.staging, io=self.io)
        self.assertEqual(list(self.staging.iterdir()), [])

    def test_origin_canonicalization_supports_explicit_nondefault_port_and_ipv6(self):
        for value in ("https://source.example:8443", "https://[2001:db8::1]", "https://[::1]:8443"):
            self.assertEqual(M.origin(value), value)
        for value in ("https://SOURCE.example", "https://source.example:", "https://source.example:0",
                      "https://source.example:65536", "https://[2001:0db8::1]", "http://source.example",
                      "https://source.example?", "https://source.example#", "https://source.example."):
            with self.subTest(value=value), self.assertRaises(ValueError):
                M.origin(value)

    def test_argument_admission_before_any_worker(self):
        owner = self.owner()
        changes = [{"url": "https://other.example/a"}, {"url": "https://source.example/a?token=secret"},
                   {"url": "https://source.example/a?"}, {"url": "https://source.example/a#"},
                   {"url": "http://source.example/a"}, {"url": "https://user:secret@source.example/a"},
                   {"url": "https://source.example/a\\b"}, {"url": "https://source.example/a%0ab"},
                   {"url": "https://source.example/a%xx"}, {"url": "https://source.example/a\nb"},
                   {"artifact_sha256": "A" * 64}, {"request_id": "bad"}, {"headers": {}}]
        for change in changes:
            request = {**self.request, **change}
            with self.subTest(change=change), self.assertRaisesRegex(B.SetupError, "invalid_mcpb_acquisition_arguments"):
                owner.run(request, self.job, self.runner, lambda: None)
        self.assertEqual(self.calls, [])
        self.assert_unpublished()

    def test_initial_and_redirect_url_limits_are_distinct(self):
        prefix = "https://source.example/"
        initial = prefix + "a" * (4096 - len(prefix))
        self.assertEqual(M.artifact_url(initial), "https://source.example")
        with self.assertRaises(ValueError):
            M.artifact_url(initial + "a")
        redirect = initial + "?token=" + "a" * (8192 - len(initial) - 7)
        self.assertEqual(M.artifact_url(redirect, redirect=True), "https://source.example")
        with self.assertRaises(ValueError):
            M.artifact_url(redirect + "a", redirect=True)

    def test_profile_or_worker_mutation_prevents_entry(self):
        owner = self.owner()
        for path in (self.profile_path, owner.assets / "worker.py"):
            original = path.read_bytes()
            path.chmod(0o600)
            path.write_bytes(original + b" ")
            with self.subTest(path=path), self.assertRaisesRegex(B.SetupError, "mcpb_acquisition_host_changed"):
                self.execute(owner)
            path.write_bytes(original)
        self.assertEqual(self.calls, [])

    def test_runtime_mutation_prevents_entry(self):
        runtime = self.root / "python"
        runtime.write_bytes(b"original trusted inert executable")
        runtime.chmod(0o700)
        self.profile["python"] = {"path": str(runtime), "sha256": M.sha(runtime.read_bytes())}
        owner = self.owner()
        runtime.write_bytes(b"changed")
        with self.assertRaisesRegex(B.SetupError, "mcpb_acquisition_host_changed"):
            self.execute(owner)
        self.assertEqual(self.calls, [])

    def test_changed_profile_after_worker_has_no_success_receipt(self):
        owner = self.owner()
        def changed(argv, **kwargs):
            result = self.runner(argv, **kwargs)
            self.profile_path.write_bytes(b"changed")
            return result
        with self.assertRaisesRegex(B.SetupError, "mcpb_acquisition_host_changed"):
            self.execute(owner, changed)
        self.assert_unpublished()

    def test_partial_or_failed_worker_has_no_success_receipt(self):
        owner = self.owner()
        def failed(argv, **kwargs):
            (self.job / "artifact.mcpb").write_bytes(b"partial")
            return 75, b"signed redirect token must not escape"
        with self.assertRaisesRegex(B.SetupError, "^mcpb_acquisition_download_failed$"):
            self.execute(owner, failed)
        self.assertEqual((self.job / "artifact.mcpb").read_bytes(), b"partial")
        self.assert_unpublished()

    def test_inconsistent_successful_worker_result_refused(self):
        owner = self.owner()
        def bad(argv, **kwargs):
            result = self.runner(argv, **kwargs)
            (self.job / "artifact.mcpb").write_bytes(b"wrong bytes")
            return result
        with self.assertRaisesRegex(B.SetupError, "mcpb_acquisition_unconfirmed"):
            self.execute(owner, bad)
        self.assert_unpublished()

    def test_sibling_job_or_existing_destination_never_enters_worker(self):
        owner = self.owner()
        (self.staging / self.job.name).mkdir()
        with self.assertRaises(B.SetupError):
            self.execute(owner)
        with self.assertRaises(B.SetupError):
            owner.run(self.request, self.root / self.job.name, self.runner, lambda: None)
        self.assertEqual(self.calls, [])

    def test_changed_request_after_worker_refused(self):
        owner = self.owner()
        def bad(argv, **kwargs):
            result = self.runner(argv, **kwargs)
            (self.job / "request.json").write_bytes(b"{}")
            return result
        with self.assertRaisesRegex(B.SetupError, "mcpb_acquisition_unconfirmed"):
            self.execute(owner, bad)
        self.assert_unpublished()

    def test_stop_after_worker_refuses_publication(self):
        owner = self.owner()
        entered = []
        def worker(argv, **kwargs):
            result = self.runner(argv, **kwargs)
            entered.append(True)
            return result
        def stop():
            if entered:
                raise B.SetupError("operation_stopped", 409)
        with self.assertRaisesRegex(B.SetupError, "operation_stopped"):
            self.execute(owner, worker, stop)
        self.assert_unpublished()

    def test_timeout_before_worker_refuses_entry(self):
        owner = self.owner()
        with patch.object(M.time, "monotonic", side_effect=[0, 121]):
            with self.assertRaisesRegex(B.SetupError, "mcpb_acquisition_timeout"):
                self.execute(owner)
        self.assertEqual(self.calls, [])

    def test_readback_allows_bounded_later_metadata_and_no_longer_needs_runtime(self):
        runtime = self.root / "python"
        runtime.write_bytes(b"inert trusted runtime")
        runtime.chmod(0o700)
        self.profile["python"] = {"path": str(runtime), "sha256": M.sha(runtime.read_bytes())}
        owner = self.owner()
        result = self.execute(owner)
        runtime.unlink()
        self.profile_path.unlink()
        for name in ("package.json", "activation.json", "configuration.json"):
            (self.staging / self.job.name / name).write_bytes(b"{}")
        self.assertEqual(M.readback(self.io, self.record(owner), self.job), result)

    def test_readback_refuses_tampered_artifact_receipt_worker_and_request(self):
        owner = self.owner()
        self.execute(owner)
        paths = [self.staging / self.job.name / "artifact.mcpb", self.staging / self.job.name / "acquisition.json",
                 self.job / "artifact.mcpb", self.job / "request.json", self.job / "report.json",
                 self.job / "worker.json", owner.assets / "profile.json", owner.assets / "worker.py"]
        for path in paths:
            original = path.read_bytes()
            path.chmod(0o600)
            path.write_bytes(original + b"changed")
            with self.subTest(path=path), self.assertRaisesRegex(B.SetupError, "^mcpb_acquisition_readback_unconfirmed$"):
                M.readback(self.io, self.record(owner), self.job)
            path.write_bytes(original)

    def test_readback_rejects_unknown_file_symlink_and_wrong_job(self):
        owner = self.owner()
        self.execute(owner)
        destination = self.staging / self.job.name
        extra = destination / "unexpected"
        extra.write_bytes(b"x")
        with self.assertRaises(B.SetupError):
            M.readback(self.io, self.record(owner), self.job)
        extra.unlink()
        extra.symlink_to(self.job / "artifact.mcpb")
        with self.assertRaises(B.SetupError):
            M.readback(self.io, self.record(owner), self.job)
        extra.unlink()
        record = self.record(owner)
        record["job_id"] = "d" * 32
        with self.assertRaises(B.SetupError):
            M.readback(self.io, record, self.job)

    def test_worker_failure_output_never_exposes_raw_exception(self):
        settings = self.job / "invalid-settings.json"
        settings.write_bytes(b'{"private":"signed-query=hidden", "private":0}')
        result = subprocess.run([str(self.python), "-I", "-S", "-B", str(Path(M.__file__)),
                                 "--fetch", str(settings), M.sha(settings.read_bytes())],
                                capture_output=True, timeout=10, env={})
        self.assertEqual(result.returncode, 75)
        self.assertEqual(result.stdout, b"")
        self.assertEqual(result.stderr, b"mcpb_acquisition_worker_failed\n")


if __name__ == "__main__":
    unittest.main()
