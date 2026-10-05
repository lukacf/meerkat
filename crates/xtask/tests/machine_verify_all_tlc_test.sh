#!/usr/bin/env bash
set -euo pipefail

xtask_bin="${1:?xtask binary path is required}"
# The canonical TLC lane, split into parts so each fits a CI job limit:
#   machine-verify  the bounded adaptive witness plus `machine-verify --all`
#   audits-a        hand-written audit shard A
#   audits-b        hand-written audit shard B
#   all             every part (the default: `make machine-verify`, the
#                   nightly bounded TLC job and the release gate)
# Bazel runs the three parts as separate targets (machine_verify_all_tlc_test,
# machine_verify_audits_a_tlc_test, machine_verify_audits_b_tlc_test) so the
# BuildBuddy machine-authority lane runs them in parallel; together they are
# exactly `all`.
lane_part="all"
if [[ "${2:-}" == "--part" ]]; then
  lane_part="${3:?--part needs machine-verify, audits-a, audits-b or all}"
fi
case "${lane_part}" in
  machine-verify|audits-a|audits-b|all) ;;
  *)
    echo "error: unknown lane part '${lane_part}' (machine-verify, audits-a, audits-b, all)" >&2
    exit 2
    ;;
esac
# run_part <part>: whether this invocation runs <part>.
run_part() {
  [[ "${lane_part}" == "all" || "${lane_part}" == "$1" ]]
}
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
if run_part machine-verify; then
  echo "running bounded adaptive_mob_bundle layer_terminal_feedback TLC witness"
  "${xtask_bin}" machine-verify-witness \
    --composition adaptive_mob_bundle \
    --witness layer_terminal_feedback \
    --workers "${tlc_workers}"
fi

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

# The live delegation steer edges (authorize, delivery outcome, reconcile at
# commit) sit behind a long live-channel setup the ci sweep cannot reach, so a
# second hand-written audit over the same generated model drives a bound
# channel to a started delegation worker and explores every steer outcome
# under every generated invariant (including
# live_delegation_steer_records_are_authorized_and_single). It also proves
# each outcome reachable by requiring a counterexample to its negation. 16
# steps reach every outcome on two continuations; deeper bounds run by hand.
live_steer_audit="${workspace_root}/specs/machines/meerkat_machine/live_delegation_steer_audit.sh"
if [[ ! -x "${live_steer_audit}" ]]; then
  echo "error: live delegation steer audit runner is missing from workspace runfiles: ${live_steer_audit}" >&2
  exit 1
fi
if run_part audits-b; then
  echo "running bounded live delegation steer TLC audit"
  TLC_WORKERS="${tlc_workers}" "${live_steer_audit}" "${LIVE_STEER_AUDIT_MAX_STEPS:-16}"
fi

# The run-start hold (#1500: a mob Stop holds member run starts so an input
# admitted before the stop runs only after Resume) is model-checked by a
# hand-written audit over the same generated model: one session, one queued
# input and two runs through every hold and release arm, every arm that
# establishes a new run and its Held twin, one turn, the run endings and the
# retired queue drain, under every generated invariant plus two action
# properties (no new run while held; hold and release never touch the queue
# or the current run). It also requires a counterexample to each witness
# negation (refused while held, released then runs, a run that finishes while
# held then a refusal, the retired drain refused). 12 steps reach every
# witness; deeper bounds run by hand.
run_start_hold_audit="${workspace_root}/specs/machines/meerkat_machine/run_start_hold_audit.sh"
if [[ ! -x "${run_start_hold_audit}" ]]; then
  echo "error: run-start hold audit runner is missing from workspace runfiles: ${run_start_hold_audit}" >&2
  exit 1
fi
if run_part audits-a; then
  echo "running bounded run-start hold TLC audit"
  TLC_WORKERS="${tlc_workers}" "${run_start_hold_audit}" "${RUN_START_HOLD_AUDIT_MAX_STEPS:-12}"
fi

# The live-context result barrier: a delegation result on a channel with a
# late bootstrap summary is released after the summary's provider
# acknowledgement and only after it, never held by queued context rows (S99).
# Readiness and the result-delivery authorization guard state the same rule;
# when they disagreed, the release task spun on refused authorizations. A
# hand-written audit over the same generated model drives one experimental
# channel's summary into delivery behind a queued row, with a released
# delegation result, and explores the summary's ACK and result authorization
# under every generated invariant plus "a result is authorized only after the
# summary ACK", and requires a result authorized while the row is still
# queued. It also refuses two seeded defects (--mutants): restoring the old
# tail-drain conjuncts in the authorization guard must make that goal
# unreachable, and dropping the summary conjunct must break the ACK rule.
# 21 steps reach the goal.
live_result_barrier_audit="${workspace_root}/specs/machines/meerkat_machine/live_context_result_barrier_audit.sh"
if [[ ! -x "${live_result_barrier_audit}" ]]; then
  echo "error: live-context result barrier audit runner is missing from workspace runfiles: ${live_result_barrier_audit}" >&2
  exit 1
