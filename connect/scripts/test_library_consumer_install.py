"""Adversarial preflight and subprocess ownership checks for offline installation."""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch
import zipfile

SCRIPT = Path(__file__).with_name("install-library-consumer.py")
SPEC = importlib.util.spec_from_file_location("consumer_install", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
ENV = {"PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1", "PYTHONNOUSERSITE": "1"}


def wheel(dependency="", package="meerkat_toolkit_services", version="0.1.0"):
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w") as archive:
        archive.writestr(package + "-" + version + ".dist-info/METADATA",
                         "Metadata-Version: 2.1\nName: " + package + "\nVersion: " + version + "\n" + dependency)
    return output.getvalue()


class InstallBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="toolkit-install-test-")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name).resolve()
        self.bundle = self.root / "bundle"
        (self.bundle / "services").mkdir(parents=True)
        self.output = self.root / "environment"
        raw = wheel()
        self.files = {"meerkat_toolkit_services-0.1.0-py3-none-any.whl": raw,
                      "install-offline.txt": ("meerkat-toolkit-services==0.1.0 --hash=sha256:" + MODULE.sha(raw) + "\n").encode()}
        self.bind()

    def bind(self):
        for path, data in self.files.items():
            (self.bundle / "services" / path).write_bytes(data)
        contents = {"schema_version": 1, "files": {"services/" + path: {"sha256": MODULE.sha(data)} for path, data in self.files.items()}}
        raw = MODULE.encoded(contents)
        (self.bundle / "CONTENTS.json").write_bytes(raw)
        self.args = argparse.Namespace(bundle=str(self.bundle), contents_sha256=MODULE.sha(raw), family="services",
                                      destination=str(self.output), python=sys.executable, node=None, npm_cli=None)

    def refused_before_effect(self):
        with patch.object(MODULE, "run") as command:
            with self.assertRaises((ValueError, OSError, KeyError)):
                MODULE.install(self.args)
            command.assert_not_called()
        self.assertFalse(self.output.exists())

    def test_bundle_substitution_cannot_self_authorize_with_new_checksums(self):
        selected = self.args.contents_sha256
        self.files["extra.txt"] = b"replacement bundle"
        self.bind()
        self.args.contents_sha256 = selected
        self.refused_before_effect()

    def test_changed_wheel_refused_before_destination(self):
        (self.bundle / "services" / next(iter(self.files))).write_bytes(b"corrupt")
        self.refused_before_effect()

    def test_missing_dependency_wheel_refused_even_when_contents_is_selected(self):
        self.files["install-offline.txt"] += b"missing==1.0 --hash=sha256:" + b"0" * 64 + b"\n"
        self.bind()
        self.refused_before_effect()

    def test_corrupt_dependency_selected_by_same_name_and_version_refused(self):
        self.files["install-offline.txt"] = self.files["install-offline.txt"].replace(MODULE.sha(next(iter(self.files.values()))).encode(), b"f" * 64)
        self.bind()
        self.refused_before_effect()

    def test_direct_url_requirements_refused_before_any_runtime(self):
        self.files["install-offline.txt"] = b"dependency @ https://example.invalid/private.whl\n"
        self.bind()
        self.refused_before_effect()

    def test_direct_url_transitive_metadata_refused(self):
        filename = next(iter(self.files))
        raw = wheel("Requires-Dist: dependency @ https://example.invalid/private.whl\n")
        self.files[filename] = raw
        self.files["install-offline.txt"] = ("meerkat-toolkit-services==0.1.0 --hash=sha256:" + MODULE.sha(raw) + "\n").encode()
        self.bind()
        self.refused_before_effect()

    def test_symlink_input_refused_even_if_bytes_match(self):
        target = self.bundle / "services" / next(iter(self.files))
        saved = self.root / "saved.whl"
        target.rename(saved)
        target.symlink_to(saved)
        self.refused_before_effect()

    def test_fifo_input_is_refused_without_blocking(self):
        target = self.bundle / "services" / next(iter(self.files))
        target.unlink()
        os.mkfifo(target)
        self.refused_before_effect()

    def test_existing_environment_and_dangling_link_are_preserved(self):
        for kind in ("directory", "link"):
            if kind == "directory":
                self.output.mkdir()
                (self.output / "owned").write_text("old runtime")
            else:
                self.output.symlink_to(self.root / "absent")
            with patch.object(MODULE, "run") as command, self.assertRaises(ValueError):
                MODULE.install(self.args)
            command.assert_not_called()
            if kind == "directory":
                self.assertEqual((self.output / "owned").read_text(), "old runtime")
                (self.output / "owned").unlink()
                self.output.rmdir()
            else:
                self.assertTrue(self.output.is_symlink())
                self.output.unlink()

    def test_install_inside_bundle_refused(self):
        self.args.destination = str(self.bundle / "new-environment")
        self.refused_before_effect()
        self.assertFalse((self.bundle / "new-environment").exists())

    def test_installed_graph_cannot_add_drop_or_duplicate_distributions(self):
        cases = [
            [["pip", "1"], ["meerkat-toolkit-services", "0.1.0"], ["unexpected", "1"]],
            [["pip", "1"]],
            [["pip", "1"], ["meerkat-toolkit-services", "0.1.0"], ["meerkat_toolkit_services", "0.1.0"]],
        ]
        for index, distributions in enumerate(cases):
            destination = self.root / ("graph-" + str(index))
            self.args.destination = str(destination)
            def fake_run(argv, cwd, env, label, timeout=900):
                if label == "python-runtime":
                    return MODULE.encoded(["3.11 fixture", [3, 11], "darwin", "arm64", "cpython"])
                if label in ("bootstrap-python", "installed-python"):
                    return MODULE.encoded({"site": str(destination / "runtime/lib/python3.11/site-packages"),
                                           "distributions": [["pip", "1"]] if label == "bootstrap-python" else distributions})
                return b""
            with self.subTest(index=index), patch.object(MODULE, "run", side_effect=fake_run), self.assertRaises(ValueError):
                MODULE.install(self.args)
            self.assertFalse((destination / "installed.json").exists())

    def test_dependency_payload_and_distribution_metadata_are_bound_to_archive(self):
        installed = self.root / "installed"
        (installed / "dependency").mkdir(parents=True)
        (installed / "dependency-1.dist-info").mkdir()
        package = self.root / "dependency.whl"
        payload = {"dependency/__init__.py": b"value = 1\n", "dependency-1.dist-info/METADATA": b"Name: dependency\nVersion: 1\n"}
        with zipfile.ZipFile(package, "w") as archive:
            for path, data in payload.items():
                archive.writestr(path, data)
                (installed / path).write_bytes(data)
        MODULE.verify_wheel_payload(package, installed, self.root)
        for path, data in payload.items():
            (installed / path).write_bytes(b"substituted")
            with self.subTest(path=path), self.assertRaises(ValueError):
                MODULE.verify_wheel_payload(package, installed, self.root)
            (installed / path).write_bytes(data)

    def test_relocated_script_body_and_selected_interpreter_are_bound(self):
        (self.root / "bin").mkdir()
        script = self.root / "bin/jp.py"
        package = self.root / "jmespath.whl"
        with zipfile.ZipFile(package, "w") as archive:
            archive.writestr("jmespath-1.data/scripts/jp.py", b"#!python\nprint('expected')\n")
        good = b"#!" + str(self.root / "bin/python").encode() + b"\nprint('expected')\n"
        script.write_bytes(good)
        MODULE.verify_wheel_payload(package, self.root, self.root)
        for corrupt in (good.replace(b"expected", b"different"), good.replace(b"/bin/python", b"/bin/other")):
            script.write_bytes(corrupt)
            with self.assertRaises(ValueError):
                MODULE.verify_wheel_payload(package, self.root, self.root)

    def test_timeout_joins_owned_child_and_preserves_partial_logs(self):
        with self.assertRaises(subprocess.TimeoutExpired):
            MODULE.run([sys.executable, "-I", "-c", "import os,time; print(os.getpid(),flush=True); time.sleep(30)"],
                       self.root, ENV, "sleeping-installer", timeout=0.2)
        pid = int((self.root / "sleeping-installer.stdout").read_text())
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)
        self.assertFalse((self.root / "installed.json").exists())

    def test_repeated_cancellation_joins_owned_child_that_ignores_termination(self):
        controller = self.root / "controller.py"
        controller.write_text("import importlib.util,pathlib,signal\n"
                              + "spec=importlib.util.spec_from_file_location('installer'," + repr(str(SCRIPT.resolve())) + ")\n"
                              + "m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m)\n"
                              + "def stop(sig,frame): raise KeyboardInterrupt\n"
                              + "signal.signal(signal.SIGTERM,stop)\n"
                              + "m.run(" + repr([sys.executable, "-I", "-c", "import os,signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); print(os.getpid(),flush=True); time.sleep(30)"])
                              + ",pathlib.Path(" + repr(str(self.root)) + ")," + repr(ENV) + ",'cancelled-installer')\n")
        child = subprocess.Popen([sys.executable, "-I", str(controller)], env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            log = self.root / "cancelled-installer.stdout"
            deadline = time.monotonic() + 5
            while not log.exists() or not log.read_bytes():
                if child.poll() is not None or time.monotonic() > deadline:
                    self.fail("owned installer did not start")
                time.sleep(0.02)
            pid = int(log.read_text())
            child.terminate()
            time.sleep(0.1)
            child.send_signal(signal.SIGINT)
            self.assertNotEqual(child.wait(timeout=5), 0)
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)
            self.assertFalse((self.root / "installed.json").exists())
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()


