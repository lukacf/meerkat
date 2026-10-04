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
    if raw["failures"] or raw["timeouts"]:
        raise ValueError("failed work cannot become a performance result")
    for sample in raw["samples"]:
        if (type(sample["depth"]) is not int or sample["depth"] not in {1, 3}
                or sample["instrumentation"] not in {"turn_only", "boundaries"}
                or sample["mode"] not in {"trusted_host", "local_governed"}):
            raise ValueError("unknown sample cell")
        if type(sample["pair"]) is not int:
            raise ValueError("pair ID must be an integer")
    summaries = []
    for depth in [1, 3]:
        for instrumentation in ["turn_only", "boundaries"]:
            subset = [s for s in raw["samples"] if s["depth"] == depth and s["instrumentation"] == instrumentation]
            by_mode = {mode: [s for s in subset if s["mode"] == mode] for mode in ["trusted_host", "local_governed"]}
            expected = raw["pairs_per_cell"]
            if any(len(rows) != expected for rows in by_mode.values()):
                raise ValueError("incomplete matched cell")
            if expected < 2000:
                raise ValueError("not enough samples for declared tail report")
            for mode, rows in by_mode.items():
                for s in rows:
                    if (s["model_requests"], s["read_effects"], s["audit_records"]) != (2, 4, 18 if mode == "local_governed" else 0):
                        raise ValueError("missing work/audit")
                    models = s.get("model_authorization_ns")
                    expected_models = s["model_requests"] if instrumentation == "boundaries" else 0
                    if not isinstance(models, list) or len(models) != expected_models:
                        raise ValueError("model timings must match actual request cardinality")
                    durations = [s["turn_ns"]] + models
                    if instrumentation == "boundaries":
                        durations.append(s["tool_batch_ns"])
                    elif s["tool_batch_ns"] is not None:
                        raise ValueError("turn-only samples must not contain batch timings")
                    if any(not positive_duration(value) for value in durations):
                        raise ValueError("timings must be positive integer nanoseconds")
            entry = {"depth": depth, "instrumentation": instrumentation, "modes": {}}
            for mode, rows in by_mode.items():
                metrics = {"turn_ns": quantiles([s["turn_ns"] for s in rows])}
                if instrumentation == "boundaries":
                    metrics["model_authorization_ns"] = quantiles([v for s in rows for v in s["model_authorization_ns"]])
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
            model_deltas_by_request = [[], []]
            for p in pairs.values():
                baseline, candidate = p["trusted_host"], p["local_governed"]
                if baseline["first_in_pair"] == candidate["first_in_pair"]:
                    raise ValueError("invalid alternating order")
                deltas.append(candidate["turn_ns"] - baseline["turn_ns"])
                ratios.append(100.0 * (candidate["turn_ns"] / baseline["turn_ns"] - 1))
                if instrumentation == "boundaries":
                    # The retained fixture records two sequential requests in
                    # encounter order. Join the same pair and request index;
                    # marginal quantiles are not paired differences.
                    for index in range(baseline["model_requests"]):
                        model_deltas_by_request[index].append(
                            candidate["model_authorization_ns"][index]
                            - baseline["model_authorization_ns"][index]
                        )
            entry["paired_turn_delta_ns"] = quantiles(deltas)
            entry["paired_turn_overhead_percent"] = quantiles(ratios)
            turn_durations = lambda sample: [sample["turn_ns"]]
            entry["turn_ratio_of_means_overhead_percent"] = paired_metric_summary(
                pairs.values(), turn_durations
            )["ratio_of_means_overhead_percent"]
            entry["first_mode_strata"] = {"turn_ns": first_mode_strata(pairs, turn_durations)}
            if instrumentation == "boundaries":
                entry["first_mode_strata"]["model_authorization_ns_by_request_index"] = [
                    {"request_index": index, "strata": first_mode_strata(
                        pairs, lambda sample, index=index: [sample["model_authorization_ns"][index]]
                    )}
                    for index in range(2)
                ]
                entry["first_mode_strata"]["tool_batch_ns"] = first_mode_strata(
                    pairs, lambda sample: [sample["tool_batch_ns"]]
                )
                entry["paired_model_authorization_delta_ns"] = quantiles(
                    [value for values in model_deltas_by_request for value in values]
                )
                entry["paired_model_authorization_delta_ns_by_request_index"] = [
                    {"request_index": index, **quantiles(values)}
                    for index, values in enumerate(model_deltas_by_request)
                ]
                entry["model_authorization_interpretation"] = (
                    "paired provider-side authorization boundary diagnostic only; "
                    "excludes native admission and tool work"
                )
            entry["turn_interpretation"] = "gate candidate" if instrumentation == "turn_only" else "instrumented diagnostic only"
            summaries.append(entry)
    return {"summaries": summaries, "unsupported": raw["unsupported"],
            "acceptance": "UNPROVEN: full representative matrix and individual tool cost absent",
            "tail_confidence": "2000 turns per mode/cell, about 20 upper-1-percent observations; repeat independent quiet windows before inference"}

def analyze_representative(raw):
    if raw.get("suite") != "representative" or raw["failures"] or raw["timeouts"]:
        raise ValueError("not a successful representative suite")
    expected = raw["pairs_per_cell"]
    if expected < 2000:
        raise ValueError("insufficient tail samples")
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
                raise ValueError("incomplete matched cell")
            validate_pair_order(raw, pairs)
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
    return {"summaries": summaries,
            "acceptance": "UNPROVEN: source-authored subset; qualified repeated quiet runs and full-surface review still required",
            "limits": {"individual_added_p99_ns_strictly_below": 1000000, "matched_turn_overhead_percent_at_most": 10},
            "scope": "Do not substitute continuing-segment or direct-fenced diagnostics for Agent whole-turn cost."}


def reject_nonfinite(value):
    raise ValueError("non-finite JSON number: " + value)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("raw_json", type=Path, help="minimal schema 1 or representative schema 2 raw JSON")
    args = parser.parse_args()
    try:
        raw = json.loads(args.raw_json.read_text(encoding="utf-8"), parse_constant=reject_nonfinite)
        if not isinstance(raw, dict) or type(raw.get("schema")) is not int:
            raise ValueError("raw JSON must be an object with an integer schema")
        if raw["schema"] == 1:
            summary = analyze(raw)
        elif raw["schema"] == 2:
            summary = analyze_representative(raw)
        else:
            raise ValueError("unsupported native cost schema")
        output = json.dumps(summary, indent=2, allow_nan=False)
    except (OSError, ValueError, KeyError, TypeError, IndexError, ZeroDivisionError, OverflowError) as error:
        parser.error("invalid raw input: " + str(error))
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
