#!/usr/bin/env python3
"""Contract test: the conflict-marker gate fails on the defect it was written
for and stays quiet on a Markdown setext heading."""

from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

GATE = Path(__file__).with_name("check-conflict-markers")


def run_gate(*paths: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(GATE), *map(str, paths)],
        capture_output=True,
        text=True,
        check=False,
    )


class ConflictMarkerGate(unittest.TestCase):
    def test_fails_on_a_committed_conflict_block(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bad = Path(tmp) / "CHANGELOG.md"
            bad.write_text(
                "### Added\n<<<<<<< HEAD\n- ours\n=======\n- theirs\n"
                ">>>>>>> codex/some-branch\n",
                encoding="utf-8",
            )
            result = run_gate(bad)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("CHANGELOG.md:2: committed conflict marker", result.stdout)
        self.assertIn("CHANGELOG.md:6: committed conflict marker", result.stdout)

    def test_setext_heading_is_not_a_marker(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fine = Path(tmp) / "README.md"
            fine.write_text("Title\n=======\n\nbody\n", encoding="utf-8")
            result = run_gate(fine)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout, "")

    def test_open_without_close_is_not_a_marker(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "doc.md"
            doc.write_text("<<<<<<< HEAD appears in this prose example\n", encoding="utf-8")
            result = run_gate(doc)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