fi
if run_part audits-a; then
  echo "running bounded live-context result barrier TLC audit"
  TLC_WORKERS="${tlc_workers}" "${live_result_barrier_audit}" "${LIVE_CONTEXT_RESULT_BARRIER_AUDIT_MAX_STEPS:-21}" --mutants
fi

# UnregisterSession against live channels (#1476): unregister is guarded on
# every live channel being closed and its close custody settled, and then
# clears the session's terminal context-preparation records. A third
# hand-written audit over the same generated model starts one channel of a
# registered session admitted, staged, bound, under a running run or with the
# session retired during that run, and explores its context preparation, its
# close transitions and the unregister drain in each phase under every
# generated invariant (including live_channel_state_requires_registered_session)
# plus the property that unregister never fires while a channel is bound. It
# requires unregister to be reachable from every start (no wedge) and to meet
# preparation records from the staged start. 16 steps reach every goal.
live_unregister_audit="${workspace_root}/specs/machines/meerkat_machine/live_unregister_cleanup_audit.sh"
if [[ ! -x "${live_unregister_audit}" ]]; then
  echo "error: live unregister cleanup audit runner is missing from workspace runfiles: ${live_unregister_audit}" >&2
  exit 1
fi
if run_part audits-a; then
  echo "running bounded live unregister cleanup TLC audit"
  TLC_WORKERS="${tlc_workers}" "${live_unregister_audit}" "${LIVE_UNREGISTER_AUDIT_MAX_STEPS:-16}"
fi

# The live media health edges (request at a channel's first output, the three
# judgements, the per-session reopen budget) guard an Active, exactly bound
# channel, which the ci sweep never reaches. A third hand-written audit over
# the same generated model binds one channel, explores requests, judgements,
# closed status, close and the second channel's open and bind on the same
# session, in Attached and (after a run starts) Running, under every
# generated invariant (including
# live_media_health_budget_and_verdicts_are_consistent) plus its own
# invariants and action properties (judged once, first output only). It
# proves every judgement reachable by requiring a counterexample to its
# negation (including a session earning its reopen again after its runtime
# stops and it resumes), and every new transition firing (each Never* action
# property must be violated). The re-earned reopen needs 20 steps; about 70 s.
live_media_health_audit="${workspace_root}/specs/machines/meerkat_machine/live_media_health_audit.sh"
if [[ ! -x "${live_media_health_audit}" ]]; then
  echo "error: live media health audit runner is missing from workspace runfiles: ${live_media_health_audit}" >&2
  exit 1
fi
if run_part audits-b; then
  echo "running bounded live media health TLC audit"
  TLC_WORKERS="${tlc_workers}" "${live_media_health_audit}" "${LIVE_MEDIA_HEALTH_AUDIT_MAX_STEPS:-20}"
fi

# Bounded audit of a durable worker start that resolves after its channel
# closed (Turbo S S104 R7): from an authorized worker start it explores the
# close and close settlement, start resolution through the operation's own
# channel and a foreign never-bound one, terminal recording and every
# revoked-worker reconciliation arm, under every generated invariant plus
# AuditForeignChannelNeverResolves. Both goals (resolved after the close;
# settled through the revoked-worker reconciliation) must be reached.
live_worker_start_after_close_audit="${workspace_root}/specs/machines/meerkat_machine/live_delegation_worker_start_after_close_audit.sh"
if [[ ! -x "${live_worker_start_after_close_audit}" ]]; then
  echo "error: live worker start after close audit runner is missing from workspace runfiles: ${live_worker_start_after_close_audit}" >&2
  exit 1
fi
if run_part audits-a; then
  echo "running bounded live worker start after close TLC audit"
  TLC_WORKERS="${tlc_workers}" "${live_worker_start_after_close_audit}" "${LIVE_WORKER_START_AFTER_CLOSE_AUDIT_MAX_STEPS:-22}"
fi

