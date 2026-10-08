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


def native_model_raw(profile=None):
    raw = small_raw()
    for sample in raw["samples"]:
        sample["native_model_preparation_ns"] = (
            [10, 90] if sample["mode"] == "trusted_host" else [15, 80]
        ) if sample["instrumentation"] == "boundaries" else []
    if profile is not None:
        raw["measurement_profile"] = profile
        raw["measurement_status"] = "complete"
        raw["samples"] = [sample for sample in raw["samples"]
                          if sample["instrumentation"] == "boundaries"]
    return raw


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
        cls.native_model = native_model_raw("model_dispatch_tail")
        cls.representative = representative_raw()

    def test_native_model_tail_pairs_same_request_without_extra_mean_cells(self):
        result = analyzer.analyze(self.native_model)
        self.assertEqual(result["measurement_profile"], "model_dispatch_tail")
        self.assertEqual(len(result["summaries"]), 2)
        for cell in result["summaries"]:
            self.assertEqual(cell["instrumentation"], "boundaries")
            self.assertEqual(cell["modes"]["trusted_host"]["native_model_preparation_ns"],
                             {"p50": 10, "p95": 90, "p99": 90, "n": 2 * PAIRS})
            self.assertEqual([s["p99"] for s in cell["paired_native_model_preparation_delta_ns_by_request_index"]],
                             [5, -10])
            self.assertEqual(cell["paired_native_model_preparation_delta_ns"]["p99"], 5)
            strata = cell["first_mode_strata"]["native_model_preparation_ns_by_request_index"]
            self.assertEqual(strata[0]["strata"]["trusted_first"]["fixture_pairs"], PAIRS // 2)
            self.assertEqual(strata[1]["strata"]["governed_first"]["paired_delta_ns"]["p99"], -10)
            self.assertIn("CallingLlm", cell["native_model_preparation_interpretation"])
            self.assertIn("two correlated", cell["native_model_preparation_interpretation"])
        self.assertTrue(result["acceptance"].startswith("UNPROVEN:"))

    def test_native_span_can_be_analyzed_in_existing_four_cell_matrix(self):
        result = analyzer.analyze(native_model_raw())
        self.assertEqual(len(result["summaries"]), 4)
        self.assertNotIn("native_model_preparation_ns", result["summaries"][0]["modes"]["trusted_host"])
        self.assertIn("native_model_preparation_ns", result["summaries"][1]["modes"]["trusted_host"])

    def test_legacy_raw_does_not_acquire_a_native_span_from_provider_diagnostic(self):
        result = analyzer.analyze(self.small)
        self.assertIn("native model preparation absent", result["acceptance"])
        for cell in result["summaries"]:
            self.assertNotIn("native_model_preparation_interpretation", cell)
            for mode in cell["modes"].values():
                self.assertNotIn("native_model_preparation_ns", mode)

    def test_native_model_profile_requires_fixed_plan_and_only_boundary_cells(self):
        for changes in ({"measurement_profile": "unknown"}, {"warmup_pairs": 99},
                        {"pairs_per_cell": 2001}, {"pairs_per_cell": 2000.0}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                analyzer.analyze(dict(self.native_model, **changes))
        for instrumentation in ("turn_only", "unknown"):
            with self.subTest(instrumentation=instrumentation), self.assertRaises(ValueError):
                analyzer.analyze(replace_sample(self.native_model, instrumentation=instrumentation))

    def test_native_model_cardinality_and_positive_integer_durations_are_required(self):
        for values in (None, [], [1], [1, 2, 3], [0, 2], [-1, 2], [True, 2], [1.5, 2]):
            with self.subTest(values=values), self.assertRaises(ValueError):
                analyzer.analyze(replace_sample(self.native_model, native_model_preparation_ns=values))
        for remove_all in (False, True):
            raw = dict(self.native_model, samples=[dict(sample) for sample in self.native_model["samples"]])
            for sample in raw["samples"] if remove_all else raw["samples"][:1]:
                del sample["native_model_preparation_ns"]
            with self.subTest(remove_all=remove_all), self.assertRaises(ValueError):
                analyzer.analyze(raw)
        # An ordinary matrix also cannot mix old and new instrumentation.
        raw = native_model_raw()
        del raw["samples"][0]["native_model_preparation_ns"]
        with self.assertRaises(ValueError):
            analyzer.analyze(raw)
        with self.assertRaises(ValueError):
            analyzer.analyze(replace_sample(native_model_raw(), native_model_preparation_ns=[1, 2]))

    def test_native_model_counters_require_exact_integers(self):
        for field in ("failures", "timeouts"):
            for value in (False, None, "", [], 0.0, -1):
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    analyzer.analyze(dict(self.native_model, **{field: value}))
        for field, value in (("model_requests", 2.0), ("read_effects", 4.0),
                             ("audit_records", False), ("audit_records", 0.0)):
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                analyzer.analyze(replace_sample(self.native_model, **{field: value}))

    def test_truncated_model_tail_reports_counts_without_any_timing_summary(self):
        raw = dict(self.native_model, measurement_status="budget_exhausted", timeouts=1,
                   samples=self.native_model["samples"][:2801])
        result = analyzer.analyze(raw)
        self.assertEqual(result["analysis_status"], "INCOMPLETE")
        self.assertEqual(result["summaries"], [])
        self.assertEqual(result["cells"][0]["sample_counts"],
                         {"trusted_host": 1401, "local_governed": 1400})
        self.assertEqual(result["cells"][1]["sample_counts"],
                         {"trusted_host": 0, "local_governed": 0})
        self.assertTrue(all(not cell["complete"] for cell in result["cells"]))
        self.assertTrue(result["acceptance"].startswith("UNPROVEN:"))
        # Failure to finish the later depth cannot turn the earlier depth into
        # a selective performance report from an incomplete allocation.
        raw["samples"] = self.native_model["samples"][:2 * PAIRS]
        result = analyzer.analyze(raw)
        self.assertTrue(result["cells"][0]["complete"])
        self.assertEqual(result["summaries"], [])

    def test_truncated_model_tail_still_rejects_corrupt_or_duplicate_samples(self):
        raw = dict(self.native_model, measurement_status="budget_exhausted", timeouts=1,
                   samples=self.native_model["samples"][:2])
        for candidate in (replace_sample(raw, native_model_preparation_ns=[]),
                          dict(raw, samples=raw["samples"] + raw["samples"][:1]),
                          replace_sample(raw, pair=PAIRS)):
            with self.assertRaises(ValueError):
                analyzer.analyze(candidate)
        for changes in ({"measurement_status": "unknown"}, {"timeouts": 0},
                        {"measurement_status": "complete"}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                analyzer.analyze(dict(raw, **changes))

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



def fixed_mean_raw(ratio=1.01):
    raw = representative_raw()
    raw.update(measurement_profile="fixed_mean_32", measurement_status="complete",
               warmup_pairs=20, pairs_per_cell=32)
    raw["samples"] = [dict(s) for s in raw["samples"] if s["pair"] < 32]
    for sample in raw["samples"]:
        pair = sample["pair"]
        value = 1000 + 17 * pair + 5 * (pair % 3)
        if sample["mode"] == "local_governed":
            value = round(ratio * value + (-3 if pair % 2 else 3))
        if sample["workload"] == "individual_fenced_tools":
            sample["individual_ns"] = [[call, value] for call in sample["measured_call_ids"]]
        else:
            sample["measured_ns"] = value
    return raw


class FixedMeanAnalysisTests(unittest.TestCase):
    def test_fixed_profile_uses_six_paired_mean_intervals_without_tail_labels(self):
        result = analyzer.analyze_representative(fixed_mean_raw())
        self.assertEqual(len(result["summaries"]), 6)
        self.assertTrue(result["acceptance"].startswith("UNPROVEN:"))
        self.assertNotIn('"p99"', json.dumps(result))
        for cell in result["summaries"]:
            self.assertEqual(cell["fixture_pairs"], 32)
            self.assertEqual(cell["independent_candidate_blocks"], 16)
            self.assertEqual(cell["conditional_classification"], "CLEAR")
            self.assertLessEqual(cell["ratio_ci"][1], 1.10)
            self.assertLessEqual(cell["threshold_difference_ci_ns"][1], 0)
            self.assertEqual(cell["order_strata"]["trusted_first"]["fixture_pairs"], 16)
            self.assertEqual(cell["order_strata"]["governed_first"]["fixture_pairs"], 16)
            self.assertEqual(cell["modes_ns"]["trusted_host"]["n"], 32)
        self.assertEqual(result["confidence_model"]["family_comparisons"], 6)
        self.assertEqual(result["confidence_model"]["degrees_of_freedom"], 15)
        self.assertIn("conditional", result["confidence_model"]["qualification"])
        self.assertIn("four correlated", result["summaries"][2]["interpretation"])

    def test_mean_interval_miss_and_threshold_uncertainty_remain_distinct(self):
        for ratio, expected in ((1.30, "MISS"), (1.10, "UNCERTAIN")):
            with self.subTest(ratio=ratio):
                result = analyzer.analyze_representative(fixed_mean_raw(ratio))
                self.assertEqual({s["conditional_classification"] for s in result["summaries"]}, {expected})
                if expected == "MISS":
                    self.assertTrue(all(s["ratio_ci"][0] > 1.10 for s in result["summaries"]))
                    self.assertTrue(all(s["threshold_difference_ci_ns"][0] > 0 for s in result["summaries"]))

    def test_degenerate_or_unbounded_denominator_cannot_be_a_clear_result(self):
        for unstable in (False, True):
            raw = fixed_mean_raw()
            for sample in raw["samples"]:
                value = 100000000 if unstable and sample["pair"] == 31 else 1000
                if sample["workload"] == "individual_fenced_tools":
                    sample["individual_ns"] = [[call, value] for call in sample["measured_call_ids"]]
                else:
                    sample["measured_ns"] = value
            with self.subTest(unstable=unstable):
                result = analyzer.analyze_representative(raw)
                self.assertEqual({s["conditional_classification"] for s in result["summaries"]}, {"UNCERTAIN"})
                self.assertTrue(all(s["ratio_ci"] is None for s in result["summaries"]))

    def test_critical_value_matches_the_predeclared_six_cell_family(self):
        result = analyzer.analyze_representative(fixed_mean_raw())
        critical = result["confidence_model"]["critical_t"]
        # Independent Student t(15) CDF via its density after t=sqrt(15)*tan(a).
        # Integrate cos(a)**14 by the finite even-power recurrence.
        def cdf(value):
            import math
            angle = math.atan(value / math.sqrt(15))
            integral = angle
            for power in range(2, 15, 2):
                integral = math.sin(angle)*math.cos(angle)**(power-1)/power + (power-1)/power*integral
            return .5 + math.exp(math.lgamma(8)-math.lgamma(7.5))/math.sqrt(math.pi)*integral
        target = 1-.05/(2*6)
        self.assertGreaterEqual(cdf(critical), target)
        self.assertLess(cdf(critical-.000001), target)

    def test_numerical_ratio_and_difference_disagreement_is_uncertain(self):
        raw = fixed_mean_raw()
        for sample in raw["samples"]:
            pair = sample["pair"]
            value = 10**12 + 17*pair + 5*(pair % 3)
            if sample["mode"] == "local_governed":
                value = round(1.10*value) + (-3 if pair % 2 else 3)
            if sample["workload"] == "individual_fenced_tools":
                sample["individual_ns"] = [[call, value] for call in sample["measured_call_ids"]]
            else:
                sample["measured_ns"] = value
        cell = analyzer.analyze_representative(raw)["summaries"][0]
        self.assertLessEqual(cell["ratio_ci"][1], 1.10)
        self.assertGreater(cell["threshold_difference_ci_ns"][1], 0)
        self.assertEqual(cell["conditional_classification"], "UNCERTAIN")
        self.assertIn("inconsistent", cell["classification_reason"])

    def test_fixed_profile_does_not_relax_the_existing_tail_path(self):
        raw = fixed_mean_raw()
        raw.pop("measurement_profile")
        with self.assertRaises(ValueError):
            analyzer.analyze_representative(raw)
        for changes in ({"measurement_profile": "unknown"}, {"pairs_per_cell": 32.0},
                        {"pairs_per_cell": 16}, {"warmup_pairs": 21},
                        {"measurement_status": "unknown"}):
            with self.subTest(changes=changes):
                with self.assertRaises(ValueError):
                    analyzer.analyze_representative(fixed_mean_raw() | changes)

    def test_timeout_or_incomplete_run_emits_uncertainty_without_numeric_intervals(self):
        raw = fixed_mean_raw()
        for changes in ({"measurement_status": "budget_exhausted", "timeouts": 1, "samples": []},
                        {"samples": raw["samples"][1:]}, {"failures": 1}):
            with self.subTest(changed_fields=tuple(changes)):
                result = analyzer.analyze_representative(raw | changes)
                self.assertEqual(result["analysis_status"], "UNCERTAIN")
                self.assertEqual(result["summaries"], [])
                self.assertTrue(result["acceptance"].startswith("UNPROVEN:"))

    def test_conflicting_order_strata_prevent_a_clear_or_miss_claim(self):
        raw = fixed_mean_raw()
        for sample in raw["samples"]:
            pair = sample["pair"]
            value = 1000 + 17 * pair + 5 * (pair % 3)
            if sample["mode"] == "local_governed":
                value = round(value * (1.15 if pair % 2 == 0 else .70))
            if sample["workload"] == "individual_fenced_tools":
                sample["individual_ns"] = [[call, value] for call in sample["measured_call_ids"]]
            else:
                sample["measured_ns"] = value
        result = analyzer.analyze_representative(raw)
        self.assertEqual({s["conditional_classification"] for s in result["summaries"]}, {"UNCERTAIN"})
        self.assertTrue(all(s["order_strata"]["trusted_first"]["ratio_of_means"] > 1.10 for s in result["summaries"]))



def tool_tail_raw():
    samples, cells = [], []
    warmup = 100
    for depth in (1, 3):
        for mode in ("trusted_host", "local_governed"):
            governed = mode == "local_governed"
            cells.append({
                "depth": depth, "mode": mode,
                "old_rows": 252, "active_rows": 4, "total_rows": 256,
                "contributor_ids": [f"{depth}-{mode}-input-{i}" for i in range(4)],
                "run_id": f"{depth}-{mode}-run",
                "prefix_audit_records": 1002 if governed else 0,
                "prefix_audit_digest": [0] * 32,
                "prefix_reads": 333,
                "final_reads": 333 + warmup + PAIRS,
                "final_audit_records": (335 + warmup + PAIRS) * 3 if governed else 0,
                "model_requests": 2,
            })
            for pair in range(PAIRS):
                ordinal = warmup + pair
                # Matched deltas alternate between -100 and +100 ns. A
                # difference between marginal p99s would incorrectly be zero.
                baseline = 200 if pair % 2 == 0 else 100
                samples.append({
                    "depth": depth, "mode": mode, "pair": pair,
                    "first_in_pair": (ordinal % 2 == 0) == (mode == "trusted_host"),
                    "call_ordinal": ordinal, "call_id": f"tool-tail-{ordinal}",
                    "audit_records_before": 1002 + 3 * ordinal if governed else 0,
                    "dispatch_ns": (300 - baseline) if governed else baseline,
                })
    return {"schema": 3, "suite": "tool_dispatch_tail", "measurement_status": "complete",
            "warmup_pairs": warmup, "pairs_per_cell": PAIRS, "failures": 0, "timeouts": 0,
            "cells": cells, "samples": samples}


class ToolDispatchTailAnalysisTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.raw = tool_tail_raw()

    def test_individual_signed_p99_uses_matched_calls_and_keeps_correlation_limits(self):
        result = analyzer.analyze_tool_dispatch_tail(self.raw)
        self.assertEqual(len(result["summaries"]), 2)
        for cell in result["summaries"]:
            self.assertEqual(cell["paired_delta_estimate_ns"],
                             {"p50": -100, "p95": 100, "p99": 100, "n": PAIRS})
            self.assertEqual(cell["modes_ns"]["trusted_host"]["p99"], 200)
            self.assertEqual(cell["modes_ns"]["local_governed"]["p99"], 200)
            self.assertTrue(cell["empirical_p99_below_1ms"])
            self.assertEqual(cell["first_mode_strata"]["trusted_first"]["paired_delta_ns"]["p99"], -100)
            self.assertEqual(cell["first_mode_strata"]["governed_first"]["paired_delta_ns"]["p99"], 100)
            for stratum in cell["first_mode_strata"].values():
                self.assertEqual(stratum["paired_calls"], PAIRS // 2)
                self.assertNotIn("fixture_pairs", stratum)
        self.assertIn("correlated", result["scope"])
        self.assertIn("model", result["scope"])
        self.assertTrue(result["acceptance"].startswith("UNPROVEN:"))

    def test_p99_nearest_rank_and_one_millisecond_limit_are_strict(self):
        for delta, below in ((999999, True), (1000000, False)):
            raw = tool_tail_raw()
            for sample in raw["samples"]:
                sample["dispatch_ns"] = 100 if sample["mode"] == "trusted_host" else 100 + delta
                if sample["mode"] == "local_governed" and sample["pair"] >= 1980:
                    sample["dispatch_ns"] += 5000000
            result = analyzer.analyze_tool_dispatch_tail(raw)
            with self.subTest(delta=delta):
                for cell in result["summaries"]:
                    self.assertEqual(cell["paired_delta_estimate_ns"]["p99"], delta)
                    self.assertEqual(cell["empirical_p99_below_1ms"], below)

    def test_rejects_nonpositive_nonfinite_and_noninteger_raw_durations(self):
        for value in (0, -1, float("nan"), float("inf"), -float("inf"), True, 1.0, "1", None):
            with self.subTest(value=value), self.assertRaises(ValueError):
                analyzer.analyze_tool_dispatch_tail(replace_sample(self.raw, dispatch_ns=value))

    def test_requires_complete_unique_pairs_and_exact_call_ordinals(self):
        cases = [
            self.raw | {"samples": self.raw["samples"][1:]},
            self.raw | {"samples": self.raw["samples"] + [self.raw["samples"][0]]},
            replace_sample(self.raw, pair=True),
            replace_sample(self.raw, pair=1),
            replace_sample(self.raw, pair=PAIRS),
            replace_sample(self.raw, call_ordinal=101),
            replace_sample(self.raw, call_ordinal=100.0),
            replace_sample(self.raw, call_id="tool-tail-101"),
            replace_sample(self.raw, first_in_pair=False),
            replace_sample(self.raw, first_in_pair=1),
            replace_sample(self.raw, depth=True),
            replace_sample(self.raw, mode="unknown"),
            replace_sample(self.raw, audit_records_before=3),
        ]
        for index, raw in enumerate(cases):
            with self.subTest(case=index), self.assertRaises(ValueError):
                analyzer.analyze_tool_dispatch_tail(raw)

    def test_rejects_suite_mixups_changed_counts_and_unsuccessful_work(self):
        for changes in ({"schema": 2}, {"schema": 3.0}, {"suite": "representative"},
                        {"measurement_status": "budget_exhausted"}, {"failures": 1},
                        {"timeouts": 1}, {"failures": False}, {"pairs_per_cell": 32},
                        {"pairs_per_cell": 2000.0}, {"warmup_pairs": 20}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                analyzer.analyze_tool_dispatch_tail(self.raw | changes)

    def test_requires_both_fixture_modes_depths_and_actual_work_shape(self):
        for cells in (self.raw["cells"][1:], self.raw["cells"] + [self.raw["cells"][0]]):
            with self.subTest(cell_count=len(cells)), self.assertRaises(ValueError):
                analyzer.analyze_tool_dispatch_tail(self.raw | {"cells": cells})
        for changes in ({"old_rows": 251}, {"active_rows": True}, {"total_rows": 255},
                        {"contributor_ids": ["same"] * 4}, {"run_id": ""},
                        {"prefix_reads": 332}, {"prefix_audit_digest": [True] * 32},
                        {"prefix_audit_records": 1002}, {"final_reads": 2432},
                        {"final_audit_records": 3}, {"model_requests": 1}):
            cells = [self.raw["cells"][0] | changes] + self.raw["cells"][1:]
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                analyzer.analyze_tool_dispatch_tail(self.raw | {"cells": cells})

    def test_cli_routes_the_new_suite_and_rejects_json_nan(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "tool-tail.json"
            path.write_text(json.dumps(self.raw), encoding="utf-8")
            result = subprocess.run([sys.executable, str(SCRIPT), str(path)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout)["suite"], "tool_dispatch_tail")
            path.write_text(json.dumps(replace_sample(self.raw, dispatch_ns=float("nan"))), encoding="utf-8")
            result = subprocess.run([sys.executable, str(SCRIPT), str(path)], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("non-finite", result.stderr)


if __name__ == "__main__":
    unittest.main()
