"""Exact pins through acquisition, owned worker and non-executing readback.

The pip fixture records argv and emits an inert report, using the installed
pip version parser. No resolver, network or package payload runs.
"""
import asyncio
import copy
import json
import os
from pathlib import Path
import platform
import sys
import unittest
from unittest.mock import patch
import zipfile

import test_python_acquisition as fixtures
import test_acquisition_recovery as recovery
from test_selected_setup import adapter, backend

M, B = fixtures.M, fixtures.B
PINS = [{"distribution": "Dependency", "version": "1.0"},
        {"distribution": "Unused_pkg", "version": "2.0"}]
DERIVED = b"dependency==1.0\nunused-pkg==2.0\n"
NATIVE = sys.implementation.name == "cpython" and sys.version_info[:2] == (3, 11) and (sys.platform, platform.machine()) in M.TARGETS.values()


class ConstraintTests(unittest.TestCase):
    def case(self, constrained=True):
        case = fixtures.AcquisitionTests(); case.setUp(); self.addCleanup(case.doCleanups)
        if constrained:
            case.request["constraints"] = copy.deepcopy(PINS)
        return case

    def runner(self, case, raw=DERIVED):
        def run(*args, **kwargs):
            if raw is not None:
                (case.job / "constraints.txt").write_bytes(raw)
            return case.runner(*args, **kwargs)
        return run

    def readback(self, case, owner):
        request = {"job_id": case.job.name, "family": "selected-python", "workspace": str(owner.workspace),
                   "staging_root": str(owner.staging), "arguments": copy.deepcopy(case.request),
                   "host": {"profile_sha256": owner.profile_sha,
                            "assets": {name: M.sha(raw) for name, raw in owner.frozen.items()}}}
        with patch("subprocess.Popen", side_effect=AssertionError("readback executed a child")):
            return M.readback(case.io, request, case.job)

    def test_closed_pins_preserve_request_and_refuse_aliases_primary_conflicts_and_options(self):
        case = self.case(); owner = case.owner()
        retained = owner.arguments(case.request)
        self.assertEqual(retained, case.request)
        retained["constraints"][0]["version"] = "2.0"
        self.assertEqual(case.request["constraints"][0]["version"], "1.0")
        same_primary = [{"distribution": "Outside_Server", "version": "1.0"}]
        self.assertEqual(owner.arguments({**case.request, "constraints": same_primary})["constraints"], same_primary)
        bad = [None, [], {}, [PINS[0]] * 33,
               [PINS[0], {"distribution": "dependency", "version": "2.0"}],
               [{"distribution": "some._pkg", "version": "1.0"}, {"distribution": "Some-pkg", "version": "1.0"}],
               [{"distribution": "Outside_Server", "version": "2.0"}],
               [{**PINS[0], "path": "elsewhere"}]]
        bad += [[{"distribution": name, "version": "1.0"}] for name in
                ("-option", "x[extra]", "x @ https://example.invalid/x", "x\nflag", "é", "x/else", "x.")]
        bad += [[{"distribution": "dependency", "version": version}] for version in
                (None, 1, ">=1", "1.*", "1;python_version<'4'", "1\n--index-url=x", "https://example.invalid", "1[extra]")]
        for pins in bad:
            with self.subTest(pins=pins), self.assertRaisesRegex(B.SetupError, "invalid_python_acquisition_arguments"):
                owner.arguments({**case.request, "constraints": pins})
        self.assertEqual(case.calls, [])

    def pip_fixture(self, case):
        import pip._vendor.packaging.version as version
        import pip._vendor.packaging._structures as structures
        marker = case.root / "pip-observed.json"
        source = ("import json,os,pathlib,sys\n"
                  "pathlib.Path(" + repr(str(marker)) + ").write_text(json.dumps({'argv':sys.argv,'private':os.environ.get('PRIVATE_TEST_SECRET')}))\n"
                  "pathlib.Path(sys.argv[sys.argv.index('--report')+1]).write_text(" + repr(json.dumps(case.report)) + ")\n")
        with zipfile.ZipFile(case.pip, "w") as archive:
            for name in ("pip/__init__.py", "pip/_vendor/__init__.py", "pip/_vendor/packaging/__init__.py"):
                archive.writestr(name, "")
            archive.writestr("pip/_vendor/packaging/version.py", Path(version.__file__).read_bytes())
            archive.writestr("pip/_vendor/packaging/_structures.py", Path(structures.__file__).read_bytes())
            archive.writestr("__main__.py", source)
        case.profile["pip"]["sha256"] = M.sha(case.pip.read_bytes())
        case.profile["target"] = next(name for name, actual in M.TARGETS.items() if actual == (sys.platform, platform.machine()))
        return marker

    @unittest.skipUnless(NATIVE, "requires supported CPython 3.11")
    def test_real_worker_fixed_file_and_omitted_behavior(self):
        for constrained in (False, True):
            with self.subTest(constrained=constrained):
                case = self.case(constrained); marker = self.pip_fixture(case); owner = case.owner()
                with patch.dict(os.environ, {"PRIVATE_TEST_SECRET": "not inherited"}):
                    result = case.execute(owner, runner=B.Runner(case.workspace).run)
                expected = [str(owner.assets / "pip.pyz"), "--disable-pip-version-check", "--no-input",
                            "--keyring-provider", "disabled", "--retries", "0", "--timeout", "10", "--no-cache-dir",
                            "install", "--dry-run", "--ignore-installed", "--only-binary=:all:", "--progress-bar", "off",
                            "--report", str(case.job / "report.json"), "--index-url", "https://pypi.org/simple"]
                path = case.job / "constraints.txt"
                if constrained:
                    expected += ["--constraint", str(path)]
                    self.assertEqual(path.read_bytes(), DERIVED)
                    self.assertEqual(path.stat().st_mode & 0o777, 0o600)
                else:
                    self.assertFalse(path.exists())
                    self.assertNotIn("constraints", result["request"])
                self.assertEqual(json.loads(marker.read_bytes()), {"argv": expected + ["outside-server==1.0"], "private": None})
                self.assertEqual(result["request"], case.request)
                self.assertEqual({row["name"] for row in result["artifacts"]}, {"outside-server", "dependency"})
                self.assertEqual(self.readback(case, owner), result)

    @unittest.skipUnless(NATIVE, "requires supported CPython 3.11")
    def test_real_worker_bad_versions_and_preexisting_file_never_enter_pip(self):
        for failure in ("1banana", "01.0", "1.0RC1", "preexisting", "symlink"):
            with self.subTest(failure=failure):
                case = self.case(); marker = self.pip_fixture(case)
                if failure in ("preexisting", "symlink"):
                    destination = case.job / "constraints.txt"
                    if failure == "symlink":
                        target = case.root / "keep"; target.write_bytes(b"untouched"); destination.symlink_to(target)
                    else:
                        destination.write_bytes(b"untouched")
                    reason = "python_acquisition_worker_failed"
                else:
                    case.request["constraints"][0]["version"] = failure
                    reason = "python_acquisition_version_invalid"
                owner = case.owner()
                with self.assertRaisesRegex(B.SetupError, reason):
                    case.execute(owner, runner=B.Runner(case.workspace).run)
                self.assertFalse(marker.exists())
                self.assertEqual(case.fetches, [])
                case.assert_unpublished()
                if failure in ("preexisting", "symlink"):
                    self.assertEqual(destination.read_bytes(), b"untouched")

    def test_file_or_report_disagreement_refuses_before_fetch(self):
        for failure in ("missing", "different", "report"):
            with self.subTest(failure=failure):
                case = self.case(); owner = case.owner()
                raw = None if failure == "missing" else b"dependency==2.0\n" if failure == "different" else DERIVED
                if failure == "report":
                    case.report["install"][1]["metadata"]["version"] = "2.0"
                with self.assertRaisesRegex(B.SetupError, "python_acquisition_unconfirmed"):
                    case.execute(owner, runner=self.runner(case, raw))
                self.assertEqual(case.fetches, [])
                case.assert_unpublished()

    def test_late_file_change_prevents_publication(self):
        case = self.case(); owner = case.owner(); fetch = case.fetch
        def changed(*args):
            value = fetch(*args)
            (case.job / "constraints.txt").write_bytes(b"dependency==2.0\n")
            return value
        with patch.object(case, "fetch", changed), self.assertRaisesRegex(B.SetupError, "python_acquisition_unconfirmed"):
            case.execute(owner, runner=self.runner(case))
        case.assert_unpublished()

    def test_readback_file_and_report_tampering_cannot_be_repaired_by_receipt(self):
        case = self.case(); owner = case.owner()
        result = case.execute(owner, runner=self.runner(case))
        self.assertEqual(self.readback(case, owner), result)
        path = case.job / "constraints.txt"
        for raw in (b"", b"dependency==2.0\nunused-pkg==2.0\n", b"x" * (M.CONSTRAINT_BYTES + 1)):
            with self.subTest(raw_length=len(raw)):
                path.write_bytes(raw)
                with self.assertRaisesRegex(B.SetupError, "python_acquisition_readback_unconfirmed"):
                    self.readback(case, owner)
        path.unlink()
        with self.assertRaisesRegex(B.SetupError, "python_acquisition_readback_unconfirmed"):
            self.readback(case, owner)
        target = case.root / "same-constraints"; target.write_bytes(DERIVED); path.symlink_to(target)
        with self.assertRaisesRegex(B.SetupError, "python_acquisition_readback_unconfirmed"):
            self.readback(case, owner)
        path.unlink(); path.write_bytes(DERIVED)
        report = copy.deepcopy(case.report); report["install"][1]["metadata"]["version"] = "2.0"
        report_raw = M.wire(report); (case.job / "report.json").write_bytes(report_raw)
        receipt_path = case.staging / case.job.name / "acquisition.json"
        receipt = json.loads(receipt_path.read_bytes())
        receipt["report_sha256"] = M.sha(report_raw)
        receipt["artifacts"][1]["version"] = "2.0"
        receipt_path.write_bytes(M.wire(receipt))
        with self.assertRaisesRegex(B.SetupError, "python_acquisition_readback_unconfirmed"):
            self.readback(case, owner)

    def test_late_file_change_during_readback_is_unconfirmed(self):
        case = self.case(); owner = case.owner()
        case.execute(owner, runner=self.runner(case))
        read = case.io.read_file
        def changed(root, name, *args):
            value = read(root, name, *args)
            if name == "artifact.whl":
                (case.job / "constraints.txt").write_bytes(b"dependency==2.0\n")
            return value
        with patch.object(case.io, "read_file", changed), self.assertRaisesRegex(B.SetupError, "python_acquisition_readback_unconfirmed"):
            self.readback(case, owner)

    def test_constrained_replay_recovery_and_next_session_never_reenter_effect(self):
        fixture = recovery.AcquisitionRecoveryTests(); self.addCleanup(fixture.doCleanups)
        case, original, owner = fixture.package_controller("python")
        case.request["constraints"] = copy.deepcopy(PINS)
        run = case.runner
        def constrained(*args, **kwargs):
            (case.job / "constraints.txt").write_bytes(DERIVED)
            return run(*args, **kwargs)
        case.runner = constrained
        first = original.acquire_python(case.request)["job"]; original.join()
        self.assertEqual(original.job["status"], "succeeded", original.job)
        with patch.object(owner, "run", side_effect=AssertionError("replayed effect")):
            self.assertEqual(original.acquire_python(copy.deepcopy(case.request))["job"]["id"], first["id"])
            changed = copy.deepcopy(case.request); changed["constraints"][0]["version"] = "2.0"
            with self.assertRaisesRegex(backend.SetupError, "acquisition_request_changed"):
                original.acquire_python(changed)
        retained, _ = backend.load_acquisition_recovery(original.workspace, first["id"], first["recovery_request"]["sha256"])
        self.assertEqual(retained["arguments"], case.request)
        recovered = fixture.recovered(original)
        with patch("subprocess.Popen", side_effect=AssertionError("recovery executed")):
            self.assertEqual(recovered.reconcile({"job_id": first["id"]})["observation"]["status"], "matched")
            with self.assertRaisesRegex(backend.SetupError, "recovery_read_only"):
                recovered.acquire_python(case.request)
        original.close_admission(); original.join()
        selection = original.recovery_manifest({"job_ids": [first["id"]]})["text"].encode()
        profile = case.root / "selected.json"
        current = backend.Controller(None, None, case.root / "next-round", (), sys.executable, None, None,
            selected_install_profile=profile, selected_install_profile_sha256=backend.digest(profile.read_bytes()),
            python_acquisition_profile=case.profile_path,
            python_acquisition_profile_sha256=backend.digest(case.profile_path.read_bytes()),
            _retained_selections=(selection,))
        self.addCleanup(current.join)
        with patch.object(current.python_acquisition, "run", side_effect=AssertionError("resumed effect")):
            self.assertEqual(current.acquire_python(case.request)["job"]["id"], first["id"])
            with self.assertRaisesRegex(backend.SetupError, "acquisition_request_changed"):
                current.acquire_python(changed)
        self.assertTrue(current.job["recovered"])
        self.assertEqual(current.job["status"], "unconfirmed")
        self.assertEqual(current.reconcile({"job_id": first["id"]})["observation"]["status"], "matched")
        (case.job / "constraints.txt").write_bytes(b"dependency==2.0\n")
        self.assertEqual(current.reconcile({"job_id": first["id"]})["observation"]["status"], "unconfirmed")
        self.assertEqual(current.validations, {})
        self.assertEqual(current.install_intents, {})