# The queued heard-speech replays behind a late summary are delivered as one
# causal-tail batch append over their exact cursor run (Turbo S S99). A
# hand-written audit over the same generated model explores the batch edge
# over every claimed tail set, the one-row edges, every resolve with every
# cursor pair, and the shell's re-enqueue of carried rows after a rejected
# batch; it requires a batch to cover only heard-speech replays and skip no
# queued row, every resolve to use the pending append's recorded cursors, no
# cursor to be delivered twice, and a batch authorized, delivered, rejected
# and re-delivered after rejection, and refuses four seeded defects
# (--mutants). 24 steps reach the deepest goal.
live_causal_tail_batch_audit="${workspace_root}/specs/machines/meerkat_machine/live_context_causal_tail_batch_audit.sh"
if [[ ! -x "${live_causal_tail_batch_audit}" ]]; then
  echo "error: live-context causal-tail batch audit runner is missing from workspace runfiles: ${live_causal_tail_batch_audit}" >&2
  exit 1
fi
if run_part audits-b; then
  echo "running bounded live-context causal-tail batch TLC audit"
  TLC_WORKERS="${tlc_workers}" "${live_causal_tail_batch_audit}" "${LIVE_CONTEXT_CAUSAL_TAIL_BATCH_AUDIT_MAX_STEPS:-24}" --mutants
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
# The live delegation steer and live media health audits above run on their
# own first (each is small: well under a minute at its default bound).
#
# Scheduling: the two audits and `machine-verify` are independent TLC work.
# With a total budget of at least four 4-worker shares (TLC_WORKERS, or the
# core count), the audits run concurrently with `machine-verify`, which itself
# runs its TLC jobs concurrently within the remaining workers. Below that,
# `machine-verify` would keep a single 4-worker share and its longest sweep
# would set the lane time (a 12-worker budget took 688 s that way, slower than
# an 8-worker budget running everything in sequence), so it runs sequentially. Each concurrent
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

# Shard assignment balances measured audit cost (4 TLC workers, default
# bounds): audits-a = live unregister cleanup (168 s), run-start hold (92 s),
# live worker start after close (14 s), live-context result barrier with
# --mutants (22 s);
# audits-b = durable in-turn steer (113 s), live media health (84 s),
# live-context outbox (54 s), live delegation steer (23 s), live-context
# causal-tail batch (with --mutants). A new audit joins
# the lighter shard. Shard of the two audits the `all` lane runs concurrently
# with machine-verify below:
durable_steer_shard="audits-b"
live_context_outbox_shard="audits-b"

# A single part runs on its own (Bazel gives each part its own executor):
# machine-verify takes every worker, and an audit shard runs its scheduled
# audits in sequence. Only `all` shares one budget below.
if [[ "${lane_part}" != "all" ]]; then
  if [[ "${lane_part}" == "machine-verify" ]]; then
    TLC_WORKERS="${tlc_workers}" run_machine_verify
    exit $?
  fi
  if [[ "${lane_part}" == "${durable_steer_shard}" ]]; then
    echo "running bounded durable in-turn steer TLC audit"
    TLC_WORKERS="${tlc_workers}" "${durable_steer_audit}" "${DURABLE_STEER_AUDIT_MAX_STEPS:-16}"
  fi
  if [[ "${lane_part}" == "${live_context_outbox_shard}" ]]; then
    echo "running bounded live-context outbox TLC audit"
    TLC_WORKERS="${tlc_workers}" "${live_context_outbox_audit}" "${LIVE_CONTEXT_OUTBOX_AUDIT_MAX_STEPS:-20}"
  fi
  echo "machine-verify lane part ${lane_part} passed"
  exit 0
fi

total_heap_mb="${TLC_HEAP_BUDGET_MB:-}"
if [[ -z "${total_heap_mb}" ]]; then
  if [[ -r /proc/meminfo ]]; then
    total_heap_mb="$(awk '/^MemTotal:/ {print int($2 / 1024 / 2)}' /proc/meminfo)"
  else
    total_heap_mb="$(( $(sysctl -n hw.memsize 2>/dev/null || echo 0) / 1024 / 1024 / 2 ))"
  fi
fi
# Each concurrent TLC JVM needs a heap floor (the audits extend the
# meerkat_machine model, which SANY needs 4 GiB to process at full speed;
# matches MIN_HEAP_MB_PER_PARALLEL_JOB in crates/xtask/src/machines.rs);
# without three floors of heap budget, run sequentially.
min_heap_mb_per_job=4096
if (( tlc_workers < 4 * audit_workers )) \
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
