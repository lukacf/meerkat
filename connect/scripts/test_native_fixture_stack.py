#!/usr/bin/env python3
"""Exercise native fixture launchers across the real environment isolation boundary."""
from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SCRIPTS = Path(__file__).resolve().parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


LATE = load("stack_late_fixture", SCRIPTS / "fixtures/late_driver_adoption.py")
CONSENT = load("stack_consent_fixture", SCRIPTS / "fixtures/registered_consent_transfer.py")
HARNESS = load("stack_native_harness", SCRIPTS / "verify_native_gateway_subprocess.py")


class NativeFixtureStackTests(unittest.TestCase):
    def test_isolated_exec_retains_stack_and_protocol(self):
        python = Path(sys.executable).resolve()
        cases = (
            ("late-fenced", "late_adoption_fences_actual_acquisition_before_native_ready_and_joins", "fenced"),
            ("late-unfenced", "late_adoption_fences_actual_acquisition_before_native_ready_and_joins", "unfenced"),
            ("drain", "nonzero_drain_refuses_new_helper_and_completes_entered_console_turn", "drain"),
            ("consent", CONSENT.SELECTOR, None),
            ("requeue", CONSENT.REQUEUE_SELECTOR, None),
        )
        for name, selector, fence in cases:
            with self.subTest(case=name), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                if fence is not None:
                    selector = "application_driver::late_adoption_tests::" + selector
                prefix = "CONNECT_LATE_ADOPTION_TEST" if fence is not None else "CONNECT_CONSENT_TRANSFER"
                binary = root / "probe"
                binary.write_text(
                    "#!" + str(python) + "\n"
                    "import json, os, sys\n"
                    "prefix = " + repr(prefix) + "\n"
                    "print('simulated libtest header', flush=True)\n"
                    "payload = {'stack': os.environ.get('RUST_MIN_STACK'), 'pid': os.getpid(),\n"
                    "           'argv': sys.argv[1:], 'startup': json.loads(os.environ[prefix + '_CHILD']),\n"
                    "           'ambient': os.environ.get('CONNECT_STACK_TEST_AMBIENT')}\n"
                    "os.write(int(os.environ[prefix + '_STDOUT']), (json.dumps(payload) + '\\n').encode())\n"
                )
                binary.chmod(0o700)
                if fence is not None:
                    source = LATE.wrapper(python, binary, selector, root, fence)
                else:
                    source = CONSENT.wrapper(python, binary, root, "http://127.0.0.1:1",
                                             selector=selector, requeue=name == "requeue")
                launcher = root / "launcher"
                launcher.write_text(source)
                launcher.chmod(0o700)
                with patch.dict(os.environ, {"RUST_MIN_STACK": "1", "CONNECT_STACK_TEST_AMBIENT": "synthetic"}):
                    environment = HARNESS.process_env(root)
                self.assertNotIn("RUST_MIN_STACK", environment)
                self.assertNotIn("CONNECT_STACK_TEST_AMBIENT", environment)
                startup = ["--persistent", "--host-control-read-fd", "7", "--host-control-write-fd", "8"]
                with subprocess.Popen([str(launcher), *startup], env=environment,
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as process:
                    try:
                        stdout, stderr = process.communicate(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.communicate()
                        self.fail("fixture launcher did not exit")
                    self.assertEqual(process.returncode, 0, stderr)
                    actual = json.loads(stdout)
                    self.assertEqual(actual["stack"], "33554432")
                    self.assertEqual(actual["pid"], process.pid)
                self.assertEqual(actual["startup"], startup)
                self.assertEqual(actual["argv"], ["--exact", selector, "--ignored", "--quiet",
                                                "--nocapture", "--test-threads=1"])
                self.assertIsNone(actual["ambient"])


if __name__ == "__main__":
    unittest.main()
