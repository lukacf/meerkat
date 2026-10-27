"""Configured-library boundaries; ordinary package installation is root-probed."""
from __future__ import annotations

import argparse
from contextlib import ExitStack, redirect_stderr, redirect_stdout
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

SCRIPT = Path(__file__).with_name("install-library-consumer.py").resolve()
SPEC = importlib.util.spec_from_file_location("configured_library_install", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
ENV = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"}


class ConfiguredLibraryTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="toolkit-configured-install-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.bundle = self.root / "bundle"
        self.bundle.mkdir()
        self.config = self.root / "host-selected.json"
        self.raw = b'{"profile":"slack","config":{"token_env":"FIRST_ACCOUNT"}}\n'
        self.config.write_bytes(self.raw)
        self.args = argparse.Namespace(bundle=str(self.bundle), contents_sha256="a" * 64,
            family="services", destination=str(self.root / "new"), python=sys.executable,
            node=sys.executable, npm_cli=sys.executable, config=str(self.config),
            config_sha256=MODULE.sha(self.raw), server_name="selected-service")

    def assert_refused_before_effects(self):
        with patch.object(MODULE, "inputs") as inputs, patch.object(MODULE, "run") as run:
            with self.assertRaises((ValueError, OSError)):
                MODULE.install(self.args)
            inputs.assert_not_called()
            run.assert_not_called()
        self.assertFalse(Path(self.args.destination).exists())

    def fake_install(self, action=None):
        """Exercise orchestration; fixture installation is not artifact evidence."""
        family = self.args.family
        namespace = MODULE.FAMILIES[family][0] if family != "activepieces" else None
        package = MODULE.name(namespace) if namespace else None
        filename = namespace + "-0.1.0-py3-none-any.whl" if namespace else None
        files = {filename: b"inert archive placeholder"} if namespace else {
            "package.json": b"{}", "package-lock.json": b'{"packages":{}}',
            "rkat-toolkit-activepieces-0.1.0.tgz": b"inert archive placeholder"}
        calls = []

        def child(argv, cwd, env, label, timeout=900, **options):
            calls.append((argv, label, options))
            self.assertNotIn("HOME", env)
            self.assertNotIn("CODEX_HOME", env)
            self.assertNotIn("FIRST_ACCOUNT", env)
            if label == "python-runtime":
                return MODULE.encoded(["fixture", [3, 11], "darwin", "arm64", "cpython"])
            if label == "node-runtime":
                return MODULE.encoded(["v24.19.0", "darwin", "arm64"])
            if label in ("bootstrap-python", "installed-python"):
                graph = [["pip", "1"]]
                if label == "installed-python":
                    graph.append([package, "0.1.0"])
                return MODULE.encoded({"distributions": graph,
                    "site": str(cwd / "runtime/lib/python3.11/site-packages")})
            if label == "profiles":
                return b'{"profiles":[{"profile":"fixture"}]}'
            if label == "check-config":
                self.assertEqual(options, {"discard_output": True})
                self.assertEqual(argv[-3:], ["check-config", "--config", str(cwd / "configuration/selected.json")])
                self.assertEqual((cwd / "configuration/selected.json").read_bytes(), self.raw)
                self.assertFalse((cwd / "ready").exists())
                self.assertFalse((cwd / "installed.json").exists())
                if action:
                    action(cwd)
                # Deliberately not valid JSON. Only the installed validator's
                # exit status matters; its response values are not projected.
                return b"PRIVATE_ACCOUNT_AND_URL_FROM_VALIDATOR"
            return b""

        with ExitStack() as stack:
            stack.enter_context(patch.object(MODULE, "inputs", return_value=(files, None)))
            stack.enter_context(patch.object(MODULE, "python_closure", return_value={package: "0.1.0"}))
            stack.enter_context(patch.object(MODULE, "npm_closure", return_value=[]))
            stack.enter_context(patch.object(MODULE, "verify_wheel_payload", return_value={"payloads": 1}))
            stack.enter_context(patch.object(MODULE, "verify_npm_payload", return_value=1))
            stack.enter_context(patch.object(MODULE, "run", side_effect=child))
            return MODULE.install(self.args), calls

    def test_missing_fields_bad_digest_and_server_name_refuse_before_effects(self):
        original = vars(self.args).copy()
        for key, value in (("config", None), ("config_sha256", None), ("server_name", None),
                           ("config_sha256", "b" * 64), ("config_sha256", "not-a-digest"),
                           ("server_name", ""), ("server_name", "bad/name"),
                           ("server_name", "x" * 129), ("server_name", "__proto__")):
            with self.subTest(key=key, value=value):
                vars(self.args).update(original)
                setattr(self.args, key, value)
                self.assert_refused_before_effects()

    def test_relative_dot_parent_and_normalized_alias_paths_refuse(self):
        for value in ("host-selected.json", str(self.root) + "/./host-selected.json",
                      str(self.root) + "/child/../host-selected.json",
                      str(self.root) + "//host-selected.json"):
            with self.subTest(path=value):
                self.args.config = value
                self.assert_refused_before_effects()

    def test_no_follow_on_final_and_parent_symlinks(self):
        final = self.root / "alias.json"
        final.symlink_to(self.config)
        parent = self.root / "alias-directory"
        parent.symlink_to(self.root, target_is_directory=True)
        for path in (final, parent / self.config.name):
            with self.subTest(path=str(path)):
                self.args.config = str(path)
                self.assert_refused_before_effects()

    def test_fifo_and_oversized_configuration_refuse_without_child(self):
        fifo = self.root / "fifo.json"
        os.mkfifo(fifo)
        self.args.config = str(fifo)
        self.assert_refused_before_effects()
        self.args.config = str(self.config)
        self.raw = b" " * (MODULE.MAX_CONFIGURATION + 1)
        self.config.write_bytes(self.raw)
        self.args.config_sha256 = MODULE.sha(self.raw)
        self.assert_refused_before_effects()

    def test_explicit_parentheses_filename_is_read_as_literal(self):
        selected = self.root / "config (selected).json"
        selected.write_bytes(self.raw)
        self.args.config = str(selected)
        self.assertEqual(MODULE.selected_configuration(self.args), self.raw)

    def test_each_family_publishes_only_validated_exact_serve_command(self):
        for family in ("agno", "services", "activepieces"):
            with self.subTest(family=family):
                self.args.family = family
                self.args.destination = str(self.root / family)
                receipt, calls = self.fake_install()
                destination = Path(self.args.destination)
                mcp = json.loads((destination / "ready/mcp.json").read_bytes())
                installed = json.loads((destination / "ready/installed.json").read_bytes())
                self.assertEqual(installed, receipt)
                self.assertEqual(receipt["configuration"]["sha256"], MODULE.sha(self.raw))
                self.assertEqual(receipt["configuration"]["validation"], "configuration_only")
                self.assertEqual(receipt["scope"], "observed_software_installation_only")
                entry = mcp["mcpServers"]["selected-service"]
                self.assertEqual(set(entry), {"command", "args"})
                self.assertEqual([entry["command"], *entry["args"]], receipt["command"])
                self.assertEqual(receipt["command"][-3:], ["serve", "--config", str(destination / "configuration/selected.json")])
                if family == "activepieces":
                    self.assertEqual(receipt["command"][1], str(destination / "node_modules/@rkat/toolkit-activepieces/dist/mcp-cli.js"))
                else:
                    self.assertEqual(receipt["command"][1:4], ["-I", "-m", MODULE.FAMILIES[family][1]])
                self.assertEqual(calls[-1][1], "check-config")
                self.assertFalse((destination / "installed.json").exists())
                self.assertFalse((destination / "pending-ready").exists())
                self.assertNotIn(b"FIRST_ACCOUNT", MODULE.encoded(receipt))
                self.assertNotIn(b"PRIVATE_ACCOUNT", MODULE.encoded(receipt))

    def test_source_mutation_cannot_change_frozen_selection(self):
        receipt, _ = self.fake_install(lambda cwd: self.config.write_bytes(b"changed after freeze"))
        self.assertEqual(Path(receipt["configuration"]["path"]).read_bytes(), self.raw)
        self.assertEqual(receipt["configuration"]["sha256"], MODULE.sha(self.raw))

    def test_changed_frozen_configuration_refuses_publication(self):
        def mutate(cwd):
            (cwd / "configuration/selected.json").write_bytes(b"different")
        with self.assertRaises(MODULE.InstallError):
            self.fake_install(mutate)
        destination = Path(self.args.destination)
        self.assertFalse((destination / "ready").exists())
        self.assertFalse((destination / "installed.json").exists())

    def test_failed_and_cancelled_validation_leave_no_ready_or_success_receipt(self):
        for index, error in enumerate((MODULE.InstallError("validator refused"), KeyboardInterrupt())):
            self.args.destination = str(self.root / ("failed-" + str(index)))
            def fail(cwd):
                raise error
            with self.subTest(error=type(error).__name__), self.assertRaises(type(error)):
                self.fake_install(fail)
            destination = Path(self.args.destination)
            self.assertFalse((destination / "ready").exists())
            self.assertFalse((destination / "installed.json").exists())

    def test_interrupted_reply_after_publication_keeps_observed_result(self):
        rename = Path.rename

        def publish_then_interrupt(path, target):
            result = rename(path, target)
            if path.name == "pending-ready":
                raise KeyboardInterrupt()
            return result

        with patch.object(Path, "rename", publish_then_interrupt), self.assertRaises(KeyboardInterrupt):
            self.fake_install()
        ready = Path(self.args.destination) / "ready"
        receipt = json.loads((ready / "installed.json").read_bytes())
        self.assertEqual(json.loads((ready / "mcp.json").read_bytes()), receipt["mcp_configuration"])
        self.assertEqual(receipt["configuration"]["sha256"], MODULE.sha(self.raw))
        self.assertFalse((Path(self.args.destination) / "installed.json").exists())
        before = {p.name: p.read_bytes() for p in ready.iterdir()}
        # Exercise the public entrypoint after the committed effect. It cannot
        # infer a failed installation from a missing completion response.
        argv = [str(SCRIPT), "--bundle", str(self.bundle), "--contents-sha256", "a" * 64,
                "--family", "services", "--destination", self.args.destination, "--python", sys.executable]
        self.addCleanup(signal.signal, signal.SIGTERM, signal.getsignal(signal.SIGTERM))
        stdout, stderr = io.StringIO(), io.StringIO()
        with patch.object(sys, "argv", argv), patch.object(MODULE, "install", side_effect=KeyboardInterrupt), \
                redirect_stdout(stdout), redirect_stderr(stderr):
            self.assertEqual(MODULE.main(), 2)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("installation status unconfirmed", stderr.getvalue())
        self.assertIn("ready/installed.json", stderr.getvalue())
        self.assertEqual(before, {p.name: p.read_bytes() for p in ready.iterdir()})

    def test_separate_configs_remain_isolated_and_existing_install_untouched(self):
        first, _ = self.fake_install()
        old = Path(self.args.destination)
        before = {str(p.relative_to(old)): p.read_bytes() for p in old.rglob("*") if p.is_file()}
        self.raw = b'{"profile":"gmail","config":{"token_env":"SECOND_ACCOUNT"}}\n'
        self.config.write_bytes(self.raw)
        self.args.config_sha256 = MODULE.sha(self.raw)
        self.args.server_name = "second-service"
        self.assert_refused_before_effects_existing()
        self.args.destination = str(self.root / "second")
        second, _ = self.fake_install()
        self.assertNotEqual(first["configuration"]["sha256"], second["configuration"]["sha256"])
        self.assertNotEqual(first["configuration"]["path"], second["configuration"]["path"])
        self.assertEqual(before, {str(p.relative_to(old)): p.read_bytes() for p in old.rglob("*") if p.is_file()})

    def assert_refused_before_effects_existing(self):
        with patch.object(MODULE, "inputs") as inputs, patch.object(MODULE, "run") as run:
            with self.assertRaises(MODULE.InstallError):
                MODULE.install(self.args)
            inputs.assert_not_called()
            run.assert_not_called()

    def test_unconfigured_library_keeps_existing_output_contract(self):
        self.args.config = self.args.config_sha256 = self.args.server_name = None
        receipt, calls = self.fake_install()
        destination = Path(self.args.destination)
        self.assertEqual(json.loads((destination / "installed.json").read_bytes()), receipt)
        self.assertNotIn("configuration", receipt)
        self.assertFalse((destination / "configuration").exists())
        self.assertFalse((destination / "ready").exists())
        self.assertNotIn("check-config", [row[1] for row in calls])

    def test_public_cli_all_or_none_and_selected_mode_conflicts(self):
        config = ["--config", str(self.config), "--config-sha256", self.args.config_sha256,
                  "--server-name", "selected-service"]
        base = [sys.executable, "-I", "-B", str(SCRIPT), "--destination", self.args.destination]
        cases = [(["--bundle", str(self.bundle), "--family", "services", "--contents-sha256", "a" * 64] + config[:n])
                 for n in (2, 4)]
        cases += [[mode, str(self.bundle)] + config for mode in ("--selected-node", "--selected-python")]
        cases.append(["--bundle", str(self.bundle), "--conf", str(self.config)])
        for argv in cases:
            with self.subTest(argv=argv):
                result = subprocess.run(base + argv, env=ENV, capture_output=True, timeout=5)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, b"")
                self.assertNotIn(b"Traceback", result.stderr)
                self.assertFalse(Path(self.args.destination).exists())

    def test_validator_stdout_and_stderr_are_not_buffered_or_retained(self):
        code = "import sys; print('PRIVATE_ACCOUNT'); print('PRIVATE_URL',file=sys.stderr); sys.exit(int(sys.argv[1]))"
        for status in (0, 1):
            with self.subTest(status=status):
                if status:
                    with self.assertRaises(MODULE.InstallError):
                        MODULE.run([sys.executable, "-I", "-B", "-c", code, str(status)], self.root, ENV,
                                   "check-config", discard_output=True)
                else:
                    self.assertEqual(MODULE.run([sys.executable, "-I", "-B", "-c", code, str(status)],
                        self.root, ENV, "check-config", discard_output=True), b"")
                self.assertFalse((self.root / "check-config.stdout").exists())
                self.assertFalse((self.root / "check-config.stderr").exists())

    def test_repeated_cancellation_joins_suppressed_validator(self):
        marker = self.root / "validator.pid"
        code = ("import os,pathlib,signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); "
                "pathlib.Path(" + repr(str(marker)) + ").write_text(str(os.getpid())); "
                "print('PRIVATE_ACCOUNT',flush=True); time.sleep(30)")
        controller = self.root / "controller.py"
        controller.write_text("import importlib.util,pathlib,signal\n"
            + "s=importlib.util.spec_from_file_location('installer'," + repr(str(SCRIPT)) + ")\n"
            + "m=importlib.util.module_from_spec(s); s.loader.exec_module(m)\n"
            + "def stop(sig,frame): raise KeyboardInterrupt\n"
            + "signal.signal(signal.SIGTERM,stop)\n"
            + "m.run(" + repr([sys.executable, "-I", "-B", "-c", code]) + ",pathlib.Path("
            + repr(str(self.root)) + ")," + repr(ENV) + ",'check-config',discard_output=True)\n")
        child = subprocess.Popen([sys.executable, "-I", "-B", str(controller)], env=ENV,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            deadline = time.monotonic() + 5
            while not marker.exists() or not marker.read_text():
                if child.poll() is not None or time.monotonic() > deadline:
                    self.fail("validator did not start")
                time.sleep(0.02)
            pid = int(marker.read_text())
            child.terminate()
            time.sleep(0.1)
            child.send_signal(signal.SIGINT)
            self.assertNotEqual(child.wait(timeout=5), 0)
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)
            self.assertFalse((self.root / "check-config.stdout").exists())
            self.assertFalse((self.root / "check-config.stderr").exists())
            self.assertFalse(Path(self.args.destination).exists())
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()


if __name__ == "__main__":
    unittest.main()
