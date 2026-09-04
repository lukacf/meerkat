#!/usr/bin/env python3
"""Tests for the nightly MobKit docs mirror lag ratchet."""

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
# Dataclasses resolve postponed annotations through sys.modules; register
# the module before executing it like a normal import would.
sys.modules[SPEC.name] = lag
SPEC.loader.exec_module(lag)

# Shaped like the real `gh api repos/lukacf/meerkat-mobkit/releases` payload on
# 2026-09-03: a draft with a null published_at listed first, and publication
# order not equal to list order.
RELEASES = [
    {"tag_name": "v0.4.11", "draft": True, "prerelease": False, "published_at": None},
    {"tag_name": "v0.8.28", "draft": False, "prerelease": False, "published_at": "2026-08-29T11:48:17Z"},
    {"tag_name": "v0.9.0-rc.1", "draft": False, "prerelease": True, "published_at": "2026-09-04T00:00:00Z"},
    {"tag_name": "v0.8.30", "draft": False, "prerelease": False, "published_at": "2026-09-03T06:15:36Z"},
    {"tag_name": "v0.8.27", "draft": False, "prerelease": False, "published_at": "2026-08-29T07:35:18Z"},
]


class CheckMobKitDocsLagTests(unittest.TestCase):
    def test_only_published_full_releases_count_newest_first(self) -> None:
        releases = lag.published_releases(RELEASES)
        self.assertEqual(
            [release.tag_name for release in releases],
            ["v0.8.30", "v0.8.28", "v0.8.27"],
        )

    def test_lag_is_the_number_of_releases_published_after_the_mirror(self) -> None:
        releases = lag.published_releases(RELEASES)
        self.assertEqual(lag.mirror_lag("v0.8.30", releases), 0)
        self.assertEqual(lag.mirror_lag("v0.8.28", releases), 1)
        self.assertEqual(lag.mirror_lag("v0.8.27", releases), 2)

    def test_unpublished_mirror_ref_is_an_error_not_a_zero_lag(self) -> None:
        releases = lag.published_releases(RELEASES)
        with self.assertRaisesRegex(lag.LagCheckError, "v0.8.29 is not among the published releases"):
            lag.mirror_lag("v0.8.29", releases)

    def test_payload_without_published_releases_is_an_error(self) -> None:
        with self.assertRaisesRegex(lag.LagCheckError, "no published releases"):
            lag.published_releases([RELEASES[0]])
        with self.assertRaisesRegex(lag.LagCheckError, "not a JSON array"):
            lag.published_releases({"message": "Not Found"})

    def run_check(self, source_ref: str, max_lag: int | None = None) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as temp:
            manifest = Path(temp) / "_source.json"
            manifest.write_text(json.dumps({"generated": True, "source_ref": source_ref}), encoding="utf-8")
            releases = Path(temp) / "releases.json"
            releases.write_text(json.dumps(RELEASES), encoding="utf-8")
            command = [
                sys.executable,
                str(SCRIPT),
                "--manifest",
                str(manifest),
                "--releases",
                str(releases),
            ]
            if max_lag is not None:
                command.extend(["--max-lag", str(max_lag)])
            return subprocess.run(command, capture_output=True, text=True)

    def test_command_passes_within_the_default_tolerance_of_one_release(self) -> None:
        current = self.run_check("v0.8.30")
        self.assertEqual(current.returncode, 0, current.stdout + current.stderr)
        self.assertIn("lag 0 release(s)", current.stdout)
        one_behind = self.run_check("v0.8.28")
        self.assertEqual(one_behind.returncode, 0, one_behind.stdout + one_behind.stderr)
        self.assertIn("lag 1 release(s)", one_behind.stdout)

    def test_command_fails_when_more_than_one_release_was_published_after_the_mirror(self) -> None:
        result = self.run_check("v0.8.27")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("docs/mobkit is 2 releases behind", result.stdout)
        self.assertIn("published after v0.8.27: v0.8.30, v0.8.28", result.stdout)
        strict = self.run_check("v0.8.28", max_lag=0)
        self.assertEqual(strict.returncode, 1, strict.stdout + strict.stderr)

    def test_command_distinguishes_unknown_lag_from_lag(self) -> None:
        result = self.run_check("v0.8.29")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("cannot determine lag", result.stdout)


if __name__ == "__main__":
    unittest.main()
