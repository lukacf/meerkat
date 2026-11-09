"""Fixed canonical refusals survive real pipes without exposing child output."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from test_selected_setup import SelectedFixture, SelectedRunner, adapter, backend


class ProcessRunner(backend.Runner):
    block = None


class DiagnosticFixture(SelectedFixture):
    def validator(self, script):
        for name in ("python", "node"):
            path = Path(self.tools[name]["path"])
            path.write_text("#!" + sys.executable + "\n" + script + "\n")
            self.profile["validators"][name]["sha256"] = backend.digest(path.read_bytes())
        self.write_profile()

    def controller(self, **kwargs):
        kwargs.setdefault("runner_factory", ProcessRunner)
        return super().controller(**kwargs)


class DiagnosticTests(DiagnosticFixture, unittest.TestCase):
    def test_exact_profile_refusals_and_unknown_output(self):
        for profile in ("python", "node"):
            for reason in ("invalid_package", "invalid_selection", "unsupported_" + profile + "_selection"):
                with self.subTest(profile=profile, reason=reason):
                    self.validator("import sys; sys.stderr.write(" + repr(reason + "\n") + "); sys.exit(1)")
                    c = self.controller()
                    request = self.stage(profile, "valid" + str(self.sequence))
                    with self.assertRaises(backend.SetupError) as caught:
                        c.selected_check(request)
                    self.assertEqual(caught.exception.code, "selected_" + reason)
                    public = backend.setup_error_payload(caught.exception)
                    self.assertEqual(public["diagnostic"]["code"], reason)
                    self.assertEqual(public["diagnostic"]["phase"], "before_installation")
                    self.assertEqual((len(c.jobs), len(c.validations)), (0, 0))
                    self.assertTrue(c.operation_lock.acquire(False)); c.operation_lock.release()
        bad = [b"invalid_package", b"invalid_package\r\n", b" invalid_package\n",
               b"invalid_package\n\n", b"invalid_package\n/private/secret\n", b"\xff\n",
               b"input_unavailable\n", b"unsupported_python_selection\n", b"x" * 129]
        for raw in bad:
            with self.subTest(raw=raw):
                self.validator("import os; os.write(2, " + repr(raw) + "); raise SystemExit(1)")
                c = self.controller()
                with self.assertRaises(backend.SetupError) as caught:
                    c.selected_check(self.stage("node", "bad" + str(self.sequence)))
                self.assertEqual(backend.setup_error_payload(caught.exception), {"error": "selected_validation_unavailable"})

    def test_success_mixed_output_other_exit_and_signal_never_diagnose(self):
        scripts = [
            "import sys; sys.stderr.write('invalid_package\\n')",
            "import sys; sys.stderr.write('invalid_package\\n'); sys.exit(2)",
            "import os, signal; os.write(2,b'invalid_package\\n'); os.kill(os.getpid(), signal.SIGTERM)",
            "import sys; sys.stdout.write('private stdout'); sys.stderr.write('invalid_package\\n'); sys.exit(1)",
        ]
        for script in scripts:
            with self.subTest(script=script):
                self.validator(script); c = self.controller()
                with self.assertRaises(backend.SetupError) as caught:
                    c.selected_check(self.stage(name="mixed" + str(self.sequence)))
                self.assertEqual(caught.exception.code, "selected_validation_unavailable")

    def test_frozen_metadata_and_tool_mutation_prevent_attribution(self):
        for target in ("package.json", "activation.json", "tool"):
            with self.subTest(target=target):
                suffix = ("p=__file__" if target == "tool" else
                          "p=sys.argv[sys.argv.index(" + repr("--package" if target == "package.json" else "--selection") + ")+1]")
                self.validator("import os,sys\n" + suffix + "\nos.chmod(p,0o700)\nopen(p,'ab').write(b'changed')\nsys.stderr.write('invalid_package\\n')\nsys.exit(1)")
                c = self.controller()
                with self.assertRaises(backend.SetupError) as caught:
                    c.selected_check(self.stage(name="changed" + str(self.sequence)))
                self.assertEqual(caught.exception.code, "selected_validation_unavailable")

    def test_success_and_legacy_custom_runner_stay_compatible(self):
        self.validator("import json,sys; print(json.dumps(json.load(open(sys.argv[sys.argv.index('--package')+1]))['fixture_plan']))")
        c = self.controller()
        self.assertEqual(c.selected_check(self.stage())["profile"], "python")
        class LegacyFailure(SelectedRunner):
            def run(self, argv, capture=False):
                self.calls.append((argv, capture))
                return 1, b""
        c = self.controller(runner_factory=LegacyFailure)
        with self.assertRaises(backend.SetupError) as caught:
            c.selected_check(self.stage(name="legacy"))
        self.assertEqual(caught.exception.code, "selected_validation_unavailable")
        self.assertEqual(len(c.runner.calls), 1)


class PipeTests(unittest.TestCase):
    def test_both_eof_orders_and_ordinary_run_return_shape(self):
        with tempfile.TemporaryDirectory() as temporary:
            runner = backend.Runner(Path(temporary))
            for script, expected in (
                ("import os,time; os.close(1); time.sleep(.03); os.write(2,b'invalid_package\\n'); raise SystemExit(1)", (1, b"", b"invalid_package\n")),
                ("import os,time; os.close(2); time.sleep(.03); os.write(1,b'{}')", (0, b"{}", b"")),
            ):
                self.assertEqual(runner.run_selected_validator([sys.executable, "-c", script]), expected)
            self.assertEqual(runner.run([sys.executable, "-c", "print('{}')"], capture=True), (0, b"{}\n"))
            self.assertFalse(hasattr(runner._diagnostic_context, "stderr"))

    def test_overflow_and_timeout_join_child_and_restore_capture(self):
        real_popen = subprocess.Popen
        children = []
        def tracked(*args, **kwargs):
            child = real_popen(*args, **kwargs); children.append(child); return child
        with tempfile.TemporaryDirectory() as temporary, patch.object(backend.subprocess, "Popen", tracked):
            runner = backend.Runner(Path(temporary))
            for script in ("import os,time; os.write(2,b'x'*129); time.sleep(30)",
                           "import os,time; os.close(1); time.sleep(30)",
                           "import os,time; os.write(1,b'x'*129); time.sleep(30)"):
                with self.subTest(script=script), patch.object(backend, "INSPECTION_TIMEOUT", .15), patch.object(backend, "MAX_JSON", 128):
                    with self.assertRaises(backend.SetupError) as caught:
                        runner.run_selected_validator([sys.executable, "-c", script])
                    self.assertEqual(caught.exception.code, "inspection_unavailable")
                    self.assertIsNotNone(children[-1].returncode)
                    self.assertTrue(children[-1].stdout.closed)
                    self.assertTrue(children[-1].stderr.closed)
                    self.assertFalse(hasattr(runner._diagnostic_context, "stderr"))

    def test_subclass_override_and_nested_capture_are_not_bypassed(self):
        class Custom(backend.Runner):
            def run(self, argv, capture=False):
                self.calls += 1
                if self.calls == 1:
                    return self.run_selected_validator(argv)
                return 1, b""
        with tempfile.TemporaryDirectory() as temporary:
            runner = Custom(Path(temporary)); runner.calls = 0
            with patch.object(backend.subprocess, "Popen", side_effect=AssertionError("unexpected spawn")):
                with self.assertRaises(backend.SetupError):
                    runner.run_selected_validator(["inert"])
                self.assertFalse(hasattr(runner._diagnostic_context, "stderr"))
                self.assertEqual(runner.run_selected_validator(["inert"]), (1, b"", b""))
                self.assertEqual(runner.calls, 2)


class DiagnosticMcpTests(DiagnosticFixture, unittest.IsolatedAsyncioTestCase):
    async def test_correction_guidance_is_fixed_and_next_check_can_succeed(self):
        from mcp import Client
        self.validator("import json,sys\np=json.load(open(sys.argv[sys.argv.index('--package')+1]))\nif 'fixture_plan' not in p:\n sys.stderr.write('invalid_package\\n'); sys.exit(1)\nprint(json.dumps(p['fixture_plan']))")
        c = self.controller(); request = self.stage("node")
        metadata = self.staging / "choice/package.json"
        valid = metadata.read_bytes()
        metadata.write_bytes(b'{"private-input":"not returned"}')
        request["package_sha256"] = backend.digest(metadata.read_bytes())
        owner = adapter.SetupAdapter(c)
        try:
            async with Client(adapter.create_server(owner)) as client:
                refused = await client.call_tool("setup_selected_check", request)
                self.assertTrue(refused.is_error)
                self.assertEqual(refused.structured_content, backend.setup_error_payload(backend.SetupError("selected_invalid_package")))
                metadata.write_bytes(valid); request["package_sha256"] = backend.digest(valid)
                checked = await client.call_tool("setup_selected_check", request)
                self.assertFalse(checked.is_error, checked)
                self.assertEqual(checked.structured_content["profile"], "node")
                self.assertEqual(len(c.jobs), 0)
        finally:
            await owner.aclose()


if __name__ == "__main__":
    unittest.main()
