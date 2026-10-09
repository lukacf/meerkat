#!/usr/bin/env python3
"""Summarize raw JSON from the ignored native_cost integration tests.

Usage: python3 scripts/analyze-native-cost.py RAW_JSON > summary.json

Build/smoke/matrix commands are documented in crates/meerkat-authorization/tests/native_cost.rs. This utility
validates the retained work and matched sample structure, then reports the
existing p50/p95/p99 summaries and paired differences. It does not run a binary,
monitor resources, verify a build identity or turn a summary into acceptance.
"""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path


def quantiles(values):
    if not values:
        raise ValueError("empty timing series")
    ordered = sorted(values)
    return {name: ordered[max(0, math.ceil(q * len(ordered)) - 1)]
            for name, q in [("p50", .50), ("p95", .95), ("p99", .99)]} | {"n": len(ordered)}

def positive_duration(value):
    return type(value) is int and value > 0


def validate_pair_order(raw, pairs):
    warmup = raw["warmup_pairs"]
    if type(warmup) is not int or not 20 <= warmup <= 500:
        raise ValueError("warmup pairs must match the producer's integer range")
    for pair_id, pair in pairs.items():
        if type(pair_id) is not int:
            raise ValueError("pair ID must be an integer")
        baseline, candidate = pair["trusted_host"], pair["local_governed"]
        if any(type(sample["first_in_pair"]) is not bool for sample in pair.values()):
            raise ValueError("first_in_pair must be a boolean")
        baseline_first = (pair_id + warmup) % 2 == 0
        if (baseline["first_in_pair"] != baseline_first
                or candidate["first_in_pair"] == baseline_first):
            raise ValueError("pair order disagrees with warmup plus pair parity")


def paired_metric_summary(pairs, durations):
    pairs = list(pairs)
    baseline = [value for pair in pairs for value in durations(pair["trusted_host"])]
    candidate = [value for pair in pairs for value in durations(pair["local_governed"])]
    if (not baseline or len(baseline) != len(candidate)
            or any(not positive_duration(value) for value in baseline + candidate)):
        raise ValueError("timings must be matched positive integer nanoseconds")
    return {
        "fixture_pairs": len(pairs),
        "modes_ns": {"trusted_host": quantiles(baseline), "local_governed": quantiles(candidate)},
        "paired_delta_ns": quantiles([y - x for x, y in zip(baseline, candidate)]),
        "paired_overhead_percent": quantiles([100.0 * (y / x - 1) for x, y in zip(baseline, candidate)]),
        "ratio_of_means_overhead_percent": 100.0 * (sum(candidate) / sum(baseline) - 1),
    }


def first_mode_strata(pairs, durations):
    return {
        name: paired_metric_summary(
            [pair for pair in pairs.values() if pair["trusted_host"]["first_in_pair"] == first],
            durations,
        )
        for name, first in [("trusted_first", True), ("governed_first", False)]
    }

