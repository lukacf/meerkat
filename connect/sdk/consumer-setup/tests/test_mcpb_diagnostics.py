"""MCPB check refusals cross real pipes as fixed, input-bound guidance.

The small executable fixtures stand in for the pinned canonical checker. These
tests establish diagnostic custody and repair through MCP, not ZIP validity,
canonical metadata semantics, installation readiness or live authentication.
"""
import json
from pathlib import Path
import subprocess
import sys
import time
import unittest
from unittest.mock import patch

from test_selected_mcpb import McpbFixture, McpbRunner, adapter, backend


REASONS = (
    "invalid_document", "invalid_package", "invalid_selection",
    "configuration_selection_mismatch", "unsupported_selection",
    "unsupported_mcpb_launch",
)
SUFFIX = ": no readiness claim; inspect the selected destination before any new attempt\n"
PRIVATE = "PRIVATE_CHECKER_DETAIL_not_for_the_agent"


class PipeRunner(backend.Runner):
    block = None

    def __init__(self, workspace):
        super().__init__(workspace)
        self.calls = []

    def run(self, argv, **kwargs):
        self.calls.append((list(argv), dict(kwargs)))
        return super().run(argv, **kwargs)


class DiagnosticFixture(McpbFixture):
    def validator(self, script):
        path = Path(self.tools["mcpb"]["path"])
        path.write_text("#!" + sys.executable + "\n" + script + "\n")
        self.profile["validators"]["mcpb"]["sha256"] = backend.digest(path.read_bytes())
        self.write_profile()

    def refusal(self, raw, *, stdout=b"", status=1):
        self.validator("import os\nos.write(1, " + repr(stdout) + ")\n"
                       "os.write(2, " + repr(raw) + ")\nraise SystemExit(" + repr(status) + ")")

    def controller(self, **kwargs):
        kwargs.setdefault("runner_factory", PipeRunner)
        return super().controller(**kwargs)

    def request(self):
        return self.stage(name="case" + str(self.sequence))

    def check_error(self, controller, request):
        with self.assertRaises(backend.SetupError) as caught:
            controller.selected_check(request)
        self.assertEqual(controller.validations, {})
        self.assertEqual(controller.jobs, {})
        self.assertIsNone(controller.job)
        self.assertTrue(controller.operation_lock.acquire(False))
        controller.operation_lock.release()
        return backend.setup_error_payload(caught.exception)

    def assert_fixed(self, value, reason):
        self.assertEqual(set(value), {"error", "diagnostic"})
        self.assertEqual(value["error"], "selected_mcpb_" + reason)
        diagnostic = value["diagnostic"]
        self.assertEqual(set(diagnostic), {"phase", "profile", "code", "message"})
        self.assertEqual((diagnostic["phase"], diagnostic["profile"], diagnostic["code"]),
                         ("before_installation", "mcpb", reason))
        self.assertIn("failed check may retain prepared files", diagnostic["message"])
        self.assertIn("no installation has been requested", diagnostic["message"])
        self.assertNotIn(PRIVATE, json.dumps(value))
        self.assertNotIn(str(self.root), json.dumps(value))


