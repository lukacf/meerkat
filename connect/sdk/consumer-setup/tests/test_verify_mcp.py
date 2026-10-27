"""Subprocess proof for the one-shot verifier, never a live provider."""
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
INPUT = {"type": "object", "properties": {"n": {"type": "integer", "minimum": 1}},
         "required": ["n"], "additionalProperties": False}
OUTPUT = {"type": "object", "properties": {"value": {"type": "integer"}},
          "required": ["value"], "additionalProperties": False}


class VerificationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="mcp verify ")
        self.addCleanup(self.temp.cleanup)
        self.work = Path(self.temp.name).resolve()
        self.log = self.work / "events.jsonl"
        self.config = self.work / "mcp.json"
        self.request_path = self.work / "request.json"
        self.env = {"PATH": "/usr/bin:/bin", "HOME": str(self.work)}

    def plan(self, mode="normal"):
        entry = {"command": sys.executable, "args": ["-I", "-B", str(ROOT / "tests/verification_server.py"), mode, str(self.log)],
                 "cwd": str(self.work), "env": {"MARKER": "preserved"}}
        self.config.write_text(json.dumps({"mcpServers": {"fixture": entry}}))
        return {"schema_version": 1,
                "launch": {"mcp_json": {"path": str(self.config), "sha256": hashlib.sha256(self.config.read_bytes()).hexdigest()},
                           "server": "fixture", "cwd": str(self.work), "env": dict(self.env, MARKER="preserved"), "mode": "auto"},
                "expected_tools": [{"name": "increment", "input_schema": INPUT, "output_schema": OUTPUT}],
                "calls": [{"name": "increment", "arguments": {"n": 1}}, {"name": "increment", "arguments": {"n": 2}}],
                "timeout_seconds": 10, "max_report_bytes": 4096}

    def start(self, plan, *, digest=None, env=None):
        self.request_path.write_text(json.dumps(plan))
        digest = digest or hashlib.sha256(self.request_path.read_bytes()).hexdigest()
        return subprocess.Popen([sys.executable, "-I", "-B", str(ROOT / "verify_mcp.py"),
                                 "--request", str(self.request_path), "--request-sha256", digest],
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env or self.env,
                                cwd=self.work, start_new_session=True)

    def finish(self, process):
        try:
            out, err = process.communicate(timeout=20)
        except BaseException:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise
        self.assertEqual(err, b"")
        result = json.loads(out)
        self.assertLessEqual(len(out), 4097)
        self.assertEqual(process.returncode, 0 if result["status"] == "observed" else 2)
        self.assertNotIn("PRIVATE_", out.decode())
        if self.log.exists():
            pid = self.events()[0]["pid"]
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)
        return result

    def check(self, plan, **kwargs):
        return self.finish(self.start(plan, **kwargs))

    def events(self):
        return [json.loads(row) for row in self.log.read_text().splitlines()]

    def calls(self):
        return [e for e in self.events() if e.get("method") == "tools/call"]

    def test_real_calls_preserve_launch_environment_and_order(self):
        result = self.check(self.plan())
        self.assertEqual(result["status"], "observed")
        self.assertEqual([r["result"]["structuredContent"]["value"] for r in result["calls"]], [2, 3])
        self.assertEqual(len(self.calls()), 2)
        self.assertEqual(self.events()[0]["marker"], "preserved")
        self.assertEqual(self.events()[0]["cwd"], str(self.work))
        discover = next(e for e in self.events() if e.get("method") == "server/discover")
        self.assertNotIn("sampling", json.dumps(discover))
        self.assertNotIn("elicitation", json.dumps(discover))
        self.assertTrue(result["transport_context_closed"])

    def test_legacy_session(self):
        plan = self.plan(); plan["launch"]["mode"] = "legacy"
        self.assertEqual(self.check(plan)["status"], "observed")
        self.assertFalse(any(e.get("method") == "server/discover" for e in self.events()))

    def test_missing_and_changed_selected_schema_prevent_all_calls(self):
        for mode in ("missing", "changed"):
            with self.subTest(mode=mode):
                self.log.unlink(missing_ok=True)
                result = self.check(self.plan(mode))
                self.assertEqual(result["reason"], "selected_tool_missing_or_changed")
                self.assertEqual(self.calls(), [])

    def test_unselected_extra_is_observed_not_called(self):
        result = self.check(self.plan("extra"))
        self.assertEqual(result["additional_tool_names"], ["unselected"])
        self.assertEqual([c["params"]["name"] for c in self.calls()], ["increment", "increment"])

    def test_changed_schema_after_first_call_stops_before_second(self):
        result = self.check(self.plan("later_changed"))
        self.assertEqual(result["reason"], "selected_tool_missing_or_changed")
        self.assertEqual(len(self.calls()), 1)
        self.assertEqual(result["calls"][0]["outcome"], "response_received")

    def test_all_pages_checked_duplicate_or_cycle_prevent_calls(self):
        self.assertEqual(self.check(self.plan("pages"))["status"], "observed")
        for mode in ("duplicate", "cycle"):
            with self.subTest(mode=mode):
                self.log.unlink()
                self.assertEqual(self.check(self.plan(mode))["status"], "stopped")
                self.assertEqual(self.calls(), [])

    def test_unknown_call_and_invalid_arguments_refused_before_launch(self):
        for call in ({"name": "other", "arguments": {}}, {"name": "increment", "arguments": {"n": True}}):
            plan = self.plan(); plan["calls"].append(call)
            self.assertEqual(self.check(plan)["phase"], "preflight")
            self.assertFalse(self.log.exists())

    def test_plan_and_config_hash_mismatch_prevent_launch(self):
        plan = self.plan()
        self.assertEqual(self.check(plan, digest="0" * 64)["reason"], "input_size_or_digest_mismatch")
        plan["launch"]["mcp_json"]["sha256"] = "0" * 64
        self.assertEqual(self.check(plan)["reason"], "input_size_or_digest_mismatch")
        self.assertFalse(self.log.exists())

    def test_nonregular_and_nonfinite_json_refuse_without_launch(self):
        plan = self.plan()
        fifo = self.work / "fifo"
        os.mkfifo(fifo)
        plan["launch"]["mcp_json"]["path"] = str(fifo)
        self.assertEqual(self.check(plan)["reason"], "regular_bounded_input_required")
        for value in (float("inf"), float("nan")):
            plan = self.plan(); plan["calls"][0]["arguments"]["n"] = value
            self.assertEqual(self.check(plan)["reason"], "nonfinite_json")
        self.assertFalse(self.log.exists())

    def test_explicit_environment_and_cwd_must_preserve_config(self):
        for change in ("environment", "cwd", "inherited"):
            with self.subTest(change=change):
                plan = self.plan(); env = self.env
                if change == "environment":
                    plan["launch"]["env"]["MARKER"] = "changed"
                elif change == "cwd":
                    plan["launch"]["cwd"] = "/tmp"
                else:
                    env = dict(self.env, TERM="must-cover")
                self.assertEqual(self.check(plan, env=env)["phase"], "preflight")
                self.assertFalse(self.log.exists())

    def test_unknown_launch_fields_are_not_ignored(self):
        plan = self.plan(); config = json.loads(self.config.read_text())
        config["mcpServers"]["fixture"]["transport"] = "not-stdio"
        self.config.write_text(json.dumps(config))
        plan["launch"]["mcp_json"]["sha256"] = hashlib.sha256(self.config.read_bytes()).hexdigest()
        self.assertEqual(self.check(plan)["reason"], "invalid_object_fields")
        self.assertFalse(self.log.exists())

    def test_input_required_task_and_error_stop_without_retry(self):
        for mode, reason in (("input", "input_required_no_retry"), ("task", "sdk_or_input_failure"), ("error", "tool_error_no_retry")):
            with self.subTest(mode=mode):
                self.log.unlink(missing_ok=True)
                result = self.check(self.plan(mode))
                self.assertEqual(result["reason"], reason)
                self.assertEqual(len(self.calls()), 1)

    def test_midcall_deadline_is_unknown_and_cleanup_joins(self):
        plan = self.plan("timeout"); plan["timeout_seconds"] = 1
        result = self.check(plan)
        self.assertEqual(result["reason"], "deadline_exceeded")
        self.assertEqual(result["calls"][0]["outcome"], "unknown")
        self.assertEqual(len(self.calls()), 1)

    def test_report_overflow_stops_after_first_call(self):
        result = self.check(self.plan("overflow"))
        self.assertEqual(result["reason"], "response_omitted_report_limit")
        self.assertNotIn("result", result["calls"][0])
        self.assertEqual(len(self.calls()), 1)

    def test_absent_and_null_output_schema_are_distinct(self):
        plan = self.plan("output_absent"); plan["expected_tools"][0].pop("output_schema")
        self.assertEqual(self.check(plan)["status"], "observed")
        self.log.unlink()
        plan = self.plan("output_null"); plan["expected_tools"][0].pop("output_schema")
        self.assertEqual(self.check(plan)["reason"], "selected_tool_missing_or_changed")
        self.assertEqual(self.calls(), [])

    def test_sdk_escalates_server_that_stalls_after_eof(self):
        result = self.check(self.plan("shutdown_stall"))
        self.assertEqual(result["status"], "observed")
        self.assertIn({"eof": True}, self.events())

    def test_termination_during_call_keeps_unknown_and_joins(self):
        process = self.start(self.plan("timeout"))
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if self.log.exists() and self.calls():
                break
            time.sleep(0.02)
        else:
            process.kill(); process.wait(); self.fail("fixture did not enter call")
        process.send_signal(signal.SIGTERM)
        time.sleep(0.1)
        process.send_signal(signal.SIGHUP)
        result = self.finish(process)
        self.assertEqual(result["reason"], "interrupted_outcome_unknown")
        self.assertEqual(result["calls"][0]["outcome"], "unknown")
        self.assertEqual(len(self.calls()), 1)


if __name__ == "__main__":
    unittest.main()