def analyze(raw):
    if any(type(raw[field]) is not int or raw[field] < 0 for field in ("failures", "timeouts")):
        raise ValueError("failure and timeout counts must be nonnegative integers")
    profile = raw.get("measurement_profile")
    if profile not in {None, "model_dispatch_tail"}:
        raise ValueError("unknown native cost measurement profile")
    expected = raw["pairs_per_cell"]
    if type(expected) is not int or not 2000 <= expected <= 10000:
        raise ValueError("sample count must match the producer's tail range")
    if profile == "model_dispatch_tail" and (
            type(raw["warmup_pairs"]) is not int or raw["warmup_pairs"] != 100 or expected != 2000):
        raise ValueError("model tail requires the fixed W100/N2000 plan")
    incomplete = False
    if profile == "model_dispatch_tail":
        if raw.get("measurement_status") not in {"complete", "budget_exhausted"}:
            raise ValueError("missing model tail completion status")
        incomplete = raw["measurement_status"] == "budget_exhausted"
        if incomplete and not raw["timeouts"]:
            raise ValueError("budget exhaustion must retain its timeout")
    if (raw["failures"] or raw["timeouts"]) and not incomplete:
        raise ValueError("failed work cannot become a performance result")
    instrumentations = ["boundaries"] if profile == "model_dispatch_tail" else ["turn_only", "boundaries"]
    native_spans = profile == "model_dispatch_tail" or any(
        "native_model_preparation_ns" in sample for sample in raw["samples"]
    )
    model_metrics = ["model_authorization_ns"]
    if native_spans:
        model_metrics.append("native_model_preparation_ns")
    seen = set()
    for sample in raw["samples"]:
        if (type(sample["depth"]) is not int or sample["depth"] not in {1, 3}
                or sample["instrumentation"] not in instrumentations
                or sample["mode"] not in {"trusted_host", "local_governed"}):
            raise ValueError("unknown sample cell")
        if type(sample["pair"]) is not int or not 0 <= sample["pair"] < expected:
            raise ValueError("pair ID must be an integer in the declared range")
        key = (sample["depth"], sample["instrumentation"], sample["pair"], sample["mode"])
        if key in seen:
            raise ValueError("duplicate sample")
        seen.add(key)
        if incomplete:
            baseline_first = (sample["pair"] + raw["warmup_pairs"]) % 2 == 0
            if (type(sample["first_in_pair"]) is not bool
                    or sample["first_in_pair"] != (baseline_first == (sample["mode"] == "trusted_host"))):
                raise ValueError("partial sample order disagrees with the fixed plan")
    summaries = []
    incomplete_cells = []
    for depth in [1, 3]:
        for instrumentation in instrumentations:
            subset = [s for s in raw["samples"] if s["depth"] == depth and s["instrumentation"] == instrumentation]
            by_mode = {mode: [s for s in subset if s["mode"] == mode] for mode in ["trusted_host", "local_governed"]}
            if not incomplete and any(len(rows) != expected for rows in by_mode.values()):
                raise ValueError("incomplete matched cell")
            for mode, rows in by_mode.items():
                for s in rows:
                    if (any(type(s[field]) is not int for field in ("model_requests", "read_effects", "audit_records"))
                            or (s["model_requests"], s["read_effects"], s["audit_records"]) != (2, 4, 18 if mode == "local_governed" else 0)):
                        raise ValueError("missing work/audit")
                    expected_models = s["model_requests"] if instrumentation == "boundaries" else 0
                    durations = [s["turn_ns"]]
                    for metric in model_metrics:
                        models = s.get(metric)
                        if not isinstance(models, list) or len(models) != expected_models:
                            raise ValueError(f"{metric} must match actual request cardinality")
                        durations.extend(models)
                    if instrumentation == "boundaries":
                        durations.append(s["tool_batch_ns"])
                    elif s["tool_batch_ns"] is not None:
                        raise ValueError("turn-only samples must not contain batch timings")
                    if any(not positive_duration(value) for value in durations):
                        raise ValueError("timings must be positive integer nanoseconds")
            if incomplete:
                incomplete_cells.append({
                    "depth": depth, "instrumentation": instrumentation,
                    "expected_per_mode": expected,
                    "sample_counts": {mode: len(rows) for mode, rows in by_mode.items()},
                    "complete": all(len(rows) == expected for rows in by_mode.values()),
                })
                continue
            entry = {"depth": depth, "instrumentation": instrumentation, "modes": {}}
            for mode, rows in by_mode.items():
                metrics = {"turn_ns": quantiles([s["turn_ns"] for s in rows])}
                if instrumentation == "boundaries":
                    for metric in model_metrics:
                        metrics[metric] = quantiles([v for s in rows for v in s[metric]])
                    metrics["tool_batch_ns"] = quantiles([s["tool_batch_ns"] for s in rows])
                entry["modes"][mode] = metrics
            pairs = {}
            for s in subset:
                if s["mode"] in pairs.setdefault(s["pair"], {}):
                    raise ValueError("duplicate pair")
                pairs[s["pair"]][s["mode"]] = s
            if set(pairs) != set(range(expected)):
                raise ValueError("pair IDs not complete")
            if any(set(pair) != set(by_mode) for pair in pairs.values()):
                raise ValueError("pair must contain both modes")
            validate_pair_order(raw, pairs)
            deltas, ratios = [], []
            for p in pairs.values():
                baseline, candidate = p["trusted_host"], p["local_governed"]
                if baseline["first_in_pair"] == candidate["first_in_pair"]:
                    raise ValueError("invalid alternating order")
                deltas.append(candidate["turn_ns"] - baseline["turn_ns"])
                ratios.append(100.0 * (candidate["turn_ns"] / baseline["turn_ns"] - 1))
            entry["paired_turn_delta_ns"] = quantiles(deltas)
            entry["paired_turn_overhead_percent"] = quantiles(ratios)
            turn_durations = lambda sample: [sample["turn_ns"]]
            entry["turn_ratio_of_means_overhead_percent"] = paired_metric_summary(
                pairs.values(), turn_durations
            )["ratio_of_means_overhead_percent"]
            entry["first_mode_strata"] = {"turn_ns": first_mode_strata(pairs, turn_durations)}
            if instrumentation == "boundaries":
                for metric in model_metrics:
                    # Two sequential requests belong to one fixture. Pair each
                    # ordinal within the same fixture, preserving correlation.
                    by_request = [
                        [pair["local_governed"][metric][index] - pair["trusted_host"][metric][index]
                         for pair in pairs.values()]
                        for index in range(2)
                    ]
                    stem = metric.removesuffix("_ns")
                    entry[f"paired_{stem}_delta_ns"] = quantiles(
                        [value for values in by_request for value in values]
                    )
                    entry[f"paired_{stem}_delta_ns_by_request_index"] = [
                        {"request_index": index, **quantiles(values)}
                        for index, values in enumerate(by_request)
                    ]
                    entry["first_mode_strata"][f"{metric}_by_request_index"] = [
                        {"request_index": index, "strata": first_mode_strata(
                            pairs, lambda sample, index=index, metric=metric: [sample[metric][index]]
                        )}
                        for index in range(2)
                    ]
                entry["first_mode_strata"]["tool_batch_ns"] = first_mode_strata(
                    pairs, lambda sample: [sample["tool_batch_ns"]]
                )
                entry["model_authorization_interpretation"] = (
                    "paired provider-side authorization boundary diagnostic only; "
                    "excludes native admission and tool work"
                )
                if native_spans:
                    entry["native_model_preparation_interpretation"] = (
                        "CallingLlm before notice refresh through native Entry/currentness before scripted transport; "
                        "two correlated request spans per fixture, paired by request ordinal; excludes earlier admission, "
                        "real provider serialization/header refresh, network and Outcome auditing"
                    )
            entry["turn_interpretation"] = "gate candidate" if instrumentation == "turn_only" else "instrumented diagnostic only"
            summaries.append(entry)
    if incomplete:
        return {"analysis_status": "INCOMPLETE", "measurement_profile": profile,
                "summaries": [], "cells": incomplete_cells,
                "acceptance": "UNPROVEN: model tail budget exhausted; retained counts only, no timing summary"}
    result = {"summaries": summaries, "unsupported": raw["unsupported"],
              "acceptance": "UNPROVEN: separate representative mean, individual tool and full-surface evidence remain required",
              "tail_confidence": "empirical paired costs; two model requests per fixture are correlated; no independent tail confidence"}
    if not native_spans:
        result["acceptance"] += "; native model preparation absent in legacy raw"
    if profile is not None:
        result["measurement_profile"] = profile
    return result

