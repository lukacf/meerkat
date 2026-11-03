"""Selected readback boundaries with inert fixtures, never donor/runtime execution."""
from __future__ import annotations

import copy
import importlib.util
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


def load(filename, name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


P = load("test_selected_python_install.py", "python_fixtures")
N = load("test_selected_node_install.py", "node_fixtures")
M = P.M
SCRIPT = Path(__file__).with_name("install-library-consumer.py").resolve()


def snapshot(root):
    result = {}
    for path in sorted(root.rglob("*")):
        key = str(path.relative_to(root))
        result[key] = ("link", os.readlink(path)) if path.is_symlink() else ("directory", None) if path.is_dir() else ("file", path.read_bytes())
    return result


class SelectedReadbackTests(unittest.TestCase):
    def fixture(self, python, argument=None, *, software=False, configure=None):
        fixture = P.SelectedPythonTests() if python else N.SelectedNodeTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        if software:
            fixture.software()
        if configure is not None:
            configure(fixture)
        if argument is not None:
            fixture.plan["entrypoint"]["arguments"] = [argument]
        if not python:
            with patch.object(N.MODULE, "run", side_effect=fixture.fake_run):
                receipt = N.MODULE.install_selected_node(fixture.args)
            return fixture, receipt
        destination = Path(fixture.args.destination)
        for path, data in fixture.files.items():
            target = destination / "inputs" / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        venv = destination / "runtime"
        (venv / "bin").mkdir(parents=True)
        runtime = str(Path(sys.executable).resolve())
        (venv / "pyvenv.cfg").write_text("home = " + str(Path(sys.executable).parent) + "\ninclude-system-site-packages = false\nversion = 3.11.15\nexecutable = " + runtime + "\n")
        for leaf in ("python", "python3", "python3.11"):
            (venv / "bin" / leaf).symlink_to(runtime)
        closure = M.selected_python_closure(fixture.files, fixture.plan, set(sys.stdlib_module_names), P.EXTENSIONS)
        generated, graph, count = {}, {}, 0
        for wheel in closure["wheels"]:
            prefix = "lib/python3.11/site-packages/"
            for path, raw in wheel["files"].items():
                target = venv / prefix / path
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(raw)
                count += not path.endswith("/RECORD")
            for suffix, raw in (("RECORD", b"generated"), ("INSTALLER", b"pip\n"), ("REQUESTED", b"")):
                path = prefix + wheel["dist_info"] + "/" + suffix
                (venv / path).write_bytes(raw)
                generated[path] = M.sha(raw)
            for script in wheel["scripts"]:
                path = "bin/" + script
                raw = b"inert generated wrapper; never executed"
                (venv / path).write_bytes(raw)
                generated[path] = M.sha(raw)
            graph[wheel["name"]] = {"version": wheel["version"], "wheel_sha256": wheel["sha256"], "payloads": len(wheel["files"]) - 1}
            (destination / "wheelhouse").mkdir(exist_ok=True)
            (destination / "wheelhouse" / wheel["filename"]).write_bytes(fixture.files["artifact.whl"])
        bootstrap = venv / "lib/python3.11/site-packages/pip"
        bootstrap.mkdir()
        (bootstrap / "__init__.py").write_bytes(b"# No historical bootstrap claim.\n")
        launch = M.selected_python_launch(str(venv / "bin/python"), fixture.plan, closure)
        configuration = {"mcpServers": {fixture.plan["mcp_name"]: {"command": launch[0], "args": launch[1:]}}}
        receipt = {"schema_version": 1, "scope": "observed_software_installation_only", "selection_sha256": fixture.args.selection_sha256,
                   **({"document_kind": "local_software_declaration", "declaration_digest": fixture.plan["declaration_digest"]}
                      if software else {"manifest_digest": fixture.plan["manifest_digest"]}), "inputs": {key: M.sha(raw) for key, raw in fixture.files.items()},
                   "command": launch, "destination": str(destination), "mcp_configuration": configuration,
                   "installed_graph": {**closure["versions"], "pip": "fixture-only"},
                   "verification": {"wheels": graph, "verified_payload_files": count, "installer_generated": generated,
                                    "preserved_bootstrap_files": 999999}, "runtime": ["unverified historical probe"]}
        (destination / "ready").mkdir()
        (destination / "ready/installed.json").write_bytes(M.encoded(receipt))
        (destination / "ready/mcp.json").write_bytes(M.encoded(configuration))
        return fixture, receipt

    def check(self, fixture, accepted=True):
        destination = Path(fixture.args.destination)
        before, original = snapshot(destination), snapshot(fixture.bundle)
        def validator(argv, cwd):
            self.assertNotIn(str(destination), str(cwd))
            software = getattr(fixture.args, "document_kind", None) == "local_software_declaration"
            filename = "declaration.json" if software else "activation.json"
            flag = "--declaration" if software else "--selection"
            self.assertEqual(argv[argv.index(flag) + 1], str(cwd / filename))
            self.assertNotIn("--selection" if software else "--declaration", argv)
            if ((cwd / filename).read_bytes() != fixture.files[filename]
                    or (cwd / "package.json").read_bytes() != fixture.files["package.json"]):
                raise M.InstallError("fixture metadata owner rejected altered canonical inputs")
            self.assertEqual(argv[-1], fixture.args.component)
            return M.encoded(fixture.plan)
        with patch.object(M, "readback_validator", side_effect=validator), patch.object(M, "run") as no_effect:
            if accepted:
                result = M.verify_existing_selected(fixture.args)
                self.assertTrue(result["verified"])
                self.assertEqual(result["scope"], "retained_payload_and_launch_binding_only")
            else:
                with self.assertRaises((M.InstallError, OSError, KeyError, TypeError)):
                    M.verify_existing_selected(fixture.args)
                result = None
            no_effect.assert_not_called()
        self.assertEqual(snapshot(destination), before)
        self.assertEqual(snapshot(fixture.bundle), original)
        return result

    def test_both_software_forms_reconcile_without_repair_or_runtime_execution(self):
        for python in (True, False):
            fixture, receipt = self.fixture(python, software=True)
            result = self.check(fixture)
            self.assertEqual(result["selection_kind"], "python" if python else "node")
            self.assertEqual(result["document_kind"], "local_software_declaration")
            self.assertEqual(result["declaration_digest"], "d" * 64)
            self.assertNotIn("manifest_digest", result)
            target = Path(fixture.args.destination) / "ready/installed.json"
            for changed in ({**receipt, "manifest_digest": "d" * 64},
                            {**receipt, "document_kind": "remote_declaration"},
                            {**receipt, "declaration_digest": "0" * 64},
                            {key: value for key, value in receipt.items() if key != "document_kind"}):
                target.write_bytes(M.encoded(changed))
                self.check(fixture, False)
            target.write_bytes(M.encoded(receipt))
            fixture.args.document_kind = None
            self.check(fixture, False)
            fixture.args.document_kind = "local_software_declaration"
            self.check(fixture)
            for prefix in (fixture.bundle, Path(fixture.args.destination) / "inputs"):
                selected = prefix / "declaration.json"
                original = selected.read_bytes()
                selected.write_bytes(original + b"changed")
                self.check(fixture, False)
                selected.write_bytes(original)
            self.check(fixture)

    def test_python_and_nested_node_reconcile_without_runtime_execution_or_mutation(self):
        for python in (True, False):
            fixture, _ = self.fixture(python)
            result = self.check(fixture)
            if not python:
                graph = result["retained_payloads"]["installed_graph"]
                self.assertEqual(graph["node_modules/nested"]["version"], "1.0.0")
                self.assertEqual(graph["node_modules/@outside/server/node_modules/nested"]["version"], "2.0.0")

    def test_selected_publications_larger_than_library_configuration_limit(self):
        for python in (True, False):
            fixture, _ = self.fixture(python, argument="\u03bb" * 70000)
            self.assertGreater((Path(fixture.args.destination) / "ready/mcp.json").stat().st_size, 65536)
            self.check(fixture)

    def test_consistent_receipt_and_mcp_launch_tampering_does_not_authorize_itself(self):
        for python in (True, False):
            fixture, receipt = self.fixture(python)
            destination = Path(fixture.args.destination)
            receipt["command"][-1] = "unauthorized changed argument"
            receipt["mcp_configuration"]["mcpServers"][fixture.plan["mcp_name"]]["args"] = receipt["command"][1:]
            (destination / "ready/installed.json").write_bytes(M.encoded(receipt))
            (destination / "ready/mcp.json").write_bytes(M.encoded(receipt["mcp_configuration"]))
            self.check(fixture, False)

    def test_each_original_and_frozen_input_is_bound(self):
        for python in (True, False):
            fixture, _ = self.fixture(python)
            for prefix in (fixture.bundle, Path(fixture.args.destination) / "inputs"):
                for path in fixture.files:
                    target = prefix / path
                    raw = target.read_bytes()
                    target.write_bytes(raw + b"changed")
                    with self.subTest(python=python, prefix=prefix.name, path=path):
                        self.check(fixture, False)
                    target.write_bytes(raw)

    def test_changed_component_and_digest_are_refused(self):
        for python in (True, False):
            fixture, _ = self.fixture(python)
            for key, value in (("component", "another"), ("selection_sha256", "0" * 64)):
                original = getattr(fixture.args, key)
                setattr(fixture.args, key, value)
                self.check(fixture, False)
                setattr(fixture.args, key, original)

    def test_missing_pending_and_crossed_publication_is_not_repaired(self):
        for python in (True, False):
            fixture, _ = self.fixture(python)
            destination = Path(fixture.args.destination)
            for path in ("pending-ready", "installed.json", "configuration", "ready/extra.json"):
                (destination / path).write_bytes(b"uncertain")
                self.check(fixture, False)
                (destination / path).unlink()
            raw = (destination / "ready/mcp.json").read_bytes()
            (destination / "ready/mcp.json").unlink()
            self.check(fixture, False)
            (destination / "ready/mcp.json").write_bytes(raw)

    def test_boolean_schema_and_binding_receipt_changes_are_refused(self):
        for python in (True, False):
            fixture, original = self.fixture(python)
            target = Path(fixture.args.destination) / "ready/installed.json"
            for key, value in (("schema_version", True), ("scope", "ready"), ("manifest_digest", "0" * 64), ("inputs", {}), ("destination", "/other")):
                receipt = {**original, key: value}
                target.write_bytes(M.encoded(receipt))
                self.check(fixture, False)

    def test_payload_extra_startup_and_links_cannot_be_hidden_by_receipt(self):
        for python in (True, False):
            fixture, receipt = self.fixture(python)
            destination = Path(fixture.args.destination)
            payload = destination / ("runtime/lib/python3.11/site-packages/outside_server/__main__.py" if python else "node_modules/@outside/server/dist/main.js")
            raw = payload.read_bytes()
            payload.write_bytes(b"changed")
            self.check(fixture, False)
            payload.write_bytes(raw)
            extra = payload.parent / ("sneaky.pth" if python else "extra.js")
            extra.write_bytes(b"unexpected")
            if python:
                key = str(extra.relative_to(destination / "runtime"))
                receipt["verification"]["installer_generated"][key] = M.sha(extra.read_bytes())
                receipt["bootstrap_distributions"] = {"sneaky": "1"}
                (destination / "ready/installed.json").write_bytes(M.encoded(receipt))
            self.check(fixture, False)
            extra.unlink()
            payload.unlink()
            payload.symlink_to(fixture.bundle / "package.json")
            self.check(fixture, False)

    def test_unexpected_fifo_is_refused_without_opening(self):
        fixture, _ = self.fixture(False)
        extra = Path(fixture.args.destination) / "node_modules/wait-forever"
        os.mkfifo(extra)
        with patch.object(M, "readback_validator", return_value=M.encoded(fixture.plan)), self.assertRaises(M.InstallError):
            M.verify_existing_selected(fixture.args)

    def test_runtime_binding_is_checked_without_probing_version(self):
        fixture, _ = self.fixture(True)
        link = Path(fixture.args.destination) / "runtime/bin/python"
        link.unlink()
        link.symlink_to("/bin/sh")
        self.check(fixture, False)

    def test_selected_runtime_and_validator_link_loops_are_controlled_refusals(self):
        fixture, _ = self.fixture(False)
        loop = fixture.root / "loop"
        loop.symlink_to("loop")
        for key in ("node", "validator"):
            original = getattr(fixture.args, key)
            setattr(fixture.args, key, str(loop))
            self.check(fixture, False)
            setattr(fixture.args, key, original)

    def test_python_generated_hash_consistency_is_not_bootstrap_attestation(self):
        fixture, receipt = self.fixture(True)
        destination = Path(fixture.args.destination)
        (destination / "runtime/lib/python3.11/site-packages/pip/__init__.py").write_bytes(b"changed bootstrap, excluded")
        self.check(fixture)
        generated = next(iter(receipt["verification"]["installer_generated"]))
        (destination / "runtime" / generated).write_bytes(b"changed generated body")
        self.check(fixture, False)
        receipt["verification"]["installer_generated"][generated] = M.sha(b"changed generated body")
        (destination / "ready/installed.json").write_bytes(M.encoded(receipt))
        result = self.check(fixture)
        self.assertIn("not provenance", " ".join(result["excluded"]))

    def test_python_verifier_context_is_explicit(self):
        fixture, _ = self.fixture(True)
        with patch.object(M.sys, "version_info", (3, 12)):
            self.check(fixture, False)

    def test_malformed_python_receipt_subobjects_are_controlled_refusals(self):
        fixture, receipt = self.fixture(True)
        for key in ("verification", "installed_graph"):
            for value in (None, [], "not an object", 42):
                malformed = {**receipt, key: value}
                (Path(fixture.args.destination) / "ready/installed.json").write_bytes(M.encoded(malformed))
                self.check(fixture, False)


class ValidatorBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="toolkit-metadata-child-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()

    def validator(self, source):
        path = self.root / ("validator-" + str(len(list(self.root.iterdir()))))
        path.write_text("#!" + sys.executable + " -I\n" + source)
        path.chmod(0o700)
        return str(path)

    def test_bounded_output_nonzero_and_closed_stdin(self):
        ok = self.validator("import os,sys,json\nassert sys.stdin.read() == ''\nassert 'HOME' not in os.environ\nprint(json.dumps({'ok': True}))\n")
        self.assertEqual(json.loads(M.readback_validator([ok], self.root)), {"ok": True})
        for source, limit in (("import sys\nsys.stderr.write('private error')\nsys.exit(1)\n", 100),
                              ("print('x' * 5000)\n", 100)):
            with self.assertRaises(M.InstallError):
                M.readback_validator([self.validator(source)], self.root, limit=limit)
        self.assertTrue(all(path.name.startswith("validator-") for path in self.root.iterdir()))

    def test_cancellation_at_first_post_spawn_clock_read_joins_child(self):
        created = []
        popen = subprocess.Popen
        monotonic = time.monotonic
        def capture(*args, **kwargs):
            child = popen(*args, **kwargs)
            created.append(child)
            return child
        first = True
        def interrupt_once():
            nonlocal first
            if first:
                first = False
                raise KeyboardInterrupt
            return monotonic()
        with patch.object(M.subprocess, "Popen", side_effect=capture), patch.object(M.time, "monotonic", side_effect=interrupt_once):
            with self.assertRaises(KeyboardInterrupt):
                M.readback_validator([self.validator("import time\ntime.sleep(30)\n")], self.root)
        self.assertEqual(len(created), 1)
        self.assertIsNotNone(created[0].poll())
        self.assertTrue(created[0].stdout.closed)

    def test_signals_during_spawn_handoff_are_deferred_until_child_is_owned(self):
        for signum in (signal.SIGINT, signal.SIGTERM):
            created = []
            popen = subprocess.Popen
            prior = {sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)}
            def capture(*args, **kwargs):
                child = popen(*args, **kwargs)
                created.append(child)
                signal.raise_signal(signum)
                return child
            with patch.object(M.subprocess, "Popen", side_effect=capture), self.assertRaises(KeyboardInterrupt):
                M.readback_validator([self.validator("import time\ntime.sleep(30)\n")], self.root)
            self.assertEqual(len(created), 1)
            self.assertIsNotNone(created[0].poll())
            self.assertTrue(created[0].stdout.closed)
            self.assertEqual(prior, {sig: signal.getsignal(sig) for sig in prior})

    def test_spawn_failure_restores_caller_signal_handlers(self):
        prior = {sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)}
        with self.assertRaises(OSError):
            M.readback_validator([str(self.root / "absent-validator")], self.root)
        self.assertEqual(prior, {sig: signal.getsignal(sig) for sig in prior})

    def test_one_deadline_covers_open_pipe_after_leader_exit(self):
        marker = self.root / "pid"
        source = "import os,signal,time\npid=os.fork()\nif pid == 0:\n signal.signal(signal.SIGTERM,signal.SIG_IGN)\n time.sleep(30)\nelse:\n open(" + repr(str(marker)) + ",'w').write(str(pid))\n os._exit(0)\n"
        started = time.monotonic()
        with self.assertRaises(M.InstallError):
            M.readback_validator([self.validator(source)], self.root, timeout=0.3)
        self.assertLess(time.monotonic() - started, 3)
        pid = int(marker.read_text())
        # Orphan reaping belongs to init; it must no longer be a running process.
        result = subprocess.run(["/bin/ps", "-p", str(pid), "-o", "stat="], capture_output=True)
        self.assertTrue(result.returncode != 0 or result.stdout.strip().startswith(b"Z"))

    def test_deadline_also_covers_child_after_stdout_close(self):
        source = "import os,time\nos.close(1)\ntime.sleep(30)\n"
        with self.assertRaises((M.InstallError, subprocess.TimeoutExpired)):
            M.readback_validator([self.validator(source)], self.root, timeout=0.2)

    def test_public_cli_invalid_validator_outputs_are_redacted_without_destination_changes(self):
        fixtures = SelectedReadbackTests()
        fixture, _ = fixtures.fixture(False)
        self.addCleanup(fixtures.doCleanups)
        before = snapshot(Path(fixture.args.destination))
        for source in ("print('not-json private value')\n", "print('[]')\n",
                       "import sys\nsys.stderr.write('private error')\nsys.exit(1)\n",
                       "print('x' * (64 * 1024 * 1024 + 1))\n"):
            argv = [sys.executable, "-I", "-B", str(SCRIPT), "--selected-node", str(fixture.bundle),
                    "--selection-sha256", fixture.args.selection_sha256, "--component", fixture.args.component,
                    "--validator", self.validator(source), "--node", sys.executable,
                    "--destination", fixture.args.destination, "--verify-existing"]
            result = subprocess.run(argv, capture_output=True, timeout=5, env={"PATH": "/usr/bin:/bin"})
            self.assertEqual(result.returncode, 2)
            self.assertEqual(result.stdout, b"")
            self.assertEqual(result.stderr, b"existing installation verification failed; no installation or repair was performed\n")
            self.assertEqual(snapshot(Path(fixture.args.destination)), before)

    def test_public_cli_deeply_nested_receipt_fails_without_traceback(self):
        fixtures = SelectedReadbackTests()
        fixture, _ = fixtures.fixture(False)
        self.addCleanup(fixtures.doCleanups)
        (Path(fixture.args.destination) / "ready/installed.json").write_bytes(b'{"nested":' + b'[' * 2000 + b'0' + b']' * 2000 + b'}')
        before = snapshot(Path(fixture.args.destination))
        validator = self.validator("import sys\nsys.stdout.buffer.write(" + repr(M.encoded(fixture.plan)) + ")\n")
        argv = [sys.executable, "-I", "-B", str(SCRIPT), "--selected-node", str(fixture.bundle),
                "--selection-sha256", fixture.args.selection_sha256, "--component", fixture.args.component,
                "--validator", validator, "--node", sys.executable,
                "--destination", fixture.args.destination, "--verify-existing"]
        result = subprocess.run(argv, capture_output=True, timeout=5, env={"PATH": "/usr/bin:/bin"})
        self.assertEqual(result.returncode, 2)
        self.assertFalse(result.stdout)
        self.assertEqual(result.stderr, b"existing installation verification failed; no installation or repair was performed\n")
        self.assertEqual(snapshot(Path(fixture.args.destination)), before)

    def test_public_cli_cancellation_joins_validator_and_does_not_write_destination(self):
        fixtures = SelectedReadbackTests()
        fixture, _ = fixtures.fixture(False)
        self.addCleanup(fixtures.doCleanups)
        marker = self.root / "pid"
        source = "import os,signal,time\nsignal.signal(signal.SIGTERM,signal.SIG_IGN)\nopen(" + repr(str(marker)) + ",'w').write(str(os.getpid()))\ntime.sleep(30)\n"
        validator = self.validator(source)
        argv = [sys.executable, "-I", "-B", str(SCRIPT), "--selected-node", str(fixture.bundle), "--selection-sha256", fixture.args.selection_sha256,
                "--component", fixture.args.component, "--validator", validator, "--node", sys.executable,
                "--destination", fixture.args.destination, "--verify-existing"]
        before = snapshot(Path(fixture.args.destination))
        child = subprocess.Popen(argv, env={"PATH": "/usr/bin:/bin"}, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 5
            while not marker.exists():
                if child.poll() is not None or time.monotonic() > deadline:
                    self.fail("validator never started")
                time.sleep(0.01)
            pid = int(marker.read_text())
            child.terminate()
            time.sleep(0.1)
            child.send_signal(signal.SIGINT)
            out, err = child.communicate(timeout=5)
            self.assertEqual(child.returncode, 2)
            self.assertFalse(out)
            self.assertIn(b"no installation or repair", err)
            self.assertNotIn(b"Traceback", err)
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)
            self.assertEqual(snapshot(Path(fixture.args.destination)), before)
        finally:
            if child.poll() is None:
                child.kill()
                child.communicate()


if __name__ == "__main__":
    unittest.main()