class NpmClosureTests(unittest.TestCase):
    def setUp(self):
        component = "@rkat/toolkit-activepieces"
        self.archive = "rkat-toolkit-activepieces-0.1.0.tgz"
        dependency = {component: "file:./" + self.archive}
        self.package = {"dependencies": dependency}
        self.lock = {"lockfileVersion": 3, "packages": {
            "": {"dependencies": dict(dependency)},
            "node_modules/" + component: {"version": "0.1.0", "resolved": "file:" + self.archive},
            "node_modules/dependency": {"version": "1", "resolved": "https://example.invalid/dependency.tgz", "integrity": "sha512-fixture"},
        }}
        self.cache = {"schema_version": 1, "tarballs": [{"name": "dependency", "version": "1", "path": "dependency.tgz",
                         "sha256": MODULE.sha(b"dependency"), "integrity": "sha512-fixture"}]}

    def files(self):
        return {"package.json": MODULE.encoded(self.package), "package-lock.json": MODULE.encoded(self.lock),
                "offline-cache.json": MODULE.encoded(self.cache), self.archive: b"component", "dependency.tgz": b"dependency"}

    def test_extra_local_dependency_cannot_escape_frozen_inputs(self):
        MODULE.npm_closure(self.files())
        self.lock["packages"]["node_modules/outside"] = {"resolved": "file:../../outside", "version": "1"}
        with self.assertRaises(ValueError):
            MODULE.npm_closure(self.files())

    def test_component_local_reference_must_be_exact(self):
        self.lock["packages"]["node_modules/@rkat/toolkit-activepieces"]["resolved"] = "file:../../outside.tgz"
        with self.assertRaises(ValueError):
            MODULE.npm_closure(self.files())

    def test_project_cannot_add_dependency_or_workspace(self):
        self.package["dependencies"]["outside"] = "file:../../outside"
        with self.assertRaises(ValueError):
            MODULE.npm_closure(self.files())

    def test_missing_or_corrupt_dependency_is_refused(self):
        for mode in ("missing", "corrupt"):
            files = self.files()
            if mode == "missing":
                del files["dependency.tgz"]
            else:
                files["dependency.tgz"] = b"changed"
            with self.subTest(mode=mode), self.assertRaises((ValueError, KeyError)):
                MODULE.npm_closure(files)


if __name__ == "__main__":
    unittest.main()
