"""Real-process watchdog controls without importing or starting the Forge app."""
import os
from pathlib import Path
import subprocess
import sys
import textwrap
import unittest
from unittest.mock import patch

from launchkit.runtime_diagnostics import runtime_diagnostics


class RuntimeDiagnosticTests(unittest.TestCase):
    def run_child(self, body):
        script = """
import asyncio, ctypes, time
from launchkit.runtime_diagnostics import RuntimeDiagnostics
async def exercise():
    retained_private_value = 'private-control-value-never-in-diagnostics'
    diagnostic = RuntimeDiagnostics(timeout=.12, pulse_interval=.02,
                                    report_interval=.08, max_dumps=2).start()
    try:
BODY
    finally:
        diagnostic.close()
    await asyncio.sleep(.16)
asyncio.run(exercise())
""".replace("BODY", textwrap.indent(textwrap.dedent(body).strip(), "        "))
        env = dict(os.environ, PYTHONPATH=str(Path(__file__).resolve().parents[1]))
        result = subprocess.run([sys.executable, "-c", script], env=env,
                                capture_output=True, text=True, timeout=8)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")
        self.assertNotIn("private-control-value-never-in-diagnostics", result.stderr)
        self.assertEqual(result.stderr.count("[forge-runtime-watchdog] stopped"), 1)
        return result.stderr

    def test_disabled_by_default_without_an_event_loop_or_timer(self):
        with patch.dict(os.environ, {}, clear=True), patch(
                "launchkit.runtime_diagnostics.faulthandler.dump_traceback_later") as arm:
            with runtime_diagnostics() as diagnostic:
                self.assertIsNone(diagnostic)
            arm.assert_not_called()

    def test_healthy_loop_reports_progress_without_stack_dumps(self):
        output = self.run_child("await asyncio.sleep(.34)")
        self.assertIn("loop_progress", output)
        self.assertNotIn("Timeout", output)
        self.assertNotIn("loop_resumed", output)

    def test_stalls_capture_threads_and_resume_with_a_bounded_dump_budget(self):
        output = self.run_child("""
            for _ in range(3):
                time.sleep(.26)
                await asyncio.sleep(.06)
        """)
        self.assertEqual(output.count("Timeout ("), 2)
        self.assertEqual(output.count("loop_resumed"), 3)
        self.assertIn('in exercise', output)
        self.assertNotIn("Timeout", output.split("[forge-runtime-watchdog] stopped", 1)[1])

    @unittest.skipUnless(sys.platform in {"darwin", "linux"}, "POSIX usleep control")
    def test_native_timer_captures_a_stall_while_python_holds_the_gil(self):
        output = self.run_child("""
            ctypes.PyDLL(None).usleep(260000)
            await asyncio.sleep(.06)
        """)
        self.assertEqual(output.count("Timeout ("), 1)
        self.assertIn("loop_resumed", output)


if __name__ == "__main__":
    unittest.main()
