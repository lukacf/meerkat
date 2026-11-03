"""Acquisition custody and real wheel-owner checks; no public network access."""
from __future__ import annotations

import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile


ROOT = Path(__file__).resolve().parents[3]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


M = load("python_acquisition_test_owner", ROOT / "sdk/consumer-setup/python_acquisition.py")
B = load("python_acquisition_test_backend", ROOT / "sdk/consumer-setup/backend.py")
W = load("python_acquisition_test_wheels", ROOT / "scripts/test_selected_python_install.py")


class AcquisitionTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix="toolkit-acquisition-unit-")
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve()
        self.staging = self.root / "staging"
        self.staging.mkdir()
        self.workspace = self.root / "workspace"
        self.workspace.mkdir()
        (self.workspace / "acquisitions").mkdir()
        self.python = Path(sys.executable).resolve()
        self.pip = self.root / "pip.pyz"
        self.pip.write_bytes(b"trusted inert pip fixture")
        self.installer = ROOT / "scripts/install-library-consumer.py"
        self.profile_path = self.root / "profile.json"
        self.profile = {"schema_version": 1, "scope": M.SCOPE,
                        "python_sha256": M.sha(self.python.read_bytes()),
                        "pip": {"path": str(self.pip), "sha256": M.sha(self.pip.read_bytes())}}
        self.profile_path.write_bytes(M.wire(self.profile))
        self.io = SimpleNamespace(Error=B.SetupError, read_file=B.read_file, write_new=B.write_new,
                                  canonical_directory=B.canonical_directory, decode=B.decode, keys=B.keys)
        self.request = {"request_id": "a" * 32, "distribution": "outside-server", "version": "1.0"}
        self.job = self.workspace / "acquisitions" / ("b" * 32)
        self.job.mkdir()
        (self.job / "request.json").write_bytes(M.wire(self.request))
        self.report = {"version": "1", "pip_version": "fixture", "install": []}
        self.payloads = {}
        self.add_wheel("outside_server", requested=True)
        self.add_wheel("dependency")
        self.calls, self.fetches = [], []

    def add_wheel(self, package, requested=False, **kwargs):
        filename, raw, _ = W.wheel(package=package, **kwargs)
        url = "https://files.pythonhosted.org/packages/ab/cd/fixture/" + filename
        self.payloads[url] = raw
        self.report["install"].append({"is_direct": False, "is_yanked": False, "requested": requested,
            "download_info": {"url": url, "archive_info": {"hashes": {"sha256": M.sha(raw)}}},
            "metadata": {"name": package, "version": "1.0"}})

    def owner(self):
        self.profile_path.write_bytes(M.wire(self.profile))
        owner = M.PythonAcquisition(self.io, self.profile_path, M.sha(self.profile_path.read_bytes()),
                                   self.python, self.staging, self.installer, M.sha(self.installer.read_bytes()))
        owner.bind(self.workspace)
        return owner

    def runner(self, argv, **kwargs):
        self.calls.append((argv, kwargs))
        (self.job / "report.json").write_bytes(M.wire(self.report))
        (self.job / "runtime.json").write_bytes(M.wire({"target": ["cpython", 3, 11, "darwin", "arm64"],
                                                       "stdlib": sorted(sys.stdlib_module_names)}))
        return 0, b"untrusted private pip output"

    def fetch(self, row, checkpoint):
        checkpoint()
        self.fetches.append(row["url"])
        return self.payloads[row["url"]]

    def execute(self, owner=None, check_stop=lambda: None, runner=None):
        owner = owner or self.owner()
        with patch.object(owner, "fetch_retained", lambda row, checkpoint, *_: self.fetch(row, checkpoint)):
            return owner.run(self.request, self.job, runner or self.runner, check_stop)

    def assert_unpublished(self):
        self.assertFalse((self.staging / self.job.name / "acquisition.json").exists())

    def test_exact_retained_bytes_feed_the_existing_wheel_closure_owner(self):
        result = self.execute()
        dest = self.staging / self.job.name
        self.assertEqual(result["scope"], M.RESULT_SCOPE)
        self.assertEqual(result["request"], self.request)
        self.assertEqual(len(self.calls), 1)
        self.assertEqual(self.calls[0][0][1:4], ["-I", "-S", "-B"])
        self.assertLessEqual(self.calls[0][1]["inspection_timeout"], 120)
        self.assertFalse((dest / "package.json").exists())
        self.assertFalse((dest / "activation.json").exists())
        files = {name: B.read_file(dest, name, M.WHEEL_LIMIT) for name in result["files"]}
        self.assertEqual({name: M.sha(raw) for name, raw in files.items()}, result["files"])
        closure = W.M.selected_python_closure(files, {
            "artifact_filename": result["primary"]["filename"],
            "entrypoint": {"executable": "outside_server/__main__.py"}, "module": "outside_server.__main__"},
            set(sys.stdlib_module_names), W.EXTENSIONS)
        self.assertEqual(closure["versions"], {"outside-server": "1.0", "dependency": "1.0"})
        self.assertEqual(M.sha((dest / "acquisition.json").read_bytes()), result["receipt_sha256"])
        self.assertNotIn("untrusted", json.dumps(result))

    def test_invalid_profile_refused_before_any_workspace_bind(self):
        self.profile["index_url"] = "https://attacker.invalid/simple"
        with self.assertRaises(B.SetupError):
            self.owner()
        self.assertFalse((self.workspace / "python-acquisition").exists())

    def test_missing_profile_path_is_a_fixed_setup_error(self):
        with self.assertRaisesRegex(B.SetupError, "invalid_python_acquisition_profile"):
            M.PythonAcquisition(self.io, None, "0" * 64, self.python, self.staging,
                                self.installer, M.sha(self.installer.read_bytes()))

    def test_arguments_accept_only_exact_named_choices(self):
        owner = self.owner()
        for values in ({"distribution": "outside_server"}, {"distribution": "x @ https://evil.invalid/a.whl"},
                       {"version": ">=1"}, {"version": "1.*"}, {"version": "1\n--find-links=x"},
                       {"request_id": "../stage"}, {"extra": "not allowed"}):
            with self.subTest(values=values), self.assertRaises(B.SetupError):
                owner.arguments({**self.request, **values})
        result = owner.arguments(self.request)
        result["version"] = "2"
        self.assertEqual(self.request["version"], "1.0")

    def test_changed_trusted_input_refused_before_launch(self):
        owner = self.owner()
        self.pip.write_bytes(b"changed")
        with self.assertRaisesRegex(B.SetupError, "python_acquisition_host_changed"):
            self.execute(owner)
        self.assertEqual(self.calls, [])
        self.assert_unpublished()

    def test_existing_staging_child_is_never_reused(self):
        dest = self.staging / self.job.name
        dest.mkdir()
        (dest / "keep").write_bytes(b"existing evidence")
        with self.assertRaises(B.SetupError):
            self.execute()
        self.assertEqual(self.calls, [])
        self.assertEqual((dest / "keep").read_bytes(), b"existing evidence")

    def test_report_rejected_before_download_when_root_or_origin_differs(self):
        owner = self.owner()
        changes = [lambda r: r["install"][0]["metadata"].update(version="2"),
                   lambda r: r["install"][0].update(requested=False),
                   lambda r: r["install"][1].update(requested=True),
                   lambda r: r["install"][0].update(is_direct=True),
                   lambda r: r["install"][0].update(is_yanked=True),
                   lambda r: r["install"][0]["download_info"].update(url="https://evil.invalid/p.whl"),
                   lambda r: r["install"].append(copy.deepcopy(r["install"][0])),
                   lambda r: r["install"][0]["download_info"]["archive_info"]["hashes"].update(sha256="bad")]
        for change in changes:
            report = copy.deepcopy(self.report)
            change(report)
            with self.subTest(change=change), self.assertRaises((ValueError, B.SetupError)):
                owner.parse_report(M.wire(report), self.request)
        self.assertEqual(self.fetches, [])

    def test_all_report_urls_validated_before_any_download(self):
        self.report["install"][1]["download_info"]["url"] = "file:///private/data.whl"
        with self.assertRaises(B.SetupError):
            self.execute()
        self.assertEqual(self.fetches, [])
        self.assert_unpublished()

    def test_canonical_owner_rejects_dependency_url_before_publication(self):
        self.report["install"], self.payloads = [], {}
        self.add_wheel("outside_server", requested=True, extra={
            "outside_server-1.0.dist-info/METADATA":
            b"Metadata-Version: 2.1\nName: outside_server\nVersion: 1.0\nRequires-Dist: danger @ file:///secret\n"})
        with self.assertRaises(B.SetupError):
            self.execute()
        self.assert_unpublished()

    def test_canonical_owner_rejects_record_tampering(self):
        self.report["install"], self.payloads = [], {}
        self.add_wheel("outside_server", requested=True, mutate_record=lambda rows: rows.pop(0))
        with self.assertRaises(B.SetupError):
            self.execute()
        self.assert_unpublished()

    def test_report_identity_cannot_replace_actual_wheel_identity(self):
        self.report["install"][1]["metadata"]["name"] = "something-else"
        with self.assertRaises(B.SetupError):
            self.execute()
        self.assert_unpublished()

    def test_nonzero_worker_never_exposes_output_or_fetches(self):
        with self.assertRaises(B.SetupError) as caught:
            self.execute(runner=lambda *a, **kw: (1, b"private provider error"))
        self.assertNotIn("private", str(caught.exception))
        self.assertEqual(self.fetches, [])
        self.assert_unpublished()

    def test_worker_exit_codes_supply_bounded_actionable_diagnostics(self):
        owner = self.owner()
        for index, (code, reason) in enumerate(((73, "runtime_unsupported"), (74, "version_invalid"),
                                              (72, "resource_limit"), (75, "worker_failed"))):
            with self.subTest(code=code):
                self.job = self.workspace / "acquisitions" / f"{index + 1:032x}"
                self.job.mkdir()
                with self.assertRaisesRegex(B.SetupError, "python_acquisition_" + reason):
                    self.execute(owner, runner=lambda *a, **kw: (code, b"private provider error"))
                self.assert_unpublished()

    def test_stop_after_resolution_prevents_fetch_and_publication(self):
        def stop():
            if self.calls:
                raise B.SetupError("operation_stopped", 409)
        with self.assertRaisesRegex(B.SetupError, "operation_stopped"):
            self.execute(check_stop=stop)
        self.assertEqual(self.fetches, [])
        self.assert_unpublished()

    def test_changed_retained_payload_cannot_publish_receipt(self):
        original = B.write_new
        def mutate(path, raw):
            original(path, raw)
            if path.parent.name == "payloads":
                path.write_bytes(b"changed after inspection")
        self.io.write_new = mutate
        with self.assertRaises(B.SetupError):
            self.execute()
        self.assert_unpublished()

    def test_stop_during_final_copy_keeps_partial_evidence_without_receipt(self):
        def stop():
            if (self.staging / self.job.name / "artifact.whl").exists():
                raise B.SetupError("operation_stopped", 409)
        with self.assertRaisesRegex(B.SetupError, "operation_stopped"):
            self.execute(check_stop=stop)
        self.assertTrue((self.staging / self.job.name / "artifact.whl").exists())
        self.assert_unpublished()

    def test_url_restrictions_include_credentials_ports_encodings_and_redirects(self):
        valid = self.report["install"][0]["download_info"]["url"]
        bad = [valid.replace("https:", "http:"), valid + "?x=y", valid + "#sha256=abc",
               valid.replace("files.pythonhosted.org", "files.pythonhosted.org:443"),
               valid.replace("files.pythonhosted.org", "user@files.pythonhosted.org"),
               valid.replace("/packages/", "/packages/../"), valid.replace("fixture", "%66ixture"),
               valid.replace("files.pythonhosted.org", "files.pythonhosted.org.evil.invalid"),
               valid.replace("https:", "https:\n")]
        for url in bad:
            with self.subTest(url=url), self.assertRaises(ValueError):
                M.artifact_url(url)
        with self.assertRaises(ValueError):
            M.NoRedirect().redirect_request(None, None, 302, "", {}, valid)

    def test_bounded_fetch_rejects_truncation_oversize_and_hash_mismatch(self):
        owner = self.owner()
        row = owner.parse_report(M.wire(self.report), self.request)[0]
        class Response(io.BytesIO):
            status = 200
            def geturl(self):
                return row["url"]
        for payload, headers in [(b"short", {"Content-Length": "9"}),
                                 (b"wrong bytes", {}), (b"", {"Content-Length": str(M.WHEEL_LIMIT + 1)})]:
            response = Response(payload)
            response.headers = headers
            opener = SimpleNamespace(open=lambda *a, **k: response)
            with self.subTest(headers=headers), patch.object(M.urllib.request, "build_opener", return_value=opener):
                with self.assertRaises(ValueError):
                    owner.fetch(row, lambda: 10)

    def test_dripping_response_yields_to_stop_after_one_buffer_fill(self):
        owner = self.owner()
        row = owner.parse_report(M.wire(self.report), self.request)[0]
        class Response(io.BytesIO):
            status = 200
            headers = {}
            fills = 0
            def geturl(self):
                return row["url"]
            def read(self, size=-1):
                raise AssertionError("a filling read can hide stop and deadline checkpoints")
            def read1(self, size=-1):
                self.fills += 1
                return b"x"
        response = Response()
        def checkpoint():
            if response.fills:
                raise B.SetupError("operation_stopped", 409)
            return 10
        with patch.object(M.urllib.request, "build_opener", return_value=SimpleNamespace(open=lambda *a, **k: response)):
            with self.assertRaisesRegex(B.SetupError, "operation_stopped"):
                owner.fetch(row, checkpoint)
        self.assertEqual(response.fills, 1)

    def test_slow_fetch_yields_between_receives_for_stop(self):
        owner = self.owner()
        row = owner.parse_report(M.wire(self.report), self.request)[0]
        class SlowResponse(io.BytesIO):
            status = 200
            headers = {}
            def geturl(self):
                return row["url"]
            def read(self, size=-1):
                raise AssertionError("read(size) may hide repeated receives")
            def read1(self, size=-1):
                return b"x"
        checks = 0
        def stop():
            nonlocal checks
            checks += 1
            if checks >= 4:
                raise B.SetupError("operation_stopped", 409)
            return 10
        opener = SimpleNamespace(open=lambda *a, **k: SlowResponse())
        with patch.object(M.urllib.request, "build_opener", return_value=opener):
            with self.assertRaisesRegex(B.SetupError, "operation_stopped"):
                owner.fetch(row, stop)
        self.assertEqual(checks, 4)

    @unittest.skipUnless(sys.version_info[:2] == (3, 11) and sys.platform == "darwin" and platform.machine() == "arm64",
                         "real worker requires the supported runtime target")
    def test_real_runner_child_uses_private_environment_and_frozen_pip(self):
        script = ("import json, os, pathlib, sys\n"
                  "assert '--dry-run' in sys.argv and '--ignore-installed' in sys.argv\n"
                  "assert '--only-binary=:all:' in sys.argv\n"
                  "assert os.environ['PIP_CONFIG_FILE'] == '/dev/null'\n"
                  "assert 'PRIVATE_TEST_SECRET' not in os.environ\n"
                  "assert pathlib.Path(os.environ['HOME']).parent == pathlib.Path.cwd()\n"
                  "pathlib.Path(sys.argv[sys.argv.index('--report')+1]).write_text(" + repr(json.dumps(self.report)) + ")\n")
        with zipfile.ZipFile(self.pip, "w") as archive:
            for name in ("pip/__init__.py", "pip/_vendor/__init__.py", "pip/_vendor/packaging/__init__.py"):
                archive.writestr(name, "")
            archive.writestr("pip/_vendor/packaging/version.py", "class Version(str): pass\n")
            archive.writestr("__main__.py", script)
        self.profile["pip"]["sha256"] = M.sha(self.pip.read_bytes())
        runner = B.Runner(self.workspace)
        with patch.dict(os.environ, {"PRIVATE_TEST_SECRET": "never inherit"}):
            result = self.execute(runner=runner.run)
        self.assertEqual(result["primary"]["name"], "outside-server")

    @unittest.skipUnless(os.environ.get("TOOLKIT_PIP_ZIPAPP"), "optional exact-pip dependency guard check")
    def test_pinned_pip_refuses_direct_dependencies_from_pypi_before_fetch(self):
        pip = Path(os.environ["TOOLKIT_PIP_ZIPAPP"]).resolve()
        script = '''
import sys
sys.path.insert(0, sys.argv[1])
from pip._internal.req.constructors import install_req_from_line, install_req_from_req_string
from pip._internal.exceptions import InstallationError
parent = install_req_from_line('parent @ https://files.pythonhosted.org/packages/parent-1-py3-none-any.whl')
for url in ('https://outside.invalid/a.whl', 'file:///private/a.whl',
            'git+https://outside.invalid/a', 'https://files.pythonhosted.org/packages/a.whl'):
    try:
        install_req_from_req_string('dependency @ ' + url, comes_from=parent)
    except InstallationError:
        continue
    raise AssertionError('direct URL was accepted')
print('four direct dependency forms rejected before candidate preparation')
'''
        result = subprocess.run([str(self.python), "-I", "-S", "-B", "-c", script, str(pip)],
                                capture_output=True, timeout=10, check=True)
        self.assertIn(b"four direct dependency forms rejected", result.stdout)


if __name__ == "__main__":
    unittest.main()
