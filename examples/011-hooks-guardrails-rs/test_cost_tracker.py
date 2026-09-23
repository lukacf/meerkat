import json
from pathlib import Path
import subprocess
import tempfile
import unittest


class CostTrackerTests(unittest.TestCase):
    def invoke(self, payload, log):
        return subprocess.run(
            ["python3", str(Path(__file__).with_name("cost_tracker.py")), str(log)],
            input=payload,
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )

    def test_invocation_is_logged_and_response_is_valid(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as root:
            log = Path(root) / "owned-log" / "costs.jsonl"
            invocation = {
                "point": "post_llm_response",
                "session_id": "00000000-0000-4000-8000-000000000001",
                "turn_number": 2,
                "llm_response": {
                    "assistant_text": "not-for-the-log",
                    "usage": {"input_tokens": 12, "output_tokens": 3},
                },
            }
            result = self.invoke(json.dumps(invocation), log)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout), {})
            row = json.loads(log.read_text())
            self.assertEqual(row["session_id"], invocation["session_id"])
            self.assertEqual(row["usage"], invocation["llm_response"]["usage"])
            self.assertEqual(row["turn_number"], 2)
            self.assertNotIn("not-for-the-log", log.read_text())

    def test_invalid_and_oversized_input_fail_without_success_response(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as root:
            log = Path(root) / "costs.jsonl"
            for payload in ["not-json", "x" * 65_537]:
                with self.subTest(payload_size=len(payload)):
                    result = self.invoke(payload, log)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(result.stdout, "")
                    self.assertFalse(log.exists())


if __name__ == "__main__":
    unittest.main()