# Fixed plan: six simultaneous two-sided intervals, alpha .05/6 each.
# t(15) CDF at 1-.05/(2*6) is 3.0362832228211785; round upward.
# NIST paired Fieller formula and t table are linked in the application note.
FIXED_MEAN_CRITICAL_T = 3.036284


def fixed_mean_uncertain(reason):
    return {"analysis_status": "UNCERTAIN", "reason": reason, "summaries": [],
            "acceptance": "UNPROVEN: incomplete or failed fixed mean study; no tail or full-surface acceptance"}


def fixed_mean_cell(depth, workload, pairs):
    direct = workload == "individual_fenced_tools"
    duration = (lambda sample: sum(call[1] for call in sample["individual_ns"])) if direct else (lambda sample: sample["measured_ns"])
    values = [(duration(pairs[i]["trusted_host"]), duration(pairs[i]["local_governed"])) for i in range(32)]
    # Each adjacent block contains both execution orders. The four direct
    # calls stay one correlated fixture unit; none is an independent replicate.
    blocks = [tuple((values[i][j] + values[i+1][j]) / 2 for j in range(2)) for i in range(0, 32, 2)]
    x, y = (math.fsum(row[j] for row in blocks) / 16 for j in range(2))
    vx = math.fsum((a-x)**2 for a, b in blocks) / (16*15)
    vy = math.fsum((b-y)**2 for a, b in blocks) / (16*15)
    covariance = math.fsum((a-x)*(b-y) for a, b in blocks) / (16*15)
    difference = y - 1.10*x
    vd = math.fsum(((b-1.10*a)-difference)**2 for a, b in blocks) / (16*15)
    q2 = FIXED_MEAN_CRITICAL_T**2
    aa, bb, cc = x*x-q2*vx, x*y-q2*covariance, y*y-q2*vy
    discriminant = bb*bb-aa*cc
    ratio_ci = None
    difference_ci = None
    classification = "UNCERTAIN"
    reason = "unbounded, degenerate or numerically inconsistent confidence interval"
    if aa > 0 and discriminant >= 0 and vd > 0:
        root = math.sqrt(discriminant)
        ratio_ci = [(bb-root)/aa, (bb+root)/aa]
        width = FIXED_MEAN_CRITICAL_T*math.sqrt(vd)
        difference_ci = [difference-width, difference+width]
        if all(math.isfinite(v) for v in ratio_ci + difference_ci):
            ratio_decision = "CLEAR" if ratio_ci[1] <= 1.10 else "MISS" if ratio_ci[0] > 1.10 else "UNCERTAIN"
            difference_decision = "CLEAR" if difference_ci[1] <= 0 else "MISS" if difference_ci[0] > 0 else "UNCERTAIN"
            if ratio_decision == difference_decision:
                classification = ratio_decision
                reason = "conditional mean interval relative to 10 percent"
        else:
            ratio_ci = difference_ci = None
    strata = {}
    for name, first in [("trusted_first", True), ("governed_first", False)]:
        rows = [values[i] for i in range(32) if pairs[i]["trusted_host"]["first_in_pair"] == first]
        strata[name] = {"fixture_pairs": len(rows), "ratio_of_means": math.fsum(b for a, b in rows)/math.fsum(a for a, b in rows)}
    if ((classification == "CLEAR" and any(s["ratio_of_means"] > 1.10 for s in strata.values()))
            or (classification == "MISS" and any(s["ratio_of_means"] <= 1.10 for s in strata.values()))):
        classification, reason = "UNCERTAIN", "descriptive order strata contradict the pooled threshold direction"
    return {"depth": depth, "workload": workload, "fixture_pairs": 32,
            "independent_candidate_blocks": 16,
            "modes_ns": {"trusted_host": {"n": 32, "mean": x}, "local_governed": {"n": 32, "mean": y}},
            "ratio_of_means_overhead_percent": 100*(y/x-1),
            "ratio_ci": ratio_ci, "threshold_difference_mean_ns": difference,
            "threshold_difference_ci_ns": difference_ci,
            "conditional_classification": classification, "classification_reason": reason,
            "order_strata": strata,
            "interpretation": "sum of four correlated fenced calls per fixture; excludes Agent scheduling" if direct else "same-run post-prefix segment; not admission" if workload == "continuing_segment" else "four-input fresh-admission whole turn"}


