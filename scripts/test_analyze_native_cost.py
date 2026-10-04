#!/usr/bin/env python3
"""Stdlib tests for native cost raw validation and matched calculations.

All timing and audit-count inputs here are synthetic parser fixtures. They do
not qualify a build, a quiet measurement window or native authorization work.
"""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("analyze-native-cost.py")
SPEC = importlib.util.spec_from_file_location("analyze_native_cost", SCRIPT)
assert SPEC and SPEC.loader
analyzer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(analyzer)
PAIRS = 2000


def duration(pair, mode):
    # Two ratios differ from the ratio of the two matched sums: +100% and -10%
    # have a ratio of means of 0%, and signed differences +100 and -100.
    return ((100, 1000) if mode == "trusted_host" else (200, 900))[pair % 2]


def small_raw(warmup=100):
    samples = []
    for depth in (1, 3):
        for instrumentation in ("turn_only", "boundaries"):
            for pair in range(PAIRS):
                for mode in ("trusted_host", "local_governed"):
                    samples.append({
                        "depth": depth, "instrumentation": instrumentation,
                        "mode": mode, "pair": pair,
                        "first_in_pair": ((pair + warmup) % 2 == 0) == (mode == "trusted_host"),
                        "model_requests": 2, "read_effects": 4,
                        "audit_records": 18 if mode == "local_governed" else 0,
                        "turn_ns": duration(pair, mode),
                        "model_authorization_ns": (
                            [1, 2] if mode == "trusted_host" else [2, 1]
                        ) if instrumentation == "boundaries" else [],
                        "tool_batch_ns": (10 if mode == "trusted_host" else 11)
                        if instrumentation == "boundaries" else None,
                    })
    return {"schema": 1, "warmup_pairs": warmup, "pairs_per_cell": PAIRS,
            "failures": 0, "timeouts": 0, "unsupported": [], "samples": samples}


def representative_raw():
    samples = []
    for depth in (1, 3):
        for workload in ("fresh_admission", "continuing_segment", "individual_fenced_tools"):
            prefix = workload != "fresh_admission"
            direct = workload == "individual_fenced_tools"
            calls = [("direct-" if direct else "read-") + str(i) for i in range(4)]
            for pair in range(PAIRS):
                for mode in ("trusted_host", "local_governed"):
                    governed = mode == "local_governed"
                    operation_count = (4 if direct else 6) if governed else 0
                    value = duration(pair, mode)
                    samples.append({
                        "depth": depth, "workload": workload, "mode": mode,
                        "pair": pair, "first_in_pair": (pair % 2 == 0) == (mode == "trusted_host"),
                        "old_rows": 252, "active_rows": 4, "total_rows": 256,
                        "contributor_ids": ["input-" + str(i) for i in range(4)],
                        "run_id": "synthetic-run", "prefix_audit_digest": [0] * 32,
                        "prefix_audit_records": 1002 if prefix and governed else 0,
                        "final_audit_records": (1017 if direct else 1020 if prefix else 18) if governed else 0,
                        "prefix_reads": 333 if prefix else 0,
                        "final_reads": 337 if prefix else 4,
                        "model_requests": 3 if workload == "continuing_segment" else 2,
                        "measured_operation_ids": ["operation-" + str(i) for i in range(operation_count)],
                        "measured_call_ids": calls, "measured_ns": None if direct else value,
                        "individual_ns": [[call, value] for call in calls] if direct else [],
                    })
    return {"schema": 2, "suite": "representative", "warmup_pairs": 100,
            "pairs_per_cell": PAIRS, "failures": 0, "timeouts": 0, "samples": samples}


def replace_sample(raw, index=0, **changes):
    result = dict(raw)
    result["samples"] = list(raw["samples"])
    result["samples"][index] = raw["samples"][index] | changes
    return result


class NativeCostAnalysisTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.small = small_raw()
        cls.representative = representative_raw()

    def test_signed_nearest_rank_and_ratio_of_means_are_distinct(self):
        result = analyzer.analyze(self.small)
        self.assertEqual(len(result["summaries"]), 4)
        cell = result["summaries"][0]
        self.assertEqual(cell["modes"]["trusted_host"]["turn_ns"],
                         {"p50": 100, "p95": 1000, "p99": 1000, "n": PAIRS})
        self.assertEqual(cell["paired_turn_delta_ns"],
                         {"p50": -100, "p95": 100, "p99": 100, "n": PAIRS})
        self.assertEqual(cell["paired_turn_overhead_percent"]["p99"], 100)
        self.assertEqual(cell["turn_ratio_of_means_overhead_percent"], 0)
        strata = cell["first_mode_strata"]["turn_ns"]
        self.assertEqual(strata["trusted_first"]["fixture_pairs"], PAIRS // 2)
        self.assertEqual(strata["trusted_first"]["paired_delta_ns"]["p99"], 100)
        self.assertEqual(strata["governed_first"]["paired_delta_ns"]["p99"], -100)
        boundaries = result["summaries"][1]
        self.assertEqual([s["p99"] for s in boundaries["paired_model_authorization_delta_ns_by_request_index"]], [1, -1])
        self.assertTrue(result["acceptance"].startswith("UNPROVEN:"))

    def test_odd_warmup_changes_first_mode_without_changing_matched_cost(self):
        cell = analyzer.analyze(small_raw(warmup=21))["summaries"][0]
        strata = cell["first_mode_strata"]["turn_ns"]
        self.assertEqual(strata["trusted_first"]["paired_delta_ns"]["p99"], -100)
        self.assertEqual(strata["governed_first"]["paired_delta_ns"]["p99"], 100)
        self.assertEqual(cell["turn_ratio_of_means_overhead_percent"], 0)

    def test_representative_intervals_and_correlated_calls_keep_their_units(self):
        result = analyzer.analyze_representative(self.representative)
        self.assertEqual(len(result["summaries"]), 6)
        fresh, continuing, direct = result["summaries"][:3]
        self.assertEqual(fresh["ratio_of_means_overhead_percent"], 0)
        self.assertEqual(continuing["ratio_of_means_overhead_percent"], 0)
        self.assertIn("whole turn", fresh["interpretation"])
        self.assertIn("not admission", continuing["interpretation"])
        self.assertEqual(direct["fixture_pairs"], PAIRS)
        self.assertEqual(direct["modes_ns"]["trusted_host"]["n"], 4 * PAIRS)
        self.assertIn("four correlated calls", direct["interpretation"])
        self.assertEqual([s["call_id"] for s in direct["by_call_id"]], ["direct-0", "direct-1", "direct-2", "direct-3"])
        for call in direct["by_call_id"]:
            self.assertEqual(call["fixture_pairs"], PAIRS)
            self.assertEqual(call["modes_ns"]["trusted_host"]["n"], PAIRS)
            self.assertEqual(call["paired_delta_ns"]["p50"], -100)
            self.assertEqual(call["ratio_of_means_overhead_percent"], 0)
        self.assertTrue(result["acceptance"].startswith("UNPROVEN:"))

    def test_both_schemas_reject_boolean_and_float_depth_aliases(self):
        for raw, analyze in ((self.small, analyzer.analyze),
                             (self.representative, analyzer.analyze_representative)):
            for value in (True, 1.0):
                with self.subTest(schema=raw["schema"], depth=value, type=type(value).__name__):
                    with self.assertRaises(ValueError):
                        analyze(replace_sample(raw, depth=value))

    def test_turn_only_rejects_any_nonnull_tool_batch(self):
        for value in (17, True, {}, []):
            with self.subTest(batch=value):
                with self.assertRaises(ValueError):
                    analyzer.analyze(replace_sample(self.small, tool_batch_ns=value))

    def test_both_schemas_require_complete_unique_integer_pairs(self):
        for raw, analyze in ((self.small, analyzer.analyze),
                             (self.representative, analyzer.analyze_representative)):
            for value in (False, 0.0, 1):
                with self.subTest(schema=raw["schema"], pair=value, type=type(value).__name__):
                    with self.assertRaises(ValueError):
                        analyze(replace_sample(raw, pair=value))
            missing = dict(raw, samples=raw["samples"][1:])
            with self.subTest(schema=raw["schema"], missing=True):
                with self.assertRaises(ValueError):
                    analyze(missing)

    def test_both_schemas_require_actual_parity_and_boolean_order(self):
        for raw, analyze in ((self.small, analyzer.analyze),
                             (self.representative, analyzer.analyze_representative)):
            for value in (False, 1):
                with self.subTest(schema=raw["schema"], first=value, type=type(value).__name__):
                    with self.assertRaises(ValueError):
                        analyze(replace_sample(raw, first_in_pair=value))
            with self.subTest(schema=raw["schema"], warmup=True):
                with self.assertRaises(ValueError):
                    analyze(dict(raw, warmup_pairs=True))

    def test_durations_must_be_positive_integer_nanoseconds(self):
        for value in (0, -1, True, 1.5):
            with self.subTest(value=value, type=type(value).__name__):
                with self.assertRaises(ValueError):
                    analyzer.analyze(replace_sample(self.small, turn_ns=value))
                with self.assertRaises(ValueError):
                    analyzer.analyze_representative(replace_sample(self.representative, measured_ns=value))

    def test_work_and_boundary_cardinality_cannot_be_omitted(self):
        for changes in ({"read_effects": 3}, {"audit_records": 1}, {"model_authorization_ns": [1]}):
            with self.subTest(changes=changes):
                with self.assertRaises(ValueError):
                    analyzer.analyze(replace_sample(self.small, **changes))
        boundary = 2 * PAIRS
        with self.assertRaises(ValueError):
            analyzer.analyze(replace_sample(self.small, boundary, model_authorization_ns=[1]))
        direct = 4 * PAIRS
        for changes in ({"individual_ns": [["direct-0", 1]]}, {"measured_ns": 1}, {"final_audit_records": 0}):
            with self.subTest(changes=changes):
                with self.assertRaises(ValueError):
                    analyzer.analyze_representative(replace_sample(self.representative, direct + 1, **changes))

    def test_failed_or_timed_out_work_is_never_summarized(self):
        for raw, analyze in ((self.small, analyzer.analyze),
                             (self.representative, analyzer.analyze_representative)):
            for field in ("failures", "timeouts"):
                with self.subTest(schema=raw["schema"], field=field):
                    with self.assertRaises(ValueError):
                        analyze(dict(raw, **{field: 1}))

    def run_cli(self, content):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "raw.json"
            path.write_text(content, encoding="utf-8")
            return subprocess.run([sys.executable, str(SCRIPT), str(path)],
                                  capture_output=True, text=True, timeout=15, check=False)

    def test_cli_rejects_invalid_schema_and_nonfinite_json_without_summary(self):
        for content in ('[]', '{"schema": true}', '{"schema": 3}', '{"schema": 1, "failures": NaN}'):
            with self.subTest(content=content):
                result = self.run_cli(content)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")
                self.assertIn("invalid raw input:", result.stderr)

    def test_cli_emits_json_but_does_not_claim_acceptance(self):
        result = self.run_cli(json.dumps(self.small))
        self.assertEqual(result.returncode, 0, result.stderr)
        summary = json.loads(result.stdout)
        self.assertEqual(summary["summaries"][0]["turn_ratio_of_means_overhead_percent"], 0)
        self.assertTrue(summary["acceptance"].startswith("UNPROVEN:"))


if __name__ == "__main__":
    unittest.main()
