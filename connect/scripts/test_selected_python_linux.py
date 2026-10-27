"""Selected Python target binding and inert Linux filesystem readback."""
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import unittest
from unittest.mock import patch

import test_selected_reconciliation as previous

M, P = previous.M, previous.P


class SelectedPythonLinuxTests(unittest.TestCase):
    def fixture(self, target="linux-x86_64"):
        case = previous.SelectedReadbackTests()
        fixture, receipt = case.fixture(True)
        self.addCleanup(case.doCleanups)
        fixture.args.python_target = target
        if target == "linux-x86_64":
            receipt["python_target"] = target
            venv = Path(fixture.args.destination) / "runtime"
            (venv / "lib64").symlink_to("lib", target_is_directory=True)
            (Path(fixture.args.destination) / "ready/installed.json").write_bytes(M.encoded(receipt))
        return case, fixture, receipt

    def install_fixture(self):
        fixture = P.SelectedPythonTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        return fixture

    def test_linux_exact_lib64_link_reads_without_runtime_execution(self):
        case, fixture, _ = self.fixture()
        result = case.check(fixture)
        self.assertTrue(result["verified"])
        self.assertEqual(result["python_target"], "linux-x86_64")
        self.assertIn("Runtime executable bytes, versions, compatibility", result["excluded"][0])
        self.assertEqual(os.readlink(Path(fixture.args.destination) / "runtime/lib64"), "lib")

    def test_linux_lib64_does_not_admit_aliases_external_links_or_other_kinds(self):
        for replacement in ("absolute", "parent", "outside", "loop", "directory", "file"):
            with self.subTest(replacement=replacement):
                case, fixture, _ = self.fixture()
                venv = Path(fixture.args.destination) / "runtime"
                link = venv / "lib64"
                link.unlink()
                if replacement == "directory":
                    link.mkdir()
                elif replacement == "file":
                    link.write_bytes(b"lib")
                else:
                    destination = {"absolute": str(venv / "lib"), "parent": "../runtime/lib",
                                   "outside": str(fixture.bundle), "loop": "lib64"}[replacement]
                    link.symlink_to(destination)
                case.check(fixture, False)

    def test_target_cannot_be_inferred_or_changed_from_receipt(self):
        for requested in (None, "macos-arm64", "linux-arm64"):
            with self.subTest(requested=requested):
                case, fixture, _ = self.fixture()
                fixture.args.python_target = requested
                case.check(fixture, False)
        for receipt_target in (None, "macos-arm64", True):
            with self.subTest(receipt_target=receipt_target):
                case, fixture, receipt = self.fixture()
                if receipt_target is None:
                    receipt.pop("python_target")
                else:
                    receipt["python_target"] = receipt_target
                (Path(fixture.args.destination) / "ready/installed.json").write_bytes(M.encoded(receipt))
                case.check(fixture, False)

    def test_mac_default_and_explicit_target_preserve_legacy_readback(self):
        for target in (None, "macos-arm64"):
            with self.subTest(target=target):
                case, fixture, receipt = self.fixture(target)
                self.assertNotIn("python_target", receipt)
                self.assertNotIn("python_target", case.check(fixture))

    def test_mac_does_not_accept_linux_lib64_relaxation(self):
        case, fixture, _ = self.fixture("macos-arm64")
        (Path(fixture.args.destination) / "runtime/lib64").symlink_to("lib")
        case.check(fixture, False)

    def test_linux_readback_refuses_native_extension_shadowing_for_both_libcs(self):
        for suffix in (".cpython-311-x86_64-linux-gnu.so", ".cpython-311-x86_64-linux-musl.so", ".abi3.so", ".so"):
            for stem in ("outside_server/__main__", "outside_server/__init__"):
                with self.subTest(suffix=suffix, stem=stem):
                    fixture = self.install_fixture()
                    fixture.filename, fixture.files["artifact.whl"], _ = P.wheel(extra={stem + suffix: b"inert binary fixture"})
                    fixture.bind()
                    with self.assertRaisesRegex(M.InstallError, "shadows selected"):
                        M.selected_python_readback(Path(fixture.args.destination), str(Path(sys.executable).resolve()),
                                                   fixture.files, fixture.plan, "linux-x86_64")

    def test_wrong_native_target_refuses_before_venv_or_package_install(self):
        for observed in (["fixture", [3, 11], "darwin", "arm64", "cpython"],
                         ["fixture", [3, 11], "linux", "aarch64", "cpython"],
                         ["fixture", [3, 12], "linux", "x86_64", "cpython"],
                         ["fixture", [3, 11], "linux", "x86_64", "pypy"],
                         ["fixture", [3, 11], "linux", "x86_64", "cpython"]):
            with self.subTest(observed=observed):
                fixture = self.install_fixture()
                fixture.args.python_target = "linux-x86_64"

                def run(argv, cwd, env, label):
                    if label == "python-runtime":
                        fixture.calls.append(label)
                        return M.encoded({"runtime": observed, "pointer_bits": 32, "stdlib": ["sys"], "extensions": [".so"]})
                    return fixture.fake_run(argv, cwd, env, label)

                with patch.object(M, "run", side_effect=run), self.assertRaisesRegex(M.InstallError, "native CPython"):
                    M.install_selected_python(fixture.args)
                self.assertEqual(fixture.calls, ["validate-selection", "python-runtime"])
                self.assertFalse((Path(fixture.args.destination) / "runtime").exists())
                self.assertFalse((Path(fixture.args.destination) / "ready").exists())

    def test_invalid_target_refuses_before_any_destination_or_child(self):
        fixture = self.install_fixture()
        fixture.args.python_target = "linux-arm64"
        with patch.object(M, "run") as run, self.assertRaisesRegex(M.InstallError, "unsupported selected Python target"):
            M.install_selected_python(fixture.args)
        run.assert_not_called()
        self.assertFalse(Path(fixture.args.destination).exists())

    def test_linux_venv_link_is_checked_before_pip_entry(self):
        fixture = self.install_fixture()
        fixture.args.python_target = "linux-x86_64"

        def run(argv, cwd, env, label):
            if label == "validate-selection":
                return M.encoded(fixture.plan)
            if label == "python-runtime":
                return M.encoded({"runtime": ["fixture", [3, 11], "linux", "x86_64", "cpython"],
                                  "pointer_bits": 64, "stdlib": ["sys"], "extensions": [".so"]})
            if label == "create-venv":
                venv = cwd / "runtime"
                venv.mkdir()
                (venv / "lib64").symlink_to(str(fixture.bundle))
                return b""
            if label == "bootstrap-python":
                return M.encoded({"distributions": [["pip", "fixture"]],
                                  "site": str(cwd / "runtime/lib/python3.11/site-packages")})
            self.fail("unbound Linux link reached " + label)

        with patch.object(M, "run", side_effect=run), self.assertRaisesRegex(M.InstallError, "lib64 must link to lib"):
            M.install_selected_python(fixture.args)
        self.assertFalse((Path(fixture.args.destination) / "ready").exists())

    def test_real_native_offline_install_and_inert_readback(self):
        pair = (sys.platform, platform.machine())
        if sys.version_info[:2] != (3, 11) or pair not in M.SELECTED_PYTHON_TARGETS.values():
            self.skipTest("native CPython 3.11 selected target required")
        fixture = self.install_fixture()
        target = next(key for key, value in M.SELECTED_PYTHON_TARGETS.items() if value == pair)
        fixture.args.python_target = target
        validator = fixture.root / "validator"
        validator.write_text("#!" + sys.executable + "\nimport sys\nsys.stdout.write(" + repr(json.dumps(fixture.plan)) + ")\n")
        validator.chmod(0o700)
        fixture.args.validator = str(validator)
        receipt = M.install_selected_python(fixture.args)
        if target == "linux-x86_64":
            self.assertEqual(receipt["python_target"], target)
            self.assertEqual(os.readlink(Path(fixture.args.destination) / "runtime/lib64"), "lib")
        else:
            self.assertNotIn("python_target", receipt)
        self.assertEqual(receipt["runtime"][1:], [[3, 11], *pair, "cpython"])
        before = previous.snapshot(Path(fixture.args.destination))
        with patch.object(M, "run", side_effect=AssertionError("observed runtime must not execute during readback")):
            self.assertTrue(M.verify_existing_selected(fixture.args)["verified"])
        self.assertEqual(previous.snapshot(Path(fixture.args.destination)), before)

    def test_cli_target_is_restricted_to_selected_python(self):
        for mode in ("--bundle", "--selected-node", "--official-bundle"):
            result = subprocess.run([sys.executable, "-I", "-B", str(previous.SCRIPT), mode, "/not-used",
                                     "--python-target", "linux-x86_64"], capture_output=True, timeout=15)
            self.assertEqual(result.returncode, 2)
            self.assertIn(b"python-target requires selected Python mode", result.stderr)


if __name__ == "__main__":
    unittest.main()