def fixed_mean_result(summaries):
    return {"analysis_status": "COMPLETE", "measurement_profile": "fixed_mean_32", "summaries": summaries,
            "confidence_model": {"method": "paired Fieller on adjacent opposite-order blocks",
                "family_comparisons": 6, "family_alpha": .05, "degrees_of_freedom": 15,
                "critical_t": FIXED_MEAN_CRITICAL_T,
                "qualification": "conditional on stationary independent approximately bivariate-normal block vectors; no distribution-free guarantee; order strata descriptive only"},
            "acceptance": "UNPROVEN: conditional fixed mean study only; operation p99 and full-surface acceptance absent",
            "scope": "Only fresh_admission is a whole-turn comparison. Continuing segments and summed direct calls are diagnostic units. Source, build, quiet-host and independence qualification remain external."}


def analyze_representative(raw):
    profile = raw.get("measurement_profile")
    if profile not in {None, "tail", "fixed_mean_32"} or raw.get("suite") != "representative":
        raise ValueError("unknown representative measurement profile or suite")
    mean_only = profile == "fixed_mean_32"
    expected = raw["pairs_per_cell"]
    if mean_only:
        if (type(expected) is not int or expected != 32
                or type(raw["warmup_pairs"]) is not int or raw["warmup_pairs"] != 20
                or raw["measurement_status"] not in {"complete", "budget_exhausted"}):
            raise ValueError("fixed mean profile requires exact W20/N32 and an explicit completion status")
        if any(type(raw[field]) is not int or raw[field] < 0 for field in ("failures", "timeouts")):
            raise ValueError("invalid fixed mean failure counts")
        if raw["measurement_status"] != "complete" or raw["failures"] or raw["timeouts"]:
            return fixed_mean_uncertain("budget exhausted or work failed; incomplete cells cannot be analyzed")
    elif raw["failures"] or raw["timeouts"] or expected < 2000:
        raise ValueError("unsuccessful or insufficient tail samples")
    for sample in raw["samples"]:
        if type(sample["depth"]) is not int or sample["depth"] not in {1, 3}:
            raise ValueError("unknown sample depth")
    summaries = []
    observed = set()
    for depth in [1, 3]:
        for workload in ["fresh_admission", "continuing_segment", "individual_fenced_tools"]:
            subset = [s for s in raw["samples"] if s["depth"] == depth and s["workload"] == workload]
            prefix = workload != "fresh_admission"
            direct = workload == "individual_fenced_tools"
            calls = ["direct-" + str(i) for i in range(4)] if direct else ["read-" + str(i) for i in range(4)]
            pairs = {}
            for s in subset:
                if type(s["pair"]) is not int:
                    raise ValueError("pair ID must be an integer")
                mode = s["mode"]
                if mode not in {"trusted_host", "local_governed"}:
                    raise ValueError("unknown mode")
                key = (depth, workload, s["pair"], mode)
                if key in observed:
                    raise ValueError("duplicate sample")
                observed.add(key)
                governed = mode == "local_governed"
                if (s["old_rows"], s["active_rows"], s["total_rows"]) != (252, 4, 256) or len(set(s["contributor_ids"])) != 4 or len(s["contributor_ids"]) != 4 or not s["run_id"]:
                    raise ValueError("missing real row/contributor/run shape")
                if len(s["prefix_audit_digest"]) != 32 or any(not isinstance(v, int) or not 0 <= v <= 255 for v in s["prefix_audit_digest"]):
                    raise ValueError("missing exact audit-prefix digest")
                final_audit = (1017 if direct else 1020 if prefix else 18) if governed else 0
                if s["prefix_audit_records"] != (1002 if prefix and governed else 0) or s["final_audit_records"] != final_audit:
                    raise ValueError("missing prefix/suffix observations")
                if s["prefix_reads"] != (333 if prefix else 0) or s["final_reads"] != (337 if prefix else 4) or s["model_requests"] != (3 if workload == "continuing_segment" else 2):
                    raise ValueError("missing physical/model work")
                operation_count = (4 if direct else 6) if governed else 0
                if len(s["measured_operation_ids"]) != operation_count or len(set(s["measured_operation_ids"])) != operation_count or s["measured_call_ids"] != calls:
                    raise ValueError("missing or duplicate measured operation")
                if direct:
                    if s["measured_ns"] is not None or [c[0] for c in s["individual_ns"]] != calls or any(not positive_duration(c[1]) for c in s["individual_ns"]):
                        raise ValueError("invalid individual fenced durations")
                elif not positive_duration(s["measured_ns"]) or s["individual_ns"]:
                    raise ValueError("invalid full interval")
                pairs.setdefault(s["pair"], {})[mode] = s
            if set(pairs) != set(range(expected)) or any(set(p) != {"trusted_host", "local_governed"} for p in pairs.values()):
                if mean_only:
                    return fixed_mean_uncertain("incomplete fixed mean cell; all six cells are required")
                raise ValueError("incomplete matched cell")
            validate_pair_order(raw, pairs)
            if mean_only:
                summaries.append(fixed_mean_cell(depth, workload, pairs))
                continue
            deltas, ratios = [], []
            modes = {mode: [] for mode in ["trusted_host", "local_governed"]}
            for pair in pairs.values():
                a, b = pair["trusted_host"], pair["local_governed"]
                if a["first_in_pair"] == b["first_in_pair"]:
                    raise ValueError("invalid paired order")
                av = [c[1] for c in a["individual_ns"]] if direct else [a["measured_ns"]]
                bv = [c[1] for c in b["individual_ns"]] if direct else [b["measured_ns"]]
                modes["trusted_host"].extend(av)
                modes["local_governed"].extend(bv)
                deltas.extend(y - x for x, y in zip(av, bv))
                ratios.extend(100.0 * (y / x - 1) for x, y in zip(av, bv))
            durations = (lambda sample: [call[1] for call in sample["individual_ns"]]) if direct else (lambda sample: [sample["measured_ns"]])
            entry = {"depth": depth, "workload": workload, "modes_ns": {m: quantiles(v) for m, v in modes.items()},
                     "paired_delta_ns": quantiles(deltas), "paired_overhead_percent": quantiles(ratios),
                     "fixture_pairs": len(pairs), "first_mode_strata": first_mode_strata(pairs, durations),
                     "interpretation": "public fenced boundary; four correlated calls per fixture; excludes Agent scheduling" if direct else "same-run post-prefix segment; not admission" if prefix else "four-input fresh-admission whole turn"}
            if not direct:
                entry["ratio_of_means_overhead_percent"] = paired_metric_summary(
                    pairs.values(), durations
                )["ratio_of_means_overhead_percent"]
            else:
                entry["by_call_id"] = [
                    {"call_id": call_id, **paired_metric_summary(
                        pairs.values(), lambda sample, index=index: [sample["individual_ns"][index][1]]
                    ), "first_mode_strata": first_mode_strata(
                        pairs, lambda sample, index=index: [sample["individual_ns"][index][1]]
                    )}
                    for index, call_id in enumerate(calls)
                ]
            summaries.append(entry)
    if len(observed) != len(raw["samples"]):
        raise ValueError("unknown or unaccounted sample cell")
    if mean_only:
        return fixed_mean_result(summaries)
    return {"summaries": summaries,
            "acceptance": "UNPROVEN: source-authored subset; qualified repeated quiet runs and full-surface review still required",
            "limits": {"individual_added_p99_ns_strictly_below": 1000000, "matched_turn_overhead_percent_at_most": 10},
            "scope": "Do not substitute continuing-segment or direct-fenced diagnostics for Agent whole-turn cost."}