class McpbDiagnosticTests(DiagnosticFixture, unittest.TestCase):
    def test_six_exact_canonical_refusals_have_fixed_profile_specific_guidance(self):
        for reason in REASONS:
            with self.subTest(reason=reason):
                self.refusal((reason + SUFFIX).encode("ascii"))
                controller = self.controller()
                self.assert_fixed(self.check_error(controller, self.request()), reason)
                self.assertEqual(len(controller.runner.calls), 1)
                self.assertEqual(controller.runner.calls[0][0][1], "check")
                self.assertEqual(controller.runner.calls[0][1]["inspection_timeout"], 120.0)
                self.assertFalse(hasattr(controller.runner._diagnostic_context, "stderr"))

    def test_unknown_modified_private_and_overlong_stderr_are_never_projected(self):
        exact = ("invalid_package" + SUFFIX).encode("ascii")
        refused = (
            b"invalid_package\n", exact[:-1], exact.replace(b"\n", b"\r\n"),
            b" " + exact, exact + b"\n", exact + PRIVATE.encode(),
            exact.replace(b"invalid_package", b"invalid_package_extra"),
            b"unknown_code" + SUFFIX.encode(), b"unsupported_python_selection" + SUFFIX.encode(),
            b"unsupported_node_selection" + SUFFIX.encode(), b"\xff" + exact,
            exact.replace(b":", b"\x00:", 1), exact + exact, b"x" * 129,
        )
        for index, raw in enumerate(refused):
            with self.subTest(case=index):
                self.refusal(raw)
                controller = self.controller()
                self.assertEqual(self.check_error(controller, self.request()),
                                 {"error": "selected_validation_unavailable"})
                self.assertFalse(hasattr(controller.runner._diagnostic_context, "stderr"))

    def test_success_other_exit_signal_and_mixed_stdout_are_not_canonical_refusals(self):
        exact = ("invalid_package" + SUFFIX).encode("ascii")
        scripts = [
            "import os; os.write(2, " + repr(exact) + "); raise SystemExit(0)",
            "import os; os.write(2, " + repr(exact) + "); raise SystemExit(2)",
            "import os,signal; os.write(2, " + repr(exact) + "); os.kill(os.getpid(),signal.SIGTERM)",
            "import os; os.write(1, " + repr(PRIVATE.encode()) + "); os.write(2, " + repr(exact) + "); raise SystemExit(1)",
            "import os; os.write(1,b' '); os.write(2, " + repr(exact) + "); raise SystemExit(1)",
        ]
        for index, script in enumerate(scripts):
            with self.subTest(case=index):
                self.validator(script)
                controller = self.controller()
                self.assertEqual(self.check_error(controller, self.request()),
                                 {"error": "selected_validation_unavailable"})

    def test_all_four_frozen_inputs_and_checker_changes_block_attribution(self):
        for filename in ("package.json", "activation.json", "configuration.json", "artifact.mcpb", "tool"):
            with self.subTest(filename=filename):
                path_expr = ("Path(__file__)" if filename == "tool" else
                             "Path(sys.argv[sys.argv.index('--package')+1]).parent / " + repr(filename))
                self.validator("import os,sys\nfrom pathlib import Path\np=" + path_expr +
                               "\nos.chmod(p,0o700)\np.write_bytes(p.read_bytes()+b'changed')\n"
                               "os.write(2," + repr(("invalid_package" + SUFFIX).encode()) + ")\nraise SystemExit(1)")
                controller = self.controller()
                self.assertEqual(self.check_error(controller, self.request()),
                                 {"error": "selected_validation_unavailable"})
                changed = (controller.workspace / "selected-tools/mcpb" if filename == "tool" else
                           next((controller.workspace / "selections").iterdir()) / filename)
                self.assertTrue(changed.read_bytes().endswith(b"changed"))

    def test_changed_original_stage_does_not_replace_the_frozen_diagnostic_identity(self):
        staged = self.staging / "case1/package.json"
        self.validator("import os\nfrom pathlib import Path\nPath(" + repr(str(staged)) +
                       ").write_bytes(b'original source changed after freezing')\nos.write(2," +
                       repr(("invalid_package" + SUFFIX).encode()) + ")\nraise SystemExit(1)")
        controller = self.controller()
        request = self.request()
        original = staged.read_bytes()
        self.assert_fixed(self.check_error(controller, request), "invalid_package")
        frozen = next((controller.workspace / "selections").iterdir()) / "package.json"
        self.assertEqual(frozen.read_bytes(), original)
        self.assertNotEqual(staged.read_bytes(), original)

    def test_archive_uses_payload_bound_instead_of_smaller_metadata_bound(self):
        self.refusal(("invalid_package" + SUFFIX).encode())
        controller = self.controller()
        request = self.request()
        artifact = self.staging / request["staged_id"] / "artifact.mcpb"
        payload = b"x" * (backend.SELECTED_DOCUMENT + 1)
        self.assertLess(len(payload), backend.SELECTED_FILE)
        artifact.write_bytes(payload)
        self.assert_fixed(self.check_error(controller, request), "invalid_package")
        frozen = next((controller.workspace / "selections").iterdir()) / "artifact.mcpb"
        self.assertEqual(frozen.read_bytes(), payload)

    def test_prelaunch_source_or_checker_drift_is_not_a_checker_diagnosis(self):
        for target, expected in (("source", "selected_source_changed"), ("tool", "selected_tool_changed")):
            with self.subTest(target=target):
                self.refusal(("invalid_package" + SUFFIX).encode())
                controller = self.controller()
                request = self.request()
                path = (self.staging / request["staged_id"] / "package.json" if target == "source" else
                        controller.workspace / "selected-tools/mcpb")
                path.chmod(0o700)
                path.write_bytes(b"changed")
                self.assertEqual(self.check_error(controller, request), {"error": expected})
                self.assertEqual(controller.runner.calls, [])

    def test_explicit_mcpb_timeout_survives_default_and_timeout_reaps_child(self):
        self.validator("import json,sys,time\ntime.sleep(.1)\n"
                       "print(json.dumps(json.load(open(sys.argv[sys.argv.index('--package')+1]))['fixture_plan']))")
        controller = self.controller()
        with patch.object(backend, "INSPECTION_TIMEOUT", .03):
            self.assertEqual(controller.selected_check(self.request())["profile"], "mcpb")
        self.assertEqual(controller.runner.calls[0][1]["inspection_timeout"], 120.0)

        self.validator("import os,time\nos.write(2," + repr(("invalid_package" + SUFFIX).encode()) +
                       ")\ntime.sleep(30)")
        controller = self.controller()
        children = []
        real_popen = subprocess.Popen

        def tracked(*args, **kwargs):
            child = real_popen(*args, **kwargs)
            children.append(child)
            return child

        started = time.monotonic()
        with patch.object(backend.subprocess, "Popen", tracked), patch.object(backend, "MCPB_INSPECTION_TIMEOUT", .15):
            self.assertEqual(self.check_error(controller, self.request()),
                             {"error": "selected_validation_unavailable"})
        self.assertLess(time.monotonic() - started, 3)
        self.assertEqual(len(children), 1)
        self.assertIsNotNone(children[0].returncode)
        self.assertTrue(children[0].stdout.closed)
        self.assertTrue(children[0].stderr.closed)
        self.assertFalse(hasattr(controller.runner._diagnostic_context, "stderr"))

    def test_existing_mcpb_runner_without_diagnostic_api_is_not_bypassed(self):
        self.assertFalse(hasattr(McpbRunner, "run_selected_validator"))
        controller = self.controller(runner_factory=McpbRunner)
        with patch.object(backend.subprocess, "Popen", side_effect=AssertionError("custom runner bypassed")):
            checked = controller.selected_check(self.request())
            self.assertEqual(checked["profile"], "mcpb")
            self.assertEqual(controller.runner.inspection_budgets[0][1], 120.0)
        controller = self.controller(runner_factory=McpbRunner)
        controller.runner.check_failure = True
        with patch.object(backend.subprocess, "Popen", side_effect=AssertionError("custom runner bypassed")):
            self.assertEqual(self.check_error(controller, self.request()),
                             {"error": "selected_validation_unavailable"})
        self.assertEqual(len(controller.runner.calls), 1)

    def test_custom_capture_malformed_or_exception_output_is_not_exposed(self):
        exact = ("invalid_package" + SUFFIX).encode()
        values = ((True, b"", exact), (1, "", exact), (1, b"", exact.decode()), (1, b""))
        for index, value in enumerate(values):
            with self.subTest(case=index):
                class Custom(McpbRunner):
                    def run_selected_validator(self, argv, **kwargs):
                        return value
                controller = self.controller(runner_factory=Custom)
                self.assertEqual(self.check_error(controller, self.request()),
                                 {"error": "selected_validation_unavailable"})
        class Raises(McpbRunner):
            def run_selected_validator(self, argv, **kwargs):
                raise RuntimeError(PRIVATE)
        controller = self.controller(runner_factory=Raises)
        self.assertEqual(self.check_error(controller, self.request()),
                         {"error": "selected_validation_unavailable"})


