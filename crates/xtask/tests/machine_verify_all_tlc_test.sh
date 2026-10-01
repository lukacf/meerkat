#!/usr/bin/env bash
set -euo pipefail

xtask_bin="${1:?xtask binary path is required}"
if [[ "${xtask_bin}" != /* ]]; then
  if [[ -x "${PWD}/${xtask_bin}" ]]; then
    xtask_bin="${PWD}/${xtask_bin}"
  else
    xtask_bin="${TEST_SRCDIR:?}/${TEST_WORKSPACE:?}/${xtask_bin}"
  fi
fi

if [[ -n "${RUSTFMT:-}" && "${RUSTFMT}" != /* ]]; then
  if [[ -x "${PWD}/${RUSTFMT}" ]]; then
    RUSTFMT="${PWD}/${RUSTFMT}"
  else
    RUSTFMT="${TEST_SRCDIR:?}/${TEST_WORKSPACE:?}/${RUSTFMT}"
  fi
  export RUSTFMT
fi

# Fail closed: this lane advertises TLC-backed verification (it is named
# `machine_verify_all_tlc_test` and blessed by buildbuddy-doctor as
# "machine-verify/TLC"). Silently degrading to a drift-only `machine-check-drift`
# pass when `tlc` is absent would launder a weaker check as TLC verification.
#
# If `tlc` is missing, the lane MUST fail unless a caller has explicitly opted
# into a drift-only run by exporting MACHINE_VERIFY_TLC_DRIFT_ONLY=1. That
# opt-in is deliberately off by default so the absence of `tlc` on a lane that
# claims TLC is treated as a hard failure, not a quiet downgrade.
if ! command -v tlc >/dev/null 2>&1; then
  if [[ "${MACHINE_VERIFY_TLC_DRIFT_ONLY:-0}" == "1" ]]; then
    echo "MACHINE_VERIFY_TLC_DRIFT_ONLY=1: tlc absent, running drift-only machine-check-drift (NOT TLC verification)"
    exec "${xtask_bin}" machine-check-drift --all
  fi
  echo "error: tlc not on PATH but this lane advertises TLC-backed verification." >&2
  echo "       Provision tlc on the lane, or set MACHINE_VERIFY_TLC_DRIFT_ONLY=1 to" >&2
  echo "       explicitly run the weaker drift-only check (which is NOT TLC)." >&2
  exit 1
fi

workspace_root="${PWD}"
if [[ -n "${TEST_SRCDIR:-}" && -n "${TEST_WORKSPACE:-}" && -d "${TEST_SRCDIR}/${TEST_WORKSPACE}" ]]; then
  workspace_root="${TEST_SRCDIR}/${TEST_WORKSPACE}"
fi

adaptive_model="${workspace_root}/specs/compositions/adaptive_mob_bundle/model.tla"
adaptive_witness="${workspace_root}/specs/compositions/adaptive_mob_bundle/witness-layer_terminal_feedback.cfg"
if [[ ! -f "${adaptive_model}" || ! -f "${adaptive_witness}" ]]; then
  echo "error: adaptive_mob_bundle bounded TLC witness files are missing from workspace runfiles." >&2
  echo "       model: ${adaptive_model}" >&2
  echo "       cfg:   ${adaptive_witness}" >&2
  exit 1
fi

tlc_workers="${TLC_WORKERS:-}"
if [[ -z "${tlc_workers}" ]]; then
  tlc_workers="$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 1)"
fi

# `xtask machine-verify` gives every TLC child a deep JVM stack through
# `merged_java_tool_options()`. The bounded adaptive witness below launches
# `tlc` directly, so apply that same policy here; otherwise GitHub's Java 21
# default stack overflows while evaluating the generated initial predicate.
tlc_java_tool_options="${JAVA_TOOL_OPTIONS:-}"
if [[ " ${tlc_java_tool_options} " != *" -Xss"* ]]; then
  tlc_java_tool_options="-Xss256m${tlc_java_tool_options:+ ${tlc_java_tool_options}}"
fi
if [[ " ${tlc_java_tool_options} " != *" -XX:+UseParallelGC "* ]]; then
  tlc_java_tool_options="-XX:+UseParallelGC${tlc_java_tool_options:+ ${tlc_java_tool_options}}"
fi
export JAVA_TOOL_OPTIONS="${tlc_java_tool_options}"

# JAVA_TOOL_OPTIONS reaches the JVM, not the launcher. The `java` binary sizes
# the main thread, where TLC parses the module and computes the initial
# states, only from its command line or JDK_JAVA_OPTIONS; `tlc` is a
# `java -jar` wrapper, so the stack flag has to travel through
# JDK_JAVA_OPTIONS as well. Without it the generated initial predicate
# overflows the launcher's default stack and TLC reports a StackOverflowError
# regardless of the -Xss above (observed on the 09-16 Governance lane and
# reproduced locally). An explicit caller stack policy governs both layers.
tlc_stack_flag="-Xss256m"
for flag in ${tlc_java_tool_options}; do
  if [[ "${flag}" == -Xss* ]]; then
    tlc_stack_flag="${flag}"
    break
  fi
done
tlc_jdk_java_options="${JDK_JAVA_OPTIONS:-}"
if [[ " ${tlc_jdk_java_options} " != *" -Xss"* ]]; then
  tlc_jdk_java_options="${tlc_stack_flag}${tlc_jdk_java_options:+ ${tlc_jdk_java_options}}"
fi
export JDK_JAVA_OPTIONS="${tlc_jdk_java_options}"

# The full adaptive composition includes two complete MobMachine instances and
# is too large for the CI TLC budget. Before applying that broad skip, prove the
# generated route that matters for the adaptive bundle: terminal layer-mob
# classification is emitted, delivered, and observed through the canonical
# `layer_terminal_reaches_adaptive_kernel` route.
#
# The witness runs through xtask rather than a bare `tlc` call: TLC exit 0 alone
# does not prove the scripted route ran (the witness state constraint can
# truncate the search, and every witness invariant is vacuous until the script
# completes). `machine-verify-witness` runs it with coverage and fails unless the
# witness's completion action fired.
echo "running bounded adaptive_mob_bundle layer_terminal_feedback TLC witness"
"${xtask_bin}" machine-verify-witness \
  --composition adaptive_mob_bundle \
  --witness layer_terminal_feedback \
  --workers "${tlc_workers}"

# The canonical meerkat_machine ci.cfg sweep is structural (it stops after one
# step), so durable in-turn Steer delivery is model-checked by a hand-written
# bounded audit over the SAME generated model: admission, the durable join,
# every terminal resolution, input lifecycle actions and every run-ending arm
# of one runtime-loop run, under every generated invariant plus the audit's
# exactly-once invariants. Its TLC config is derived from the generated ci.cfg
# on each run, so it cannot drift from the model. 16 steps reach every
# resolution followed by every run-ending arm; deeper bounds run by hand.
durable_steer_audit="${workspace_root}/specs/machines/meerkat_machine/durable_in_turn_steer_audit.sh"
if [[ ! -x "${durable_steer_audit}" ]]; then
  echo "error: durable in-turn steer audit runner is missing from workspace runfiles: ${durable_steer_audit}" >&2
  exit 1
fi

# The live-context outbox invariants (no closed channel leaves a queued row;
# no queued row is one a channel's provider session already carries) are
# model-checked by a second bounded audit over the same generated model: one
# session's outbox across two channels through enqueue, staging and seed
# advance, bind, append authorization and resolution, close, abandoned
# admission and ambiguity recovery, under every generated invariant. It then
# requires TLC to reach each goal (a close ending a leftover, a recovery
# authorization ending the rows its seed carries, a row queued after the
# authorization reaching the replacement), so the invariants are not vacuous.
# 20 steps reach the deepest goal; deeper bounds run by hand.
live_context_outbox_audit="${workspace_root}/specs/machines/meerkat_machine/live_context_outbox_audit.sh"
if [[ ! -x "${live_context_outbox_audit}" ]]; then
  echo "error: live-context outbox audit runner is missing from workspace runfiles: ${live_context_outbox_audit}" >&2
  exit 1
fi

# Broad composition full-TLC skips are CI-time/memory-budget exceptions, NOT
# codegen defects. `machine-verify` still validates drift and the generated
# ci.cfg structural-invariant contract before honoring these skips. The earlier
# "Unknown operator" gap (mob
# coordination temporal predicates referenced but not emitted) was RESOLVED by
# the LUC-524 machine-authority work: the generated composition model
# (specs/compositions/meerkat_mob_seam/model.tla) now parses cleanly under SANY
# (zero unknown-operator/semantic errors). What remains is scale — the emitted
# model is very large and a full ci.cfg state-space sweep does not yet fit the
# CI budget. The per-machine specs still model-check, and `machine-check-drift`
# (below + on the default cargo CI lane) keeps the generated kernels honest.
# The skip covers only the full ci.cfg sweep: each skipped composition's
# scripted witnesses still run with the completion proof (the meerkat_mob_seam
# runtime-binding, retire and destroy round trips, and adaptive's
# layer_terminal_feedback, which is also proven directly above).
#
# `adaptive_mob_bundle` has a canonical route for layer-terminal feedback,
# generated driver, checked-in witness config, ci.cfg structural invariant, and
# the bounded witness TLC proof above. It still composes two full MobMachine
# instances, so the full composition TLC sweep exceeds the required CI budget.
#
# Scheduling: the two audits and `machine-verify` are independent TLC work.
# With a total budget of at least three 4-worker shares (TLC_WORKERS, or the
# core count), the audits run concurrently with `machine-verify`, which itself
# runs its TLC jobs concurrently within the remaining workers. Each concurrent
# audit JVM gets 4 workers, GC threads capped to match, and a heap share
# proportional to its workers; `machine-verify` gets the rest of both budgets,
# so the totals are never exceeded. Outputs are captured and printed in the
# fixed lane order below, and the lane fails if any part fails. A smaller
# budget runs everything sequentially, exactly as before.
audit_workers=4
run_machine_verify() {
  "${xtask_bin}" machine-verify --all --skip-cargo-tests \
    --skip-tlc-composition meerkat_mob_seam \
    --skip-tlc-composition adaptive_mob_bundle
}

total_heap_mb="${TLC_HEAP_BUDGET_MB:-}"
if [[ -z "${total_heap_mb}" ]]; then
  if [[ -r /proc/meminfo ]]; then
    total_heap_mb="$(awk '/^MemTotal:/ {print int($2 / 1024 / 2)}' /proc/meminfo)"
  else
    total_heap_mb="$(( $(sysctl -n hw.memsize 2>/dev/null || echo 0) / 1024 / 1024 / 2 ))"
  fi
fi
# Each concurrent TLC JVM needs a heap floor (the largest generated models do);
# without three floors of heap budget, run sequentially.
min_heap_mb_per_job=16384
if (( tlc_workers < 3 * audit_workers )) \
  || (( total_heap_mb > 0 && total_heap_mb < 3 * min_heap_mb_per_job )); then
  echo "running bounded durable in-turn steer TLC audit"
  TLC_WORKERS="${tlc_workers}" "${durable_steer_audit}" "${DURABLE_STEER_AUDIT_MAX_STEPS:-16}"
  echo "running bounded live-context outbox TLC audit"
  TLC_WORKERS="${tlc_workers}" "${live_context_outbox_audit}" "${LIVE_CONTEXT_OUTBOX_AUDIT_MAX_STEPS:-20}"
  TLC_WORKERS="${tlc_workers}" run_machine_verify
  exit $?
fi

audit_java_tool_options="${JAVA_TOOL_OPTIONS}"
machine_verify_heap_mb=""
if [[ " ${audit_java_tool_options} " != *" -XX:ParallelGCThreads="* ]]; then
  audit_java_tool_options+=" -XX:ParallelGCThreads=${audit_workers} -XX:ConcGCThreads=1"
fi
if (( total_heap_mb > 0 )); then
  audit_heap_mb=$(( total_heap_mb * audit_workers / tlc_workers ))
  (( audit_heap_mb < min_heap_mb_per_job )) && audit_heap_mb=${min_heap_mb_per_job}
  if [[ " ${audit_java_tool_options} " != *" -Xmx"* ]]; then
    audit_java_tool_options+=" -Xmx${audit_heap_mb}m"
  fi
  machine_verify_heap_mb="$(( total_heap_mb - 2 * audit_heap_mb ))"
fi

lane_logs="$(mktemp -d "${TMPDIR:-/tmp}/machine-verify-lane.XXXXXX")"
trap 'rm -rf "${lane_logs}"' EXIT
echo "machine-verify lane: audits run concurrently (${audit_workers} workers each), machine-verify gets $(( tlc_workers - 2 * audit_workers )) workers"
TLC_WORKERS="${audit_workers}" JAVA_TOOL_OPTIONS="${audit_java_tool_options}" \
  "${durable_steer_audit}" "${DURABLE_STEER_AUDIT_MAX_STEPS:-16}" >"${lane_logs}/steer.log" 2>&1 &
steer_pid=$!
TLC_WORKERS="${audit_workers}" JAVA_TOOL_OPTIONS="${audit_java_tool_options}" \
  "${live_context_outbox_audit}" "${LIVE_CONTEXT_OUTBOX_AUDIT_MAX_STEPS:-20}" >"${lane_logs}/live.log" 2>&1 &
live_pid=$!
machine_verify_status=0
(
  export TLC_WORKERS="$(( tlc_workers - 2 * audit_workers ))"
  if [[ -n "${machine_verify_heap_mb}" ]]; then
    export TLC_HEAP_BUDGET_MB="${machine_verify_heap_mb}"
  fi
  run_machine_verify
) >"${lane_logs}/machine-verify.log" || machine_verify_status=$?
steer_status=0
wait "${steer_pid}" || steer_status=$?
live_status=0
wait "${live_pid}" || live_status=$?

echo "running bounded durable in-turn steer TLC audit"
cat "${lane_logs}/steer.log"
echo "running bounded live-context outbox TLC audit"
cat "${lane_logs}/live.log"
cat "${lane_logs}/machine-verify.log"

lane_status=0
for part in "durable in-turn steer audit:${steer_status}" \
  "live-context outbox audit:${live_status}" \
  "machine-verify:${machine_verify_status}"; do
  if [[ "${part##*:}" != "0" ]]; then
    echo "error: ${part%:*} failed (exit ${part##*:})" >&2
    lane_status=1
  fi
done
exit "${lane_status}"