def analyze_tool_dispatch_tail(raw):
    """Empirical paired dispatch tails from two retained runs per depth.

    Calls within each run are correlated and retain a growing audit history.
    These quantiles are descriptive estimates, not independent tail inference.
    """
    if (not isinstance(raw, dict) or type(raw.get("schema")) is not int
            or raw["schema"] != 3 or raw.get("suite") != "tool_dispatch_tail"
            or raw.get("measurement_status") != "complete"):
        raise ValueError("expected complete tool_dispatch_tail schema 3")
    for field, expected in (("warmup_pairs", 100), ("pairs_per_cell", 2000),
                            ("failures", 0), ("timeouts", 0)):
        if type(raw.get(field)) is not int or raw[field] != expected:
            raise ValueError("tool tail requires fixed W100/N2000 and successful work")
    expected_cells = {(depth, mode) for depth in (1, 3)
                      for mode in ("trusted_host", "local_governed")}
    if not isinstance(raw.get("cells"), list) or not isinstance(raw.get("samples"), list):
        raise ValueError("tool tail requires fixture cells and individual samples")
    cells = {}
    for cell in raw["cells"]:
        if not isinstance(cell, dict) or type(cell.get("depth")) is not int:
            raise ValueError("invalid tool-tail fixture")
        key = (cell["depth"], cell.get("mode"))
        if key not in expected_cells or key in cells:
            raise ValueError("unknown or duplicate tool-tail fixture")
        governed = key[1] == "local_governed"
        counts = {"old_rows": 252, "active_rows": 4, "total_rows": 256,
                  "prefix_reads": 333, "final_reads": 2433, "model_requests": 2,
                  "prefix_audit_records": 1002 if governed else 0,
                  "final_audit_records": 7305 if governed else 0}
        if any(type(cell.get(field)) is not int or cell[field] != value
               for field, value in counts.items()):
            raise ValueError("missing tool-tail physical work, retained rows or audit")
        contributors = cell.get("contributor_ids")
        if (not isinstance(contributors, list) or len(contributors) != 4
                or any(not isinstance(value, str) or not value for value in contributors)
                or len(set(contributors)) != 4
                or not isinstance(cell.get("run_id"), str) or not cell["run_id"]):
            raise ValueError("missing actual tool-tail contributor/run identity")
        digest = cell.get("prefix_audit_digest")
        if (not isinstance(digest, list) or len(digest) != 32
                or any(type(value) is not int or not 0 <= value <= 255 for value in digest)):
            raise ValueError("missing exact tool-tail audit-prefix digest")
        cells[key] = cell
    if set(cells) != expected_cells:
        raise ValueError("incomplete tool-tail fixtures")
    by_depth = {depth: {} for depth in (1, 3)}
    for sample in raw["samples"]:
        if not isinstance(sample, dict) or type(sample.get("depth")) is not int:
            raise ValueError("invalid tool-tail sample")
        key = (sample["depth"], sample.get("mode"))
        if key not in cells:
            raise ValueError("unknown tool-tail sample cell")
        pair = sample.get("pair")
        if type(pair) is not int or not 0 <= pair < 2000:
            raise ValueError("invalid tool-tail pair ordinal")
        ordinal = 100 + pair
        if (type(sample.get("call_ordinal")) is not int or sample["call_ordinal"] != ordinal
                or sample.get("call_id") != f"tool-tail-{ordinal}"):
            raise ValueError("call identity must match the exact post-warmup ordinal")
        audit_before = 1002 + 3 * ordinal if key[1] == "local_governed" else 0
        if (type(sample.get("audit_records_before")) is not int
                or sample["audit_records_before"] != audit_before):
            raise ValueError("tool-tail audit-growth position does not match the call")
        duration = sample.get("dispatch_ns")
        if not positive_duration(duration) or duration > 2**64 - 1:
            raise ValueError("dispatch duration must be positive integer u64 nanoseconds")
        modes = by_depth[key[0]].setdefault(pair, {})
        if key[1] in modes:
            raise ValueError("duplicate tool-tail pair/call ordinal")
        modes[key[1]] = sample
    summaries = []
    for depth, pairs in by_depth.items():
        if (set(pairs) != set(range(2000))
                or any(set(modes) != {"trusted_host", "local_governed"} for modes in pairs.values())):
            raise ValueError("incomplete matched tool-tail pairs")
        validate_pair_order(raw, pairs)
        durations = lambda sample: [sample["dispatch_ns"]]
        metric = paired_metric_summary(pairs.values(), durations)
        strata = first_mode_strata(pairs, durations)
        for stratum in strata.values():
            stratum["paired_calls"] = stratum.pop("fixture_pairs")
        summaries.append({
            "depth": depth, "paired_calls": 2000, "retained_runs": 2,
            "modes_ns": metric["modes_ns"],
            "paired_delta_estimate_ns": metric["paired_delta_ns"],
            "empirical_p99_below_1ms": metric["paired_delta_ns"]["p99"] < 1000000,
            "first_mode_strata": strata,
        })
    return {"suite": "tool_dispatch_tail", "analysis_status": "COMPLETE", "summaries": summaries,
            "limit_ns_strictly_below": 1000000,
            "scope": "Full resolve/validate/fenced dispatch including actual file read and audit; signed paired differences estimate added authorization cost. Calls are correlated within retained runs with growing audit history; no independent confidence claim. Excludes Agent scheduling, model preparation and full-profile coverage.",
            "acceptance": "UNPROVEN: empirical warm tool-dispatch subset only; repeated qualified windows and remaining operation/profile coverage required"}


def reject_nonfinite(value):
    raise ValueError("non-finite JSON number: " + value)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("raw_json", type=Path, help="minimal schema 1, representative schema 2 or tool-tail schema 3 raw JSON")
    args = parser.parse_args()
    try:
        raw = json.loads(args.raw_json.read_text(encoding="utf-8"), parse_constant=reject_nonfinite)
        if not isinstance(raw, dict) or type(raw.get("schema")) is not int:
            raise ValueError("raw JSON must be an object with an integer schema")
        if raw["schema"] == 1:
            summary = analyze(raw)
        elif raw["schema"] == 2:
            summary = analyze_representative(raw)
        elif raw["schema"] == 3:
            summary = analyze_tool_dispatch_tail(raw)
        else:
            raise ValueError("unsupported native cost schema")
        output = json.dumps(summary, indent=2, allow_nan=False)
    except (OSError, ValueError, KeyError, TypeError, IndexError, ZeroDivisionError, OverflowError) as error:
        parser.error("invalid raw input: " + str(error))
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