class ConstraintMcpTests(unittest.IsolatedAsyncioTestCase):
    async def test_registered_schema_routes_exact_pins_and_refuses_unknown_or_duplicate_input(self):
        from mcp import Client
        fixture = recovery.AcquisitionRecoveryTests(); self.addCleanup(fixture.doCleanups)
        case, controller, owner = fixture.package_controller("python")
        case.request["constraints"] = copy.deepcopy(PINS)
        observed = []
        def effect(request, *_):
            observed.append(copy.deepcopy(request)); return {"scope": "fixture_only"}
        owner.run = effect
        service = adapter.SetupAdapter(controller)
        try:
            async with Client(adapter.create_server(service)) as client:
                tool = next(t for t in (await client.list_tools()).tools if t.name == "setup_acquire_python")
                self.assertIn("constraints", tool.input_schema["properties"])
                for pins in ([], [{**PINS[0], "index_url": "https://example.invalid"}],
                             [PINS[0], {"distribution": "dependency", "version": "2.0"}]):
                    result = await client.call_tool("setup_acquire_python", {**case.request, "constraints": pins})
                    self.assertTrue(result.is_error, result)
                self.assertEqual(observed, [])
                first = await client.call_tool("setup_acquire_python", case.request)
                self.assertFalse(first.is_error, first)
                await asyncio.to_thread(controller.join)
                replay = await client.call_tool("setup_acquire_python", case.request)
                self.assertEqual(first.structured_content["job"]["id"], replay.structured_content["job"]["id"])
                self.assertEqual(observed, [case.request])
        finally:
            await service.aclose()


if __name__ == "__main__":
    unittest.main()