class McpbDiagnosticMcpTests(DiagnosticFixture, unittest.IsolatedAsyncioTestCase):
    async def test_failed_check_guidance_corrected_check_preserves_failed_snapshot(self):
        from mcp import Client

        self.validator("import json,os,sys\nfrom pathlib import Path\n"
                       "destination=Path(sys.argv[sys.argv.index('--destination')+1])\n"
                       "destination.mkdir()\n(destination/'prepared').mkdir()\n"
                       "package=json.load(open(sys.argv[sys.argv.index('--package')+1]))\n"
                       "if 'fixture_plan' not in package:\n os.write(2," +
                       repr(("invalid_package" + SUFFIX).encode()) + "); raise SystemExit(1)\n"
                       "print(json.dumps(package['fixture_plan']))")
        controller = self.controller()
        request = self.request()
        source = self.staging / request["staged_id"] / "package.json"
        valid = source.read_bytes()
        invalid = backend.encoded({"private": PRIVATE})
        source.write_bytes(invalid)
        request["package_sha256"] = backend.digest(invalid)
        owner = adapter.SetupAdapter(controller)
        try:
            async with Client(adapter.create_server(owner)) as client:
                failed = await client.call_tool("setup_selected_check", request)
                self.assertTrue(failed.is_error)
                self.assert_fixed(failed.structured_content, "invalid_package")
                self.assertIn("IntegrationPackage", failed.structured_content["diagnostic"]["message"])
                self.assertIn("setup_selected_check", failed.structured_content["diagnostic"]["message"])
                first = next((controller.workspace / "selections").iterdir())
                self.assertEqual((first / "package.json").read_bytes(), invalid)
                self.assertTrue((first / "inspection/prepared").is_dir())
                self.assertFalse((first / "effect.json").exists())
                self.assertEqual(controller.jobs, {})
                self.assertEqual(controller.validations, {})

                source.write_bytes(valid)
                request["package_sha256"] = backend.digest(valid)
                corrected = await client.call_tool("setup_selected_check", request)
                self.assertFalse(corrected.is_error, corrected)
                checked = corrected.structured_content
                self.assertEqual(checked["profile"], "mcpb")
                self.assertNotEqual(checked["validation_id"], first.name)
                self.assertEqual(checked["files"]["package.json"], backend.digest(valid))
                self.assertEqual((first / "package.json").read_bytes(), invalid)
                self.assertFalse(Path(checked["destination"]).exists())
                self.assertEqual(controller.jobs, {})
                self.assertEqual(len(controller.validations), 1)
                self.assertEqual([call[0][1] for call in controller.runner.calls], ["check", "check"])
                replay = await client.call_tool("setup_selected_check", request)
                self.assertFalse(replay.is_error, replay)
                self.assertEqual(replay.structured_content, checked)
                self.assertEqual(len(controller.runner.calls), 2)
        finally:
            await owner.aclose()


if __name__ == "__main__":
    unittest.main()
