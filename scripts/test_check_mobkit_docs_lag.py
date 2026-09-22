#!/usr/bin/env python3
"""Tests for the MobKit docs mirror lag ratchet (mirror vs MobKit main)."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("check-mobkit-docs-lag.py")
SPEC = importlib.util.spec_from_file_location("check_mobkit_docs_lag", SCRIPT)
assert SPEC and SPEC.loader
lag = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = lag
SPEC.loader.exec_module(lag)

MIRRORED = "1" * 40
HEAD = "9" * 40


def compare_payload(
    *,
    status: str,
    commits: list[str],
    files: list[str],
    renamed_from: dict[str, str] | None = None,
) -> dict[str, object]:
    """Shaped like `gh api repos/lukacf/meerkat-mobkit/compare/<mirrored>...main`."""
    renamed_from = renamed_from or {}
    return {
        "status": status,
        "ahead_by": len(commits),
        "behind_by": 0,
        "base_commit": {"sha": MIRRORED},
        "commits": [{"sha": sha} for sha in commits],
        "files": [
            {"filename": name, **({"previous_filename": renamed_from[name]} if name in renamed_from else {})}
            for name in files
        ],
    }


class DocsLagTests(unittest.TestCase):
    def test_identical_mirror_has_no_lag(self) -> None:
        result = lag.docs_lag(compare_payload(status="identical", commits=[], files=[]))
        self.assertEqual(result.ahead_by, 0)
        self.assertEqual(result.docs_commits, ())
        self.assertEqual(result.head_sha, MIRRORED)

    def test_code_only_commits_after_the_mirror_are_not_lag(self) -> None:
        result = lag.docs_lag(
            compare_payload(status="ahead", commits=["a" * 40, HEAD], files=["src/lib.rs", "Cargo.toml"])
        )
        self.assertEqual(result.ahead_by, 2)
        self.assertEqual(result.docs_commits, ())
        self.assertEqual(result.head_sha, HEAD)

    def test_docs_touching_commits_after_the_mirror_are_lag(self) -> None:
        result = lag.docs_lag(
            compare_payload(status="ahead", commits=["a" * 40, HEAD], files=["docs/guides/x.mdx", "src/lib.rs"])
        )
        self.assertEqual(result.docs_commits, ("a" * 12, HEAD[:12]))

    def test_a_docs_rename_counts_through_its_previous_path(self) -> None:
        result = lag.docs_lag(
            compare_payload(
                status="ahead",
                commits=[HEAD],
                files=["README.md"],
                renamed_from={"README.md": "docs/old.mdx"},
            )
        )
        self.assertEqual(result.docs_commits, (HEAD[:12],))

    def test_mirror_not_on_main_is_an_error_not_a_zero_lag(self) -> None:
        for status in ("behind", "diverged"):
            with self.assertRaisesRegex(lag.LagCheckError, "not an ancestor of main"):
                lag.docs_lag(compare_payload(status=status, commits=[], files=[]))

    def test_malformed_payloads_are_errors(self) -> None:
        with self.assertRaisesRegex(lag.LagCheckError, "not a JSON object"):
            lag.docs_lag([])
        with self.assertRaisesRegex(lag.LagCheckError, "unknown status"):
            lag.docs_lag({"status": "sideways"})
        payload = compare_payload(status="ahead", commits=[HEAD], files=["docs/x.mdx"])
        del payload["files"]
        with self.assertRaisesRegex(lag.LagCheckError, "no files array"):
            lag.docs_lag(payload)

    def run_check(self, payload: dict[str, object], max_lag: int | None = None) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as temp:
            manifest = Path(temp) / "_source.json"
            manifest.write_text(
                json.dumps({"generated": True, "source_commit": MIRRORED, "source_branch": "main"}),
                encoding="utf-8",
            )
            compare = Path(temp) / "compare.json"
            compare.write_text(json.dumps(payload), encoding="utf-8")
            command = [sys.executable, str(SCRIPT), "--manifest", str(manifest), "--compare", str(compare)]
            if max_lag is not None:
                command.extend(["--max-lag", str(max_lag)])
            return subprocess.run(command, capture_output=True, text=True)

    def test_command_passes_when_the_mirror_is_current_or_only_code_moved(self) -> None:
        current = self.run_check(compare_payload(status="identical", commits=[], files=[]))
        self.assertEqual(current.returncode, 0, current.stdout + current.stderr)
        self.assertIn("0 touching docs/", current.stdout)
        code_only = self.run_check(
            compare_payload(status="ahead", commits=[HEAD], files=["meerkat-mobkit/src/lib.rs"])
        )
        self.assertEqual(code_only.returncode, 0, code_only.stdout + code_only.stderr)
        self.assertIn("1 commit(s) ahead, 0 touching docs/", code_only.stdout)

    def test_command_fails_when_docs_moved_on_main_after_the_mirror(self) -> None:
        result = self.run_check(
            compare_payload(status="ahead", commits=["a" * 40, HEAD], files=["docs/concepts/roster.mdx"])
        )
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("misses 2 docs-touching commit(s)", result.stdout)
        self.assertIn("Publish MobKit docs workflow", result.stdout)
        tolerant = self.run_check(
            compare_payload(status="ahead", commits=[HEAD], files=["docs/concepts/roster.mdx"]),
            max_lag=1,
        )
        self.assertEqual(tolerant.returncode, 0, tolerant.stdout + tolerant.stderr)

    def test_command_distinguishes_unknown_lag_from_lag(self) -> None:
        result = self.run_check(compare_payload(status="diverged", commits=[], files=[]))
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("cannot determine lag", result.stdout)
        with tempfile.TemporaryDirectory() as temp:
            manifest = Path(temp) / "_source.json"
            manifest.write_text(json.dumps({"generated": True, "source_ref": "main"}), encoding="utf-8")
            compare = Path(temp) / "compare.json"
            compare.write_text(json.dumps(compare_payload(status="identical", commits=[], files=[])))
            missing = subprocess.run(
                [sys.executable, str(SCRIPT), "--manifest", str(manifest), "--compare", str(compare)],
                capture_output=True,
                text=True,
            )
        self.assertEqual(missing.returncode, 2, missing.stdout + missing.stderr)
        self.assertIn("has no source_commit", missing.stdout)


if __name__ == "__main__":
    unittest.main()
