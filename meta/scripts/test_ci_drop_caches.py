#!/usr/bin/env python3
"""Contracts for scripts/ci-drop-superseded-caches.sh, run against a stub `gh`."""

import json
import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/ci-drop-superseded-caches.sh"
STARTED = "2026-10-14T12:00:00Z"

STUB = """#!/usr/bin/env bash
printf '%s\\n' "$*" >> "$GH_STUB_LOG"
case "$1 $2" in
  "api repos/owner/repo/actions/runs/42") [ -n "${GH_STUB_API_FAIL:-}" ] && exit 1; printf '%s\\n' "$GH_STUB_STARTED" ;;
  "cache list") [ -n "${GH_STUB_LIST_FAIL:-}" ] && exit 1; printf '%s' "$GH_STUB_LISTING" ;;
  "cache delete") [ "$3" = "${GH_STUB_DELETE_FAIL:-}" ] && exit 1; exit 0 ;;
  *) exit 2 ;;
esac
"""


def entry(id_, key, created, ref="refs/heads/main"):
    return {"id": id_, "key": key, "ref": ref, "createdAt": created}


class DropSupersededCachesTests(unittest.TestCase):
    def run_script(self, *families, listing=(), **env):
        with tempfile.TemporaryDirectory() as tmp:
            stub = Path(tmp) / "gh"
            stub.write_text(STUB)
            stub.chmod(stub.stat().st_mode | stat.S_IXUSR)
            log = Path(tmp) / "log"
            log.touch()
            result = subprocess.run(
                ["bash", str(SCRIPT), *families],
                env={
                    "PATH": f"{tmp}:{os.environ['PATH']}",
                    "GITHUB_REPOSITORY": "owner/repo",
                    "GITHUB_RUN_ID": "42",
                    "RUNNER_OS": "Linux",
                    "GH_STUB_LOG": str(log),
                    "GH_STUB_STARTED": STARTED,
                    "GH_STUB_LISTING": json.dumps(list(listing)),
                    **env,
                },
                capture_output=True,
                text=True,
            )
            return result, log.read_text().splitlines()

    def test_drops_earlier_main_generations_of_families_this_run_saved(self):
        result, calls = self.run_script(
            "meta-dev-examples",
            "fmt-lint",
            listing=[
                entry(11, "v0-rust-meta-dev-examples-Linux-x64-aaaa-1111", "2026-10-14T07:29:01.123456Z"),
                # Saved during this run: kept, and it supersedes 11 and 13.
                entry(12, "v0-rust-meta-dev-examples-Linux-x64-bbbb-2222", "2026-10-14T12:20:00.5Z"),
                entry(13, "v0-rust-meta-dev-examples-Linux-x64-aaaa-0000", "2026-10-13T23:59:59Z"),
                # Another ref's entry is never touched.
                entry(14, "v0-rust-meta-dev-examples-Linux-x64-aaaa-1111", "2026-10-14T07:00:00Z", "refs/pull/7/merge"),
                # This run saved no fmt-lint entry (an exact hit), so its entry stays.
                entry(15, "v0-rust-fmt-lint-Linux-x64-cccc-1111", "2026-10-14T08:00:00Z"),
            ],
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            calls,
            [
                "api repos/owner/repo/actions/runs/42 --jq .run_started_at",
                "cache list --repo owner/repo --ref refs/heads/main --key v0-rust-meta-dev-examples-Linux- "
                "--limit 100 --json id,key,ref,createdAt",
                "cache delete 11 --repo owner/repo",
                "cache delete 13 --repo owner/repo",
                "cache list --repo owner/repo --ref refs/heads/main --key v0-rust-fmt-lint-Linux- "
                "--limit 100 --json id,key,ref,createdAt",
            ],
        )
        self.assertIn("v0-rust-fmt-lint-Linux-*: nothing to drop", result.stdout)

    def test_a_family_prefix_never_matches_a_longer_job_name(self):
        # The server filters by key prefix; the script re-checks it, so "test"
        # never reaches "test-voice" even if a listing returns it.
        _, calls = self.run_script(
            "test",
            listing=[
                entry(21, "v0-rust-test-voice-Linux-x64-cccc-1111", "2026-10-14T07:00:00Z"),
                entry(22, "v0-rust-test-Linux-x64-dddd-1111", "2026-10-14T07:00:00Z"),
                entry(23, "v0-rust-test-Linux-x64-dddd-2222", "2026-10-14T12:10:00Z"),
                entry(24, "v0-rust-test-voice-Linux-x64-cccc-2222", "2026-10-14T12:10:00Z"),
            ],
        )
        self.assertIn("--key v0-rust-test-Linux- ", calls[1])
        self.assertEqual(calls[2:], ["cache delete 22 --repo owner/repo"])

    def test_failures_are_warnings_and_never_fail_the_job(self):
        result, calls = self.run_script("test", GH_STUB_API_FAIL="1")
        self.assertEqual(result.returncode, 0)
        self.assertIn("::warning::could not read this run's start time", result.stdout)
        self.assertEqual(len(calls), 1)

        # A family that cannot be listed is kept; the next one is still read.
        result, calls = self.run_script("test", "fmt-lint", GH_STUB_LIST_FAIL="1")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout.count("::warning::could not list"), 2)
        self.assertEqual(len(calls), 3)

        result, _ = self.run_script(
            "test",
            listing=[
                entry(7, "v0-rust-test-Linux-x64-a-1", "2026-10-14T01:00:00Z"),
                entry(8, "v0-rust-test-Linux-x64-a-2", "2026-10-14T02:00:00Z"),
                entry(9, "v0-rust-test-Linux-x64-a-3", "2026-10-14T12:30:00Z"),
            ],
            GH_STUB_DELETE_FAIL="7",
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("::warning::could not drop cache 7", result.stdout)
        self.assertIn("dropped cache 8", result.stdout)

    def test_nothing_to_drop_is_quiet_success(self):
        for listing in (
            # Only this run's entry.
            [entry(31, "v0-rust-fmt-lint-Linux-x64-e-1", "2026-10-14T12:30:00Z")],
            # Only older entries: this run saved nothing, so they stay.
            [entry(32, "v0-rust-fmt-lint-Linux-x64-e-1", "2026-10-14T09:30:00Z")],
        ):
            result, calls = self.run_script("fmt-lint", listing=listing)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(len(calls), 2)
            self.assertIn("v0-rust-fmt-lint-Linux-*: nothing to drop", result.stdout)

    def test_a_call_without_families_is_refused(self):
        result, calls = self.run_script()
        self.assertEqual(result.returncode, 2)
        self.assertEqual(calls, [])


if __name__ == "__main__":
    unittest.main()
