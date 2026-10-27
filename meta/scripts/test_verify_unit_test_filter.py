#!/usr/bin/env python3
"""Behavioral tests for the unit test filter check.

Each test writes a `nextest list --message-format json` listing and runs the
check on it as CI does: a filter that drops a lib or bin test, or skips a
whole binary, must fail the job; #[ignore]d tests and a filter that drops
nothing must pass; an empty listing must fail.
"""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

CHECK = Path(__file__).resolve().parents[1] / "scripts/verify-unit-test-filter.py"
MATCHES = {"status": "matches"}
BURST = "console_aggregator::tests::live_event_burst_reaches_store_while_slow_backfill_is_running"


def suite(cases: dict, status: str = "listed") -> dict:
    return {
        "package-name": "meerkat-meta",
        "kind": "lib",
        "status": status,
        "testcases": {name: {"ignored": match.get("reason") == "ignored", "filter-match": match}
                      for name, match in cases.items()},
    }


def run_check(suites: dict) -> subprocess.CompletedProcess:
    with tempfile.TemporaryDirectory() as tmp:
        listing = Path(tmp) / "unit-tests.json"
        listing.write_text(json.dumps({"test-count": 0, "rust-suites": suites}))
        return subprocess.run([sys.executable, str(CHECK), str(listing)], capture_output=True, text=True)


class UnitTestFilterTests(unittest.TestCase):
    def test_a_filter_that_drops_a_lib_test_fails_and_names_it(self):
        result = run_check({"meerkat-meta": suite({
            "rpc::tests::round_trip": MATCHES,
            BURST: {"status": "mismatch", "reason": "expression"},
        })})
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(result.stdout, f"left out of the unit test job: meerkat-meta {BURST} (expression)\n")

    def test_a_filter_that_drops_a_bin_test_or_names_tests_fails(self):
        result = run_check({
            "meerkat-meta::bin/rpc_gateway": suite({"tests::parses_flags": {"status": "mismatch",
                                                                            "reason": "expression"}}),
            "meerkat-meta": suite({"rpc::tests::round_trip": {"status": "mismatch", "reason": "string"}}),
        })
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("meerkat-meta::bin/rpc_gateway tests::parses_flags (expression)", result.stdout)
        self.assertIn("meerkat-meta rpc::tests::round_trip (string)", result.stdout)

    def test_a_filter_that_skips_a_whole_binary_fails(self):
        result = run_check({
            "meerkat-meta": suite({"rpc::tests::round_trip": MATCHES}),
            "meta-extension-state": suite({}, status="skipped"),
        })
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("meta-extension-state (the whole binary: skipped)", result.stdout)

    def test_ignored_tests_and_a_filter_that_drops_nothing_pass(self):
        result = run_check({
            "meerkat-meta": suite({
                "rpc::tests::round_trip": MATCHES,
                BURST: MATCHES,
                "rpc::tests::needs_a_live_key": {"status": "mismatch", "reason": "ignored"},
            }),
            "meerkat-meta::bin/rpc_gateway": suite({"tests::parses_flags": MATCHES}),
        })
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout,
                         "3 lib and bin tests run in the unit test job; 1 more are #[ignore]d in source\n")

    def test_an_empty_listing_fails(self):
        for suites in ({}, {"meerkat-meta": suite({})}):
            with self.subTest(suites=suites):
                result = run_check(suites)
                self.assertEqual(result.returncode, 1)
                self.assertIn("lists no tests", result.stderr)


if __name__ == "__main__":
    unittest.main()
