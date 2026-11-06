#!/usr/bin/env bash
set -euo pipefail

# usage: machine_verify_all_tlc_test.sh <xtask-bin> [--part <part>]
#        machine_verify_all_tlc_test.sh --part audits-a|...|audits-o|audits-q|...|audits-y
# The canonical TLC lane, split into parts so each fits a CI job limit:
#   machine-verify  the bounded adaptive witness plus `machine-verify --all`
#   audits-a        hand-written audit shard A
#   audits-b        hand-written audit shard B
#   audits-c        hand-written audit shard C
#   audits-d        hand-written audit shard D
#   audits-e        hand-written audit shard E
#   audits-f        hand-written audit shard F
#   audits-g        hand-written audit shard G
#   audits-h        hand-written audit shard H
#   audits-i        hand-written audit shard I
#   audits-j        hand-written audit shard J
#   audits-k        hand-written audit shard K
#   audits-l        hand-written audit shard L
#   audits-m        hand-written audit shard M
#   audits-n        hand-written audit shard N
#   audits-o        hand-written audit shard O
#   audits-q        hand-written audit shard Q
#   audits-r        hand-written audit shard R
#   audits-s        hand-written audit shard S
#   audits-t        hand-written audit shard T
#   audits-u        hand-written audit shard U
#   audits-v        hand-written audit shard V
#   audits-w        hand-written audit shard W
#   audits-x        hand-written audit shard X
#   audits-y        hand-written audit shard Y
#   all             every part (the default: `make machine-verify`, the
#                   nightly bounded TLC job and the release gate)
# Bazel runs the twenty-five parts as separate targets
# (machine_verify_all_tlc_test, machine_verify_audits_a_tlc_test,
# machine_verify_audits_b_tlc_test, machine_verify_audits_c_tlc_test,
# machine_verify_audits_d_tlc_test, machine_verify_audits_e_tlc_test,
# machine_verify_audits_f_tlc_test, machine_verify_audits_g_tlc_test,
# machine_verify_audits_h_tlc_test, machine_verify_audits_i_tlc_test,
# machine_verify_audits_j_tlc_test, machine_verify_audits_k_tlc_test,
# machine_verify_audits_l_tlc_test, machine_verify_audits_m_tlc_test,
# machine_verify_audits_n_tlc_test, machine_verify_audits_o_tlc_test,
# machine_verify_audits_q_tlc_test, machine_verify_audits_r_tlc_test,
# machine_verify_audits_s_tlc_test, machine_verify_audits_t_tlc_test,
# machine_verify_audits_u_tlc_test, machine_verify_audits_v_tlc_test,
# machine_verify_audits_w_tlc_test, machine_verify_audits_x_tlc_test,
# machine_verify_audits_y_tlc_test) so the
# BuildBuddy machine-authority lane runs them in parallel; together they are
# exactly `all`. CI's tlc-audits job runs the twenty-four audit shards, one
# job each.
# The audit shards never execute xtask, so they alone may omit its path.
audit_shard() {
  case "$1" in
    audits-a|audits-b|audits-c|audits-d|audits-e|audits-f|audits-g|audits-h|audits-i|audits-j|audits-k|audits-l|audits-m|audits-n|audits-o|audits-q|audits-r|audits-s|audits-t|audits-u|audits-v|audits-w|audits-x|audits-y) return 0 ;;
    *) return 1 ;;
  esac
}
lane_part="all"
if [[ "${1:-}" == "--part" ]] && audit_shard "${2:-}"; then
  xtask_bin=""
  lane_part="$2"
else
  xtask_bin="${1:?xtask binary path is required (only --part audits-a..audits-o or audits-q..audits-y may omit it)}"
  if [[ "${2:-}" == "--part" ]]; then
    lane_part="${3:?--part needs machine-verify, an audit shard (audits-a..audits-o, audits-q..audits-y) or all}"
  fi
fi
if [[ "${lane_part}" != "machine-verify" && "${lane_part}" != "all" ]] && ! audit_shard "${lane_part}"; then
  echo "error: unknown lane part '${lane_part}' (machine-verify, audits-a..audits-o, audits-q..audits-y, all)" >&2
  exit 2
fi
# run_part <part>: whether this invocation runs <part>.
run_part() {
  [[ "${lane_part}" == "all" || "${lane_part}" == "$1" ]]
}
if [[ -n "${xtask_bin}" && "${xtask_bin}" != /* ]]; then
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
    if [[ -z "${xtask_bin}" ]]; then
      echo "error: tlc is absent and drift-only needs the xtask binary; pass it before --part" >&2
      exit 1
    fi
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
# resolution followed by every run-ending arm; deeper bounds run by hand. It
# then requires TLC to reach a durable join of the late Steer input once per
# content input kind (five short runs), so the invariants are not vacuous for
# any of them, and substitutes a small run contributor bound for the generated
# 256 (three short runs), so the join the contributor guard refuses is explored.
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

# The controller hold (a kept governed input that waits, held by the machine,
# for its host to supply its controller client again) and the destination a
# resumed run waits for and runs under are model-checked by a hand-written
# audit over the same generated model: one session recovers a queued input
# and an input staged into the run its registration marked resumable, both
# governed, the bound one with its destination, and explores in Idle,
# Attached and Running every hold and release, the start of the marked run
# and of a new one, the resume, settlement and release of the marked run,
# staging, rollback, apply, the abandon of either input as a refused resume,
# a document that comes to require a destination and the bind of that
# destination, under every generated invariant
# (input_controller_holds_are_kept_unstaged_governed_inputs among them) plus
# five action properties (a held input is never staged or applied and keeps
# its attempts; the marked run never resumes while its input is held; a hold
# or release changes only the holds, by exactly its input; the marked run
# never resumes while its document requires a destination never bound; a
# resumed run is the staged run under its inputs' destination and batch). It
# requires nine witnesses reached and refuses nine seeded defects
# (--mutants): a stage, an apply and a resume that ignore the hold, a
# settlement and an abandon that keep it, a hold that ignores the input's
# phase, a release that clears every hold, a resume that ignores the
# destination and a resume that keeps the previous staged run and context.
# 14 steps reach every witness. It runs in shard audits-h, with the
# instruction boundary audit and the superseded-ack retention audit below.
input_controller_hold_audit="${workspace_root}/specs/machines/meerkat_machine/input_controller_hold_audit.sh"
if [[ ! -x "${input_controller_hold_audit}" ]]; then
  echo "error: input controller hold audit runner is missing from workspace runfiles: ${input_controller_hold_audit}" >&2
  exit 1
fi
if run_part audits-h; then
  echo "running bounded input controller hold TLC audit"
  TLC_WORKERS="${tlc_workers}" "${input_controller_hold_audit}" "${INPUT_CONTROLLER_HOLD_AUDIT_MAX_STEPS:-14}" --mutants
fi

# The bounded retention of the ScheduleLifecycleMachine's superseded_ack_ids
# (the set holds only the acknowledgements of the latest supersession) is
# model-checked by a hand-written audit over the generated schedule model:
# one schedule through revises and deletes from Active and Paused, the
# repeated delete, pause and resume, and acknowledgements of two occurrence
# ids for every revision the bound reaches and one past it, in every phase,
# under every generated invariant plus two history invariants (every recorded
# id was acknowledged at the current revision; every acknowledgement of the
# current revision is recorded). It requires three witnesses reached (a
# revise and a delete that drop an earlier acknowledgement while recording
# their own, a late acknowledgement of a dropped id accepted without effect)
# and refuses eight seeded defects (--mutants): each revise and delete arm
# keeping the old ids, the Active and Paused acknowledgement arms recording
# late acknowledgements, and the Paused and Deleted ones dropping current
# acknowledgements. 6 steps reach every witness and mutant with room to
# spare. It runs in shard audits-h.
superseded_ack_retention_audit="${workspace_root}/specs/machines/schedule_lifecycle/superseded_ack_retention_audit.sh"
if [[ ! -x "${superseded_ack_retention_audit}" ]]; then
  echo "error: superseded-ack retention audit runner is missing from workspace runfiles: ${superseded_ack_retention_audit}" >&2
  exit 1
fi
if run_part audits-h; then
  echo "running bounded superseded-ack retention TLC audit"
  TLC_WORKERS="${tlc_workers}" "${superseded_ack_retention_audit}" "${SUPERSEDED_ACK_RETENTION_AUDIT_MAX_STEPS:-6}" --mutants
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
# held then a refusal, the retired drain refused). It explores the MobStop
# and ToolsNotPublished reasons, and a second run registers, holds and
# releases every reason (a mob member's HostPreparation among them) with no
# input and no run, the reasons being alike in every other arm. 12 steps
# reach every witness; deeper bounds run by hand.
run_start_hold_audit="${workspace_root}/specs/machines/meerkat_machine/run_start_hold_audit.sh"
if [[ ! -x "${run_start_hold_audit}" ]]; then
  echo "error: run-start hold audit runner is missing from workspace runfiles: ${run_start_hold_audit}" >&2
  exit 1
fi
if run_part audits-q; then
  echo "running bounded run-start hold TLC audit"
  TLC_WORKERS="${tlc_workers}" "${run_start_hold_audit}" "${RUN_START_HOLD_AUDIT_MAX_STEPS:-12}"
fi

# Keeping queued inputs across a graceful stop: an unregister records the
# disposition it began with (Stop, or Requeue with a max age), and every
# runtime stop asks the generated ClassifyStopInputDisposition which
# non-terminal inputs it keeps. A hand-written audit over the same generated
# model explores two processes. One session stops with either disposition or
# none, its in-flight run cancelled or still running, with every kind,
# durability, directedness and objective binding of the queued input, a
# replayed stop transaction and the unregister drain. A new process (every
# variable at the generated Init) then recovers the durable rows through the
# generated recovery arms, settles each kept row's expiry before the serving
# gate and runs. Under every generated invariant it checks that a kept input
# never runs before the restart, never gets a stop's runless carrier while
# every input a stop ends gets one, runs at most once across the restart,
# never runs once expired or before its settlement, and is kept only under
# Requeue and only when eligible (never objective-bound), that exactly one
# verdict arm answers each input, and that the requeued wait-failure arm has
# no session guard and changes no state. A store fence gap suspends the
# in-flight run of the first process instead, outside any stop, with an
# executor attached or none: the audit checks that the run's turn returns to
# Ready, that its input stays staged and bound with its stage attempts, that
# no fence-gap suspension comes while the registration drains, that a cancel
# (the run's own or an ambient one) or stop the run accepted before the gap
# stands as its stop intent, as an ambient cancel of the suspended run does,
# that a run whose stop was asked never resumes and is settled only as
# cancelled, that a requeue stop keeps nothing of a run whose stop or
# ambient cancel was asked, and that no direct run (a conversation run or an
# immediate append) starts while the suspended run is marked resumable (each
# starts once it is settled), while the same process resumes any other
# suspended run and starts its turn again. It requires fifteen witnesses
# reached, a graceful end and a crash among them; its --mutants mode (by
# hand) refuses seventeen seeded defects against every invariant but the
# two stop-intent ones and twelve stop-intent, fence-gap and direct-start
# defects each against its target invariant alone. 32 steps reach the
# deepest witness.
requeue_across_stop_audit="${workspace_root}/specs/machines/meerkat_machine/requeue_across_stop_audit.sh"
if [[ ! -x "${requeue_across_stop_audit}" ]]; then
  echo "error: requeue-across-stop audit runner is missing from workspace runfiles: ${requeue_across_stop_audit}" >&2
  exit 1
fi
if run_part audits-f; then
  echo "running bounded requeue-across-stop TLC audit"
  TLC_WORKERS="${tlc_workers}" "${requeue_across_stop_audit}" "${REQUEUE_ACROSS_STOP_AUDIT_MAX_STEPS:-36}"
fi

# The Keep disposition: a materialization refused before it served (a durable
# tail held for recovery, a failed agent build) rolls its registration back
# with an unregister that keeps every input the registration recovered from
# the store, exactly as the store holds it, except the inputs bound to a run
# marked resumable whose stop was asked, and still stops an input admitted
# while the registration stood; so does the retirement of an executor
# attachment that never served, while a retire that meets one still stops;
# once the registration served live work (an admission, a deduplicated wait
# or a recycle) its unregister keeps nothing. A hand-written audit over the
# same generated model runs three processes: the first crashes with one
# input staged and one queued under a key, the second recovers both through
# the generated recovery arms, either marking the first's run resumable
# first (its staged input stays bound to it, and a stop of the run may be
# recorded) or not, then, unmarked, may admit a third, resolve a
# resubmission as a duplicate or take a recycle, and rolls back with Keep
# (or, as the control, Stop), or attaches an executor that never serves and
# unregisters it through the unserved-attachment arm with its abort's Keep
# or a retire's Stop, and the third recovers and serves. It checks that
# exactly one verdict arm answers each input, that the input bound to a
# stopped marked run is never kept, that Keep stops no other recovered input
# and keeps no admitted one, that only Keep keeps, that a kept input
# survives the rollback, that live work forfeits the keep and that a retire
# over the unserved attachment keeps nothing, and requires eleven witnesses
# reached; its --mutants mode refuses fourteen seeded defects, the machine
# without the Keep arm, an unregister begin that keeps after live work, the
# machine without the exception for a stopped marked run and a retire that
# requests Keep among them. 44 steps reach the deepest witness, and 50 the
# whole reachable space.
materialization_rollback_keep_audit="${workspace_root}/specs/machines/meerkat_machine/materialization_rollback_keep_audit.sh"
if [[ ! -x "${materialization_rollback_keep_audit}" ]]; then
  echo "error: materialization-rollback keep audit runner is missing from workspace runfiles: ${materialization_rollback_keep_audit}" >&2
  exit 1
fi
if run_part audits-c; then
  echo "running bounded materialization-rollback keep TLC audit"
  TLC_WORKERS="${tlc_workers}" "${materialization_rollback_keep_audit}" "${MATERIALIZATION_ROLLBACK_KEEP_AUDIT_MAX_STEPS:-50}" --mutants
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
if run_part audits-d; then
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
if run_part audits-f; then
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
if run_part audits-c; then
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
if run_part audits-f; then
  echo "running bounded live-context causal-tail batch TLC audit"
  TLC_WORKERS="${tlc_workers}" "${live_causal_tail_batch_audit}" "${LIVE_CONTEXT_CAUSAL_TAIL_BATCH_AUDIT_MAX_STEPS:-24}" --mutants
fi

# The peer request identity maps (the addressee each outbound request was
# sent to; the requester each inbound request came from) are empty in the ci
# sweep, whose PeerCorrelationIdValues and PeerIdValues are {}, so their
# guards and invariants hold there only vacuously. A hand-written audit over
# the same generated model gives one registered session one correlation id
# and two peers and explores every arm that records, advances, answers,
# rejects, times out or fails an outbound request, every arm that records or
# answers an inbound request, and both verdict signals, in Idle and Attached,
# under every generated invariant plus: exactly one verdict of each kind is
# enabled, a progress, terminal or rejection step is taken only for the
# recorded addressee, and each verdict matches the record. It requires six
# witnesses reached (answered, progress then answered, NotAddressee,
# UnknownRequest after the answer, replied, NotRequester) and refuses five
# seeded defects (--mutants). 8 steps reach every witness.
peer_request_maps_audit="${workspace_root}/specs/machines/meerkat_machine/peer_request_maps_audit.sh"
if [[ ! -x "${peer_request_maps_audit}" ]]; then
  echo "error: peer request maps audit runner is missing from workspace runfiles: ${peer_request_maps_audit}" >&2
  exit 1
fi
if run_part audits-d; then
  echo "running bounded peer request maps TLC audit"
  TLC_WORKERS="${tlc_workers}" "${peer_request_maps_audit}" "${PEER_REQUEST_MAPS_AUDIT_MAX_STEPS:-8}" --mutants
fi

mob_operator_grant_audit="${workspace_root}/specs/machines/meerkat_machine/mob_operator_grant_audit.sh"
if [[ ! -x "${mob_operator_grant_audit}" ]]; then
  echo "error: mob operator grant audit runner is missing from workspace runfiles: ${mob_operator_grant_audit}" >&2
  exit 1
fi
if run_part audits-a; then
  echo "running bounded mob operator grant TLC audit"
  TLC_WORKERS="${tlc_workers}" "${mob_operator_grant_audit}" "${MOB_OPERATOR_GRANT_AUDIT_MAX_STEPS:-5}" --mutants
fi

# The empty-resume retirement verdict (a recovered registration whose resume
# found nothing to run retires only when it is Session-owned with nothing
# queued; a Composition-owned one, a mob member, keeps serving) reads the
# registration's residency owner, which no ci-sweep step sets. A hand-written
# audit over the same generated model drives one session and one runtime
# binding through the registration, the placement with either owner, the
# composition's claim and the verdict with and without queued input in Idle
# and Attached, under every generated invariant plus: exactly one verdict
# answers each observation, Retire fires only for a Session-owned
# registration with nothing queued, and a Composition placement claims the
# registration while a Session placement never downgrades it. It requires
# four witnesses reached (a Session-owned retire, a Composition-owned keep,
# a queued keep and the claim) and refuses two seeded defects (--mutants).
# 4 steps reach every witness.
empty_resume_residency_audit="${workspace_root}/specs/machines/meerkat_machine/empty_resume_residency_audit.sh"
if [[ ! -x "${empty_resume_residency_audit}" ]]; then
  echo "error: empty-resume residency audit runner is missing from workspace runfiles: ${empty_resume_residency_audit}" >&2
  exit 1
fi
if run_part audits-e; then
  echo "running bounded empty-resume residency TLC audit"
  TLC_WORKERS="${tlc_workers}" "${empty_resume_residency_audit}" "${EMPTY_RESUME_RESIDENCY_AUDIT_MAX_STEPS:-6}" --mutants
fi

# The settlement of an until-changed switch-turn (the shell releases the
# session mutation gate while the LLM reconfigure runs; on a failure
# AbortUntilChangedSwitchTurnReconfigure withdraws the request, and on a
# success CompleteUntilChangedSwitchTurnReconfigure completes it) is reached by
# no ci-sweep step that also admits an input or stops the runtime in that gap.
# A hand-written audit over the same generated model drives one session, one
# input and two requests through the accepted until-changed request, its
# refusal while another one is pending, the refusal of a finite request while
# a switch is pending, the input's admission, the stop and the input's
# abandonment, and the completion and the abort in every phase each declares
# (only their Idle and Stopped arms fire, since nothing attaches, runs or
# retires the session; all phases of an arm share one generated body), under
# every generated invariant plus one invariant and four action properties:
# the pending switch's id holds no denial; an abort withdraws exactly the
# pending switch and its request's approval entries (no terminal; the phase
# and every input's facts unchanged), and fires only for the request still
# pending; a pending switch ends only through its own request; a completion
# clears it, records it as applied and makes its target the baseline. It
# requires five witnesses reached (an approved request withdrawn while
# another input is queued, one withdrawn in Stopped after a stop abandoned
# that input, a request refused while another one is pending, a finite
# request refused while the other one is pending, one completed in Stopped)
# and refuses eight seeded defects (--mutants): an abort that records a
# terminal, one that erases the inputs' phases, one without its
# pending-request guard, one that keeps its request's approval entry, an
# accept without its pending-switch guard, a completion that keeps the old
# baseline, an accept without its denied-request guard, a finite refusal
# without its pending-request guard.
# 9 steps reach every witness.
switch_turn_withdrawal_audit="${workspace_root}/specs/machines/meerkat_machine/switch_turn_withdrawal_audit.sh"
if [[ ! -x "${switch_turn_withdrawal_audit}" ]]; then
  echo "error: switch-turn withdrawal audit runner is missing from workspace runfiles: ${switch_turn_withdrawal_audit}" >&2
  exit 1
fi
if run_part audits-t; then
  echo "running bounded switch-turn withdrawal TLC audit"
  TLC_WORKERS="${tlc_workers}" "${switch_turn_withdrawal_audit}" "${SWITCH_TURN_WITHDRAWAL_AUDIT_MAX_STEPS:-10}" --mutants
fi

# The member generation high-water (resume recovers each identity's highest
# generation from the whole log, every reset epoch included, then seals the
# window; minting reads it) is empty in the MobMachine ci sweep, whose
# AgentIdentityValues are {}, so next_generation_above_highwater holds there
# only vacuously. A hand-written audit over the same generated model gives
# one identity generations 0..2 and explores the high-water recovery, the
# seal, and the roster and member reset recoveries, which replay any
# ascending generation, under every generated invariant plus two action
# properties (the seal closes recovery; recovery never lowers). It requires
# its six witnesses reached and refuses four seeded defects (--mutants): a
# mint helper that ignores the high-water, a recovery without its seal
# guard, a recovery without its not-lowered guard, and the exact-successor
# replay rule, which must leave both replay witnesses unreachable. Its reset
# run adds the retirement of the recovered member and the mob reset
# (ResetToRunning folds each retained generation into the high-water and
# drops the retained history, as a restart after the reset does), checks
# that the machine retains a generation only for the current reset epoch and
# mints above every generation seated, requires its witness reached, and
# refuses two seeded defects, each by its target invariant alone: a reset
# that keeps the retained history, and one that drops it without the fold.
# 4 steps reach every witness.
generation_highwater_audit="${workspace_root}/specs/machines/mob_machine/generation_highwater_audit.sh"
if [[ ! -x "${generation_highwater_audit}" ]]; then
  echo "error: generation high-water audit runner is missing from workspace runfiles: ${generation_highwater_audit}" >&2
  exit 1
fi
if run_part audits-r; then
  echo "running bounded generation high-water TLC audit"
  TLC_WORKERS="${tlc_workers}" "${generation_highwater_audit}" "${GENERATION_HIGHWATER_AUDIT_MAX_STEPS:-4}" --mutants
fi

# The AuthMachine source-credential publication (a publisher that re-reads
# its credential value from a source asks ResolveSourceCredentialPublication
# with the value's fingerprint and fires Acquire naming it only on Publish)
# needs two values to tell a new value from the recorded one, which the auth
# ci sweep (one string, three steps) never has. A hand-written audit over the
# same generated model explores every AuthMachine action over two values,
# OAuth flow membership left out, under every generated invariant plus an
# action property (a refused or logged-out value is never published again
# while the lease records it) and two invariants (the classifier's verdict is
# Publish exactly when an Acquire arm naming the value is enabled; a
# MarkPublicationRefused arm withdraws the lease exactly when the refusal
# names the publication the lease carries in a live phase). It requires its
# seven witnesses reached (a refusal and a logout healed by another value, a
# run-out window renewing the same value, a refused and a logged-out value
# whose Acquire is disabled while another's is enabled, a newer publication a
# late refusal of the earlier one leaves valid, a refusal of the current
# publication withdrawing it). Its --mutants mode (by hand) refuses the
# Acquire guard removed, the renewal arms AcquireFromExpiring and
# AcquireFromExpired disabled, and the refusal's correlation guard removed.
# 3 steps reach every witness.
source_credential_publication_audit="${workspace_root}/specs/machines/auth/source_credential_publication_audit.sh"
if [[ ! -x "${source_credential_publication_audit}" ]]; then
  echo "error: source-credential publication audit runner is missing from workspace runfiles: ${source_credential_publication_audit}" >&2
  exit 1
fi
if run_part audits-s; then
  echo "running bounded AuthMachine source-credential publication TLC audit"
  TLC_WORKERS="${tlc_workers}" "${source_credential_publication_audit}" "${SOURCE_CREDENTIAL_PUBLICATION_AUDIT_MAX_STEPS:-5}"
fi

# Scoped tool grant expiry and run scope (the ToolGrantLifecycleMachine
# deadline and run guards, the deadline timer, the boundary that shows a gated
# name, the dispatch gate's fresh admission and the end it writes after a
# refusal or loses, revocation, reads that settle a due grant without loading
# the session, two runs and the runtime's signal of their ends, crash,
# restart, recovery of the interrupted run and one bounded clock rollback)
# spans the shell's use of the machine, which the machine's own ci sweep
# cannot see. A hand-written audit over the same generated model drives one
# grant with deadline 2 on a clock in 0..3, under every generated invariant
# plus seven invariants and one action property (no call handed on at or
# after the deadline or after the grant ended; a persisted end is latched
# across crash, restart and rollback; expiry is never early; revocation
# precedes the deadline; a visible name was shown before the deadline; a
# run-scoped grant hands on calls of its own run only; it never ends with its
# run while that run is current). It requires its twelve witnesses reached
# and refuses nine seeded defects (--mutants): an early expiry guard, an
# inclusive admission guard, an admission guard that ignores the run scope, a
# dispatch that reuses the boundary's verdict, a restart that revives an
# ended grant, a revoked grant handed on, a boundary that trusts the stored
# phase, a call of another run handed on, and a run end signalled while the
# run is current. 6 steps reach every witness.
tool_grant_expiry_audit="${workspace_root}/specs/machines/tool_grant_lifecycle/tool_grant_expiry_audit.sh"
if [[ ! -x "${tool_grant_expiry_audit}" ]]; then
  echo "error: tool grant expiry audit runner is missing from workspace runfiles: ${tool_grant_expiry_audit}" >&2
  exit 1
fi
if run_part audits-u; then
  echo "running bounded tool grant expiry TLC audit"
  TLC_WORKERS="${tlc_workers}" "${tool_grant_expiry_audit}" "${TOOL_GRANT_EXPIRY_AUDIT_MAX_STEPS:-6}" --mutants
fi

# The MobMachine member lifecycle (roster recovery and reset, session
# binding, progress observations, the local Retire arms, RetireMember, the
# session ingress detach, the runtime retirement, destroy and archive
# observations, the resume path's replay arms, stop and resume) is empty in
# the MobMachine ci sweep, whose AgentIdentityValues are {}, and deep.cfg,
# which has identities, does not finish within the per-run cap: the spawn arms'
# parameters give a state millions of successors. A hand-written audit over
# the same generated model gives one identity two runtime ids, two fence
# tokens, generations 0..1 and two sessions, and drives every one of those
# arms its domains enable without spawning, under every generated invariant
# plus action properties (a member's recorded observation time never
# decreases; a new incarnation keeps no predecessor's health or failure
# record; a member runtime destroyed removes only that runtime; the mob is
# Destroyed only through a destroy it admitted) and two
# invariants that check why the remaining member arms cannot fire. It
# explores the whole reachable space, with no step bound: its TLC VIEW
# leaves out the two counters the model never reads
# (model_step_count, topology_epoch), which it checks first. It runs with
# TLC coverage and requires every listed arm to fire and a floor of distinct
# states, requires its six witnesses reached (one recovers an archived
# identity's next incarnation, one replays a delayed final of an older
# generation), and refuses eight seeded defects (--mutants): a progress arm
# without its observation guard, that guard rendered as a plain `<=` between
# option records, which TLC cannot evaluate, a peer-only retirement-start
# replay that drops a releasing retirement's session while its ingress
# detach is pending, the stale-incarnation Retired replay with its old
# guard, which prunes the Retiring runtime of an identity held for a
# respawn, a roster recovery that keeps its predecessor's execution health,
# and a runtime destroy observation that ends the mob, one that leaves its
# runtime live, and one that resets the mob's run count (the audit also starts
# and finishes one mob-level run, so the count is not always zero).
# A second run of the same audit drops the progress observations, adds the
# mob's completion and destruction (DestroyMob, and the destroy admission,
# after which a member runtime destroyed still removes only that runtime),
# and drives every
# RetireAtScope verdict arm over every scope its domains name. It checks
# that exactly one verdict arm is enabled for every scope, and that a
# Current verdict names the identity's current runtime and fence and admits
# a Retire of that incarnation; it requires two more witnesses and refuses
# two more defects: the Current arm without its runtime and fence guards,
# and the StaleFenceToken arm overlapping NoCurrentIncarnation. It also
# fires the RetireAbsent arms of a completed and a destroyed mob, checks
# that in every phase a RetireAbsent arm is enabled exactly when the
# identity maps to no runtime, and refuses two more defects: a completed
# mob without that arm, and the arm without its identity_absent guard.
member_lifecycle_audit="${workspace_root}/specs/machines/mob_machine/member_lifecycle_audit.sh"
if [[ ! -x "${member_lifecycle_audit}" ]]; then
  echo "error: member lifecycle audit runner is missing from workspace runfiles: ${member_lifecycle_audit}" >&2
  exit 1
fi
if run_part audits-e; then
  echo "running member lifecycle TLC audit"
  TLC_WORKERS="${tlc_workers}" "${member_lifecycle_audit}" --mutants
fi

# MobMachine member residency (Dormant, Waking, Live) is empty in the ci
# sweep, whose AgentIdentityValues are {}, so the residency invariants hold
# there only vacuously. A hand-written audit over the same generated model
# checks eleven small runs over their whole reachable space, each under a cap
# of minted wake attempts and the member lifecycle audit's VIEW
# (model_step_count and topology_epoch left out, checked first): one identity
# through the build seed, the boot plan, the seal and the host carry-out; one
# identity through every WakeMember arm, every attempt-fenced resolution (the
# fresh create of an unborn seat's session among them) and its stale twin in
# Running and Stopped, the seat session replay, the runtime-ready observation,
# every release shape, the drift and terminal classifications, the peer notice
# classification, the release verdict and the mob's stop and resume; one
# identity woken, released, retired (routed or archive-only), rotated, seated
# again on its first seat's session and destroyed; one identity in the
# Completed and Destroyed phases; one identity through an explicit resume; two
# identities and one wiring edge; two identities from the spawn ladder's
# Opened phase through the dormant seat, the embodied spawn, the release and
# the rotation; two identities woken beside each other; one identity, an
# identity entry or not and host-pending or not, rotated onto its own session
# (a respawn) beside its retirement starts, archives, wake, scoped release
# verdict and the mob's destroy; one identity whose retirement start and
# rotation are replayed and finished by the rotation's own terminal; and two
# identities, wired and with an external peer, whose committed same-session
# rotation's rows are replayed. Under every generated invariant plus the
# audit's own properties (no recovery or seat arm overwrites a phase, an open
# attempt is never replaced and ends only Live, Dormant or without a phase,
# every Waking member has an enabled closing arm of every kind, a Lifecycle
# input never wakes), WakeMember totality, a local wake that owes an endpoint
# closes Live only with it (and one owing no endpoint cause always can), a
# boot plan that never leaves a member blocking the seal, the release verdict
# helper and its agreement with the Released arm, the attempt fence on the
# routed binding refusal, the fresh create only for an unborn seat's session,
# the release epoch fence, no runtime retire routed for a member with no
# registration, a settled retirement that can always archive (under an
# admitted destroy no rotation or host carry-out holds it), no rotation under
# an admitted destroy, a resume seat unborn exactly when no build created its
# session, a rotation's terminal that records its next incarnation exempt from
# the definition's wiring rules exactly when its own input says so, a seat
# that records its member exempt exactly when its own input says so, no
# exemption after the seal but a seat's, an explicit resume that can always
# join a wake in flight before its topology whatever readiness it settled,
# holds no member work in its readiness or topology stages and reserves the
# members it rebuilds only before its readiness, a same-session terminal that
# keeps the member's session, endpoint, entry and host preparation mark and
# never unseats it, a scoped release that answers Stale for any incarnation
# but the current one, a replayed rotation its own terminal can always
# finish, and a replay that keeps the edges a same-session terminal records
# again, with the ASSUME over the wake refusal helper. It runs with TLC
# coverage and requires every listed arm to fire and a floor of distinct
# states per run, and requires its forty-four witnesses reached. Its
# --mutants mode (run by hand, outside the shards' budget) refuses
# sixty-eight seeded defects, one per residency invariant and property (a
# same-session start or replay that records no started role, and a terminal
# that keeps it, among them). The lane runs it
# in two parts: its whole-space runs in shard audits-g (--part spaces) and
# its witness runs in shard audits-i (--part witnesses).
member_residency_audit="${workspace_root}/specs/machines/mob_machine/member_residency_audit.sh"
if [[ ! -x "${member_residency_audit}" ]]; then
  echo "error: member residency audit runner is missing from workspace runfiles: ${member_residency_audit}" >&2
  exit 1
fi
if run_part audits-g; then
  echo "running bounded member residency whole-space TLC audit"
  TLC_WORKERS="${tlc_workers}" "${member_residency_audit}" --part spaces
fi
if run_part audits-i; then
  echo "running bounded member residency witness TLC audit"
  TLC_WORKERS="${tlc_workers}" "${member_residency_audit}" --part witnesses
fi

# MobMachine identity entries (an identity the host declared as its own,
# as opposed to a worker member) are reached only with identities, which the
# ci profile has none of. A hand-written audit over the same generated model
# checks ten runs over their whole reachable space, in eight parts, each in
# a shard of its own. Its two entry runs drive a replayed member and a member
# seated by the spawn ladder with or without entry custody, the replay of
# entry rows, every legacy adoption arm over the shell's sixteen raw
# observations, the admission probe, the agent lane's member admission, the
# worker reconcile and ensure, Stop and Resume, under every generated
# invariant (the entry ones among them: every entry is seated or unseated
# with a typed cause, no worker reconcile spawns or retires an entry, entry
# custody belongs to an open ladder), three ENABLED-form invariants (exactly
# one probe, agent-lane and adoption arm answers each input), the probe's
# refusal of an entry's name and three action properties (an entry only by
# the declared apply, a custody commit, an adoption from qualifying legacy
# marks or a replayed row; an adoption records an entry only from qualifying
# marks; the retirement without an adoption answers only the member it
# names). A single run over all sixteen observations reached the 900 s
# per-run cap on a hosted runner (1,575,121 states generated, 544 s locally
# with 4 TLC workers), so the two runs split them by the roster's listing:
# the listed run (--part listed, shard audits-w) drives the eight of a
# listed identity, the unlisted run (--part unlisted, shard audits-x) the
# eight of an unlisted one, 953,041 states generated each. Each reaches the
# single run's whole space (12,960 distinct states) and the two check every
# state and step it checked: identity_entry_audit.tla says why, and the
# runner checks the generated model's shape that makes it so. Their
# witnesses and mutants run in the entry part (--part entry, shard
# audits-l), with the lifecycle run. The
# lifecycle run takes the member residency audit's retire run with its
# member made an entry (the replay of its row, or its reseat's ladder opened
# with entry custody) through the dormant seat, the live and the
# archive-only retirement to its archive terminal (which keeps the entry,
# unseated and owed its seat, with its archived session recorded) and the
# reseat on that session, the rotation, the legacy fresh-life respawn and its
# refusal of an entry (in Running and Stopped), the legacy respawn's
# topology-preserving archive terminals, and the mob's destroy (DestroyMob
# and DestroyFromAny), under every generated invariant (a destroyed mob holds
# no entry and no entry custody), the retire run's own properties and five
# action properties (a seat records an entry exactly when its ladder holds
# entry custody, a rotation keeps the entry, an entry only by a custody seat
# or a replayed row; the legacy fresh-life respawn admitted only for a member
# that is no entry, and its topology-preserving archive never taking an
# entry). All three run with TLC coverage and require every listed arm to
# fire and a floor of distinct states; the entry part reaches their
# twenty-five witnesses, and --mutants refuses fourteen seeded defects: a
# reconcile that retires an entry, a probe that answers an entry's name as a
# plain duplicate, an adoption without the observation, the replay of an
# entry row on an unseated identity (in the listed run), an unlisted
# adoption that takes one legacy mark, a retirement without an adoption that
# ignores the host's intent row (in the unlisted run), an archive that drops
# the kept entry's unseated cause, a destroy's member archive that keeps the
# entry, a DestroyMob that keeps the entries, a DestroyFromAny that keeps an
# open ladder's entry custody, a rotation that drops the entry, a seat that
# ignores its ladder's entry custody, a fresh-life respawn admitted for an
# entry and a topology-preserving archive that takes an entry. Its declared
# apply runs
# (identity_entry_apply_audit.tla) make the other four parts. --part apply
# (shard audits-k): a declared member seated by its custody ladder, retired
# unlisted or not, declared again under the role its archived session was
# built under or another (with the migration from it declared or not),
# revived on its archived session or seated fresh once a forget let its
# record go, under the seal, the unmark on listing, a candidate composition
# and the forced destroy. --part legacy (shard audits-m): a legacy member
# adopted, unlisted and retired to every copy of the plain end (live, after
# its runtime retired, stopped, or replayed with its session archived or
# not and its incarnation local or hosted elsewhere), the replay of its
# start row (stale or not), Stop, Resume and Complete, and the mob's reset
# from each phase it leaves (its preflight with a member seated, its commit
# on an empty roster); the witnesses and mutants of its unlisted end (four
# witnesses of the end on each copy and the five mutants that keep its mark
# there) run in --part legacy-end (shard audits-y). --part roles (shard
# audits-n): a seated entry
# declared under another role or mode, a first-boot preparation, every
# WakeMember cause and a same-session rotation under another role that
# names the member's key under the new role's comms name or no endpoint,
# its terminal seating the role it started with whatever its grant; and a
# role-held member's declared migration, its same-session and fresh-session
# rotations under another role or its own, and a first-boot Resume request.
# --part hosted (shard audits-o): held creation, the held spawn and entries
# hosted on their first use, the seal that leaves a member whose spawn is in
# flight unmarked, with Stop, Resume, Complete and the forced destroy (which
# ends the held spawn's ladder whether its membership committed or not), and
# the declared apply's open fact; one bound host and the placed spawn ladder,
# its remote commit, its retirement and the placed end; and an explicit
# resume over a member, Dormant or Live, whose host preparation is owed or
# not, probed while the mob is still stopped and again once it runs. They
# check every generated invariant (the declared apply's among them),
# exactly one arm answering every declaration, seal, mark replay and apply
# opening, and the declared apply's action properties
# (identity_entry_apply_audit.tla names them); they require every listed
# arm to fire, a floor of distinct states and their fifty-eight witnesses
# reached, and --mutants refuses sixty seeded defects: one per declared
# apply invariant and property, one per guard of the declared apply that a
# reachable state needs, two forced destroys, one that keeps a spawn ladder
# whose membership committed and one that keeps the startup binding request
# that commit recorded, and a seal that marks a member whose spawn is in
# flight, refused by its target invariant alone.
identity_entry_audit="${workspace_root}/specs/machines/mob_machine/identity_entry_audit.sh"
if [[ ! -x "${identity_entry_audit}" ]]; then
  echo "error: identity entry audit runner is missing from workspace runfiles: ${identity_entry_audit}" >&2
  exit 1
fi
if run_part audits-l; then
  echo "running bounded identity entry TLC audit"
  TLC_WORKERS="${tlc_workers}" "${identity_entry_audit}" --mutants --part entry
fi
if run_part audits-w; then
  echo "running bounded identity entry listed adoption TLC audit"
  TLC_WORKERS="${tlc_workers}" "${identity_entry_audit}" --mutants --part listed
fi
if run_part audits-x; then
  echo "running bounded identity entry unlisted adoption TLC audit"
  TLC_WORKERS="${tlc_workers}" "${identity_entry_audit}" --mutants --part unlisted
fi
if run_part audits-k; then
  echo "running bounded identity entry declared apply TLC audit"
  TLC_WORKERS="${tlc_workers}" "${identity_entry_audit}" --mutants --part apply
fi
if run_part audits-m; then
  echo "running bounded identity entry declared apply legacy TLC audit"
  TLC_WORKERS="${tlc_workers}" "${identity_entry_audit}" --mutants --part legacy
fi
if run_part audits-y; then
  echo "running bounded identity entry declared apply legacy end TLC audit"
  TLC_WORKERS="${tlc_workers}" "${identity_entry_audit}" --mutants --part legacy-end
fi
if run_part audits-n; then
  echo "running bounded identity entry declared apply roles TLC audit"
  TLC_WORKERS="${tlc_workers}" "${identity_entry_audit}" --mutants --part roles
fi
if run_part audits-o; then
  echo "running bounded identity entry declared apply hosted TLC audit"
  TLC_WORKERS="${tlc_workers}" "${identity_entry_audit}" --mutants --part hosted
fi

# The SessionDocumentMachine operator-instruction region: a hand-written
# audit drives the generated arms as the shell does (one session, two
# slots, revisions 1..2, a small and a budget-sized body, two mobs, every
# run start, all lowerings, folds at and after a run's first request,
# session writes, mob records, rebinds and releases) within the ci.cfg step
# bound, and checks the machine's own per-slot state: the slots in effect
# change only at a FreshRun start or a run-start fold and hold only Active
# slots, an application reaches the target with one entry per run start, an
# applied revoke holds, a mid-run fold keeps what is in effect, a session
# write never lowers a revision, a leading-prefix-only model applies only on
# a leading System prefix, a mob-owned target changes owner only by its own
# mob's release, a release empties the target and the next fresh run start
# revokes what the released mob had in effect, a rebind starts from the
# right target, the target stays within the active budget, a mob member's
# session takes no write or adoption of its own host, and a mob record takes
# over a session-owned target only on a member's session (an attach). It
# first checks that the generated model defines every arm it drives, and
# requires its six witnesses reached, one run each. --mutants refuses twelve
# seeded defects, each by the invariant it targets, run as the only
# invariant. No run uses -continue, which prints a trace for every violating
# state TLC checks: one run of the five witnesses hit the 900 s per-run cap
# in this shard. Not yet measured on a hosted runner; it joins this shard as
# the lightest one measured locally.
instruction_boundary_audit="${workspace_root}/specs/machines/session_document/instruction_boundary_audit.sh"
if [[ ! -x "${instruction_boundary_audit}" ]]; then
  echo "error: instruction boundary audit runner is missing from workspace runfiles: ${instruction_boundary_audit}" >&2
  exit 1
fi
if run_part audits-h; then
  echo "running bounded instruction boundary TLC audit"
  TLC_WORKERS="${tlc_workers}" "${instruction_boundary_audit}" --mutants
fi

# The MobMachine operator-instruction region (the mob's desired records, the
# owed computation and each member's delivery facts: owed, recorded,
# blocked) is empty in the MobMachine ci sweep, whose AgentIdentityValues
# are {}, so its member arms and member invariants hold there only
# vacuously. A hand-written audit over the same generated model
# (operator_instruction_delivery_audit.tla) checks two runs over their whole
# reachable space under a mob generation bound and a VIEW without the step
# and topology counters: the write run (two bound Dormant members under two
# profiles; every write, revoke, preparation, record and refusal arm its
# domains reach for a mob, a profile and a member target) and the seat run
# (two identities seated, bound, made unborn seats, born, retired by a
# releasing retire and unseated around writes, with the release verdict of
# their sessions). Beside every generated invariant it checks that a
# committed write owes the new generation to exactly the members the slot's
# old or new target selects, a seat owes the mob's generation, an unseat
# leaves no delivery fact, a delivery is prepared only for an owed, bound,
# born, local member, and no region step touches residency or wakes. It
# first checks that the generated model defines every arm it drives,
# requires each run's covered arms to fire and its eleven witnesses reached
# (among them a retiring member's release, after its retire unbound it), one
# run each, and with --mutants refuses four seeded defects, each by the
# property it targets run as the only property, and requires a live-only
# delivery and a bound-only release verdict to leave their witnesses
# unreachable. No run uses -continue. Not yet measured; it joins this shard
# as the lightest one measured locally.
operator_instruction_delivery_audit="${workspace_root}/specs/machines/mob_machine/operator_instruction_delivery_audit.sh"
if [[ ! -x "${operator_instruction_delivery_audit}" ]]; then
  echo "error: operator-instruction delivery audit runner is missing from workspace runfiles: ${operator_instruction_delivery_audit}" >&2
  exit 1
fi
if run_part audits-h; then
  echo "running bounded operator-instruction delivery TLC audit"
  TLC_WORKERS="${tlc_workers}" "${operator_instruction_delivery_audit}" --mutants
fi

# MobMachine created identity entries (an identity an agent created, as
# opposed to one the host declared) are recorded by the boot's scan of the
# identity store's created entry rows, which the ci profile cannot reach
# either. A hand-written audit over the same generated model
# (created_identity_audit.tla, shards audits-q, audits-r and audits-t
# through audits-v) checks
# fourteen runs over their whole reachable space, in two parts. Its
# recovery part's scan run
# replays a seated member with or without its Declared entry's row (before
# or after the scan of its rows: no replay turns a Created entry Declared;
# its own replay never after the scan of its pending row, the boot's order)
# beside an identity without a member, and scans a row of either with every
# presence and revival record, in a running and a stopped mob, until the
# boot plan's seal: a Present row of a seated local member records a Created
# entry, one of an identity without a member an entry owed its seat (with
# its revival record), a row pending its host's preparation an entry
# awaiting it, a
# removed row the name's tombstone, and an unreadable or contradicted row (a
# Declared entry of the name, a member where the row says none, a placed
# member, the name's tombstone, a spawn ladder open for it) reserves the
# name. The reseat run scans the rows of two identities without a member and
# opens, aborts and seats their spawn ladders with every pair of custodies:
# created custody seats a Created entry owed its seat again, on its revival
# record, and a ladder opened before its name was reserved seats no member
# and is aborted; then the worker ensure, the worker reconcile and the forced
# destroy. The names run scans the rows and then drives the declarations, the
# seal over every desired set, the legacy adoption, the agent lane and the
# admission probe in a running and a stopped mob; the placed run scans the
# row of a member
# seated on a member host, before or after its remote commit. They check
# every generated invariant (the created entries' among them: only a
# store-owned entry is archived, a removed entry's name is never an entry
# or a member again, created custody belongs to an open ladder of a Created
# entry and never beside entry custody, no worker reconcile spawns a
# reserved name or retires an unreadable one, a destroyed mob holds no
# entry, custody or reserved name),
# exactly one arm answering every row, declaration, seal, adoption,
# agent-lane admission and admission probe, and four action properties (no
# spawn ladder opens for a reserved name, and no ladder close seats a member
# under one; a ladder's custody matches its name's origin, and a seat under
# created custody binds the entry's revival record; the seal never touches a
# Created entry or an unreadable name's). Its changes part (0.9.9) drives a
# change's admission (in a run of its own over the only facts it reads: it
# changes nothing) and a store-owned entry's record (exactly one arm
# answering each, a new entry taking only a free name, its cause the
# declared apply's precedence over its facts, an entry held for its
# creation never seated Dormant), a store-owned update beside a roster's
# change of the same spec (the same verdict for the same facts, seated or
# not), the removal of either custody (every removal's end but the
# destroy's tombstoning its name, the destroy ending its mark), the archive
# and unarchive (an archived identity keeping its name and never seated, an
# identity awaiting its preparation never archived, the update of either
# changing its row only), the host's preparation and its held
# constructions (an entry awaiting its preparation seated only by its
# accepted activation, an accepted activation ended only by its
# publication, a `constructed` preparation never settled without an
# attempt, at most one attempt held or activating, a removal ending a held
# attempt) with their verdicts in a run of their own, the placed and
# peer-only spawns of a store-owned identity hosted on its first use, the
# one-entry unlist (exactly one arm answering it, deciding on its identity
# alone, a record taking no name an entry or the unlisted identity's
# archived session record holds) and the replay of a removal or an archive
# in flight carried to its terminal, its one row scanned before or after
# the journal replay (no replay seats a member under an archived entry, and
# the boot plan's seal keeps an archived identity's standing). They
# require every listed arm
# to fire, a floor of distinct states (the changes part's printed until its
# first green run sets them) and their witnesses reached, and --mutants
# refuses the seeded defects named in the runner: the created entries'
# invariants and properties and the new guards a reachable state needs (a
# removal's end that leaves no tombstone or keeps its mark, a destroy that
# keeps a mark, a removal that marks an unseated entry, a store-owned
# update that skips its declared migration or its RecordOnly or swaps two
# verdicts, an archive that frees its name or takes a pending entry, a
# roster-owned identity archived, an archived entry seated or replayed, a
# tombstoned name seated, a Declared row's replay over a Created entry, a
# record over an entry's name, a new entry's cause out of precedence, an
# unlist of a Created entry, a created custody kept local, which leaves the
# placed witness unreachable).
created_identity_audit="${workspace_root}/specs/machines/mob_machine/created_identity_audit.sh"
if [[ ! -x "${created_identity_audit}" ]]; then
  echo "error: created identity audit runner is missing from workspace runfiles: ${created_identity_audit}" >&2
  exit 1
fi
# Its runs are split over five shards (sized below): the recovery part's
# scan and placed runs in audits-q and its reseat and names runs in
# audits-v; the changes part's admit, commit and update runs in audits-r,
# its archive, unlist, placement and recovered runs in audits-t, and its
# removal, preparation and prepverdicts runs in audits-u.
if run_part audits-q; then
  echo "running bounded created identity scans TLC audit"
  TLC_WORKERS="${tlc_workers}" "${created_identity_audit}" --mutants --part scan,placed
fi
if run_part audits-v; then
  echo "running bounded created identity reseats and names TLC audit"
  TLC_WORKERS="${tlc_workers}" "${created_identity_audit}" --mutants --part reseat,names
fi
if run_part audits-r; then
  echo "running bounded created identity admissions and records TLC audit"
  TLC_WORKERS="${tlc_workers}" "${created_identity_audit}" --mutants --part admit,commit,update
fi
if run_part audits-u; then
  echo "running bounded created identity removals TLC audit"
  TLC_WORKERS="${tlc_workers}" "${created_identity_audit}" --mutants --part removal
fi
if run_part audits-t; then
  echo "running bounded created identity archives and unlists TLC audit"
  TLC_WORKERS="${tlc_workers}" "${created_identity_audit}" --mutants --part archive,unlist,placement,recovered
fi
if run_part audits-u; then
  echo "running bounded created identity preparations TLC audit"
  TLC_WORKERS="${tlc_workers}" "${created_identity_audit}" --mutants --part preparation,prepverdicts
fi

# The SessionDocumentMachine activation-body retirement region: a hand-written
# audit drives the generated recovery and decision arms as the shell does
# (one session, two keys, up to three rows per key, own and copied heads; a
# first recovery from any projection, torn ones included, an own activation
# as a recovery with one more row, a fork as a recovery of a prefix with
# every head copied, and the decision) within the ci.cfg step bound, and
# checks the machine's own per-key state: only the decision retires a body,
# it retires every superseded row, never lowers a recovered count and
# changes nothing else, and an own newest row keeps its body. It first
# checks that the generated model defines both arms it drives, and requires
# its three witnesses reached, each in a capped run of its own without
# -continue. --mutants refuses eight seeded defects, each run with the
# invariant it targets as its only invariant. Not yet measured; it joins the
# shard of the other session-document audit as a small one.
activation_body_retirement_audit="${workspace_root}/specs/machines/session_document/activation_body_retirement_audit.sh"
if [[ ! -x "${activation_body_retirement_audit}" ]]; then
  echo "error: activation body retirement audit runner is missing from workspace runfiles: ${activation_body_retirement_audit}" >&2
  exit 1
fi
if run_part audits-h; then
  echo "running bounded activation body retirement TLC audit"
  TLC_WORKERS="${tlc_workers}" "${activation_body_retirement_audit}" --mutants
fi

# The MobMachine arm-coverage gate: every arm of the generated model's Next
# relation is either in a hand-written audit runner's covered_actions array
# (each runner checks with TLC coverage that every listed arm fires) or named
# in specs/machines/mob_machine/arm_coverage_not_covered.txt with the reason
# no audit drives it. A stale, contradictory or reasonless entry fails it too.
# It reads model.tla and the runners only and runs no TLC, so it costs
# seconds in the shard of the member lifecycle audit it reads.
mob_arm_coverage_gate="${workspace_root}/specs/machines/mob_machine/arm_coverage_gate.sh"
if [[ ! -x "${mob_arm_coverage_gate}" ]]; then
  echo "error: MobMachine arm-coverage gate is missing from workspace runfiles: ${mob_arm_coverage_gate}" >&2
  exit 1
fi
if run_part audits-e; then
  echo "running MobMachine arm coverage gate"
  "${mob_arm_coverage_gate}"
fi

# The SessionDocumentMachine durable-tail classification (Completed,
# Repairable, Ambiguous, by the store's durable head relation, the tail's
# run-id count, terminal stop reason, dangling tool uses, orphan results and
# messages after the terminal) has three arms whose guards must partition the
# observations: the DSL declares state invariants only and the generated Next
# is a disjunction, so the ci sweep never checks it. A hand-written audit over
# the same generated model checks, with ENABLED at the initial state over all
# 1296 observation tuples, that exactly one arm answers each tuple, that a
# store-proven rewrite descendant classifies exactly like a strict one, and
# that it holds as Ambiguous without exactly one run id. It first checks on
# the generated model that the three arms read no state but phase, so the
# initial state stands for every state, and requires its witness reached.
# --mutants refuses fourteen seeded defects, each by the invariant it
# targets. It needs no step bound.
durable_tail_classification_audit="${workspace_root}/specs/machines/session_document/durable_tail_classification_audit.sh"
if [[ ! -x "${durable_tail_classification_audit}" ]]; then
  echo "error: durable-tail classification audit runner is missing from workspace runfiles: ${durable_tail_classification_audit}" >&2
  exit 1
fi
if run_part audits-e; then
  echo "running bounded durable-tail classification TLC audit"
  TLC_WORKERS="${tlc_workers}" "${durable_tail_classification_audit}" --mutants
fi

# The MeerkatMachine durable-tail recovery authorization
# (AuthorizeDurableTailRecovery*, Idle and Retired) over the drive's
# store-proven ownership (NotProven, ExclusiveHost, ExclusiveWriter): a
# hand-written audit over the same generated model checks, with ENABLED in
# each state of a short walk (Idle and Retired, with no terminal run, the
# candidate's run terminal, or another run terminal) over all 2160
# observation tuples, that exactly one arm answers each tuple, that an
# unproven drive gates on the persisted facts as before, that ownership never
# admits another run or an undecodable row, that a host decides as a restart
# does over the row read as quiescent, and that a writer decides as for a
# retired runtime and never continues a run. It first checks on the
# generated model that the arms read no state but phase, the current run and
# the terminal run, so the walk stands for every state without a current
# run, and requires its four witnesses reached. --mutants refuses eleven
# seeded defects, each by the invariant it targets.
durable_tail_recovery_ownership_audit="${workspace_root}/specs/machines/meerkat_machine/durable_tail_recovery_ownership_audit.sh"
if [[ ! -x "${durable_tail_recovery_ownership_audit}" ]]; then
  echo "error: durable-tail recovery ownership audit runner is missing from workspace runfiles: ${durable_tail_recovery_ownership_audit}" >&2
  exit 1
fi
if run_part audits-d; then
  echo "running bounded durable-tail recovery ownership TLC audit"
  TLC_WORKERS="${tlc_workers}" "${durable_tail_recovery_ownership_audit}" --mutants
fi

# The ApprovalLifecycleMachine entered state of a reviewed effect (a spent
# attempt stays Used while its admitting dispatch is live or a step admitted
# under it is in flight; AdmitEnteredStep and FinishEnteredStep never spend;
# the dispatch end releases the attempt, or closes its effect until the last
# admitted step finishes) needs two attempts, a step count and a dispatch
# end with steps in flight, which the ci sweep (one id, six steps) never
# reaches. A hand-written audit over the same generated model drives a
# model-reviewed and a consented attempt through their spends, step
# admissions and finishes requested at any time, and the dispatch end with
# zero, one or more steps in flight, under every generated invariant plus six
# history invariants and one action property. It requires every new arm to
# fire (TLC coverage), three scenario witnesses reached (the dispatch
# cancelled while a step is in flight, a step refused after the release, the
# helper graph admission and two children under one consent spend) and
# refuses ten seeded defects (--mutants). 16 steps reach both attempts
# entered with two steps each and both dispatches ending with steps in
# flight.
entered_step_audit="${workspace_root}/specs/machines/approval_lifecycle/entered_step_audit.sh"
if [[ ! -x "${entered_step_audit}" ]]; then
  echo "error: approval entered-step audit runner is missing from workspace runfiles: ${entered_step_audit}" >&2
  exit 1
fi
if run_part audits-c; then
  echo "running bounded approval entered-step TLC audit"
  TLC_WORKERS="${tlc_workers}" "${entered_step_audit}" "${ENTERED_STEP_AUDIT_MAX_STEPS:-16}" --mutants
fi

# The durable decision record (DurableDecisionMachine: one decision's sealed
# binding, its attested decision, the execution permit and the executed
# fact, expiry, withdrawal and the requester notice an ended decision owes)
# is model-checked by a hand-written audit over the generated model: every
# input in every phase, through one wrapper per arm, under every generated
# invariant plus twelve step properties (terminals latch; approvals and
# rejections attested; execution only from Approved with the held binding;
# expiry only at the deadline and, for an approved decision, only without
# an outstanding permit or by its release; withdrawal only while open by the
# requester or the host; the sealed binding never changes; the notice owed
# once at the end; no permit at or after the deadline; a release clears the
# permit; a human decision decided only when observed at or after its first
# presentation, and refused for the order only before it) and two state
# invariants (an owner's commit stays settleable; the first presentation
# time is the earliest). It requires every wrapper to fire, fifteen
# witnesses reached and refuses twenty-four seeded defects (--mutants). 6
# steps reach every witness. It runs alone in shard audits-j.
durable_decision_audit="${workspace_root}/specs/machines/durable_decision/durable_decision_audit.sh"
if [[ ! -x "${durable_decision_audit}" ]]; then
  echo "error: durable decision audit runner is missing from workspace runfiles: ${durable_decision_audit}" >&2
  exit 1
fi
if run_part audits-j; then
  echo "running bounded durable decision TLC audit"
  TLC_WORKERS="${tlc_workers}" "${durable_decision_audit}" "${DURABLE_DECISION_AUDIT_MAX_STEPS:-6}" --mutants
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

# Shard assignment balances measured audit cost on hosted 4-vCPU CI runners
# (4 TLC workers, default bounds), where each TLC run's JVM start and model
# parse is most of a small audit. Hosted seconds per audit are the lowest and
# highest of the 0.9.9 release runs (pull requests 41473326233 and
# 41581056376, main pushes 41594713659, 41605379855 and 41608949055,
# 2026-11-02 and 2026-11-03), and each shard's job time is its range on the
# three pushes, 7-26 s of it checkout and TLC setup:
# audits-a = live unregister cleanup 238-341, mob operator grant with
# --mutants 158-240: 414-602 s;
# audits-b = durable in-turn steer with its per-kind goal runs 348-610, live
# delegation steer 34-51: 514-685 s;
# audits-c = materialization-rollback keep with --mutants 267-309, live
# worker start after close 25-29, approval entered-step with --mutants
# 20-24: 656-663 s with the run-start hold audit (242-293), which moved to
# audits-q, so about 370-375 s;
# audits-d = durable-tail recovery ownership with --mutants 143-238,
# live-context outbox 100-146, peer request maps with --mutants 87-134,
# live-context result barrier with --mutants 25-39: 383-555 s;
# audits-e = member lifecycle with --mutants 441-522, empty-resume residency
# with --mutants 52-64, durable-tail classification with --mutants 36-48, the
# MobMachine arm coverage gate under 1: 558-646 s;
# audits-f = requeue-across-stop 222-303, live media health 125-176,
# live-context causal-tail batch with --mutants 86-125: 618-742 s with the
# generation high-water audit with --mutants (102-126), which moved to
# audits-r, so about 516-616 s. On the 0.9.9 release branch the shard also
# ran the AuthMachine source-credential publication audit and the tool grant
# expiry audit, and a pull request's run passed its 20-minute job timeout
# (20m18s on 41581056376); those two moved to audits-s and audits-u;
# audits-g = member residency's whole-space runs (--part spaces), alone: the
# audit 473-627, the shard 605-630 s;
# audits-h = input controller hold with --mutants 126-192, the MobMachine
# operator-instruction delivery audit with --mutants 82-134, the
# SessionDocumentMachine instruction boundary audit with --mutants 41-70 and
# activation-body retirement audit with --mutants 15-23, and the
# ScheduleLifecycleMachine superseded-ack retention audit with --mutants
# 7-11: 389-445 s;
# audits-i = member residency's witness runs (--part witnesses), alone: the
# audit 346-542, the shard 357-565 s;
# audits-j = durable decision with --mutants, alone: the audit 182-372, the
# shard 206-398 s;
# audits-k, audits-m, audits-n and audits-o = the identity entry audit's
# declared apply parts with --mutants, each alone: apply 384-460 (the shard
# 396-484 s), legacy 596-726 (605-751 s), roles 563-740 (604-745 s, and
# 776 s on a pull request's runner) and hosted 353-623 (366-640 s). The
# legacy part then took 1171 s (the shard 1182 s) on a pull request's run
# 42057238243: its whole space 260, 13 witnesses 401 and 16 mutants 510.
# The 4 witnesses and 5 mutants of its unlisted end, 562 s of it, moved to
# audits-y, so about 620 s for audits-m (256 s locally with 4 TLC
# workers), from those runs' times there;
# audits-y = the legacy run's unlisted end (--part legacy-end), alone:
# about 575 s from those runs' times there (227 s locally; this run
# class's hosted factor was 1.7-2.7x on that runner);
# audits-l = the identity entry audit's entry part with --mutants (--part
# entry), alone: the audit 404-715, the shard 588-741 s while the part also
# held the single entry run (101 s of main's 0.9.10 release run
# 41976604006); now the lifecycle run with its 14 witnesses and 8 mutants
# (203 s on that run, 157 s locally with 4 TLC workers) and the two entry
# runs' 11 witnesses and 6 mutants (148 s locally): not yet measured on a
# hosted runner, about 545 s with the entry runs' at 2.3x their local time
# (700 s with every run at 2.3x);
# audits-w = the identity entry audit's listed run (--part listed), alone:
# not yet measured on a hosted runner, 277 s locally, about 640 s at 2.3x;
# audits-x = its unlisted run (--part unlisted), alone: 270 s locally,
# about 620 s at 2.3x. The single entry run they split took 544 s locally
# and reached the 900 s per-run cap on a pull request's run 42057238243, a
# hosted factor above 1.65x for that run class (1.29x on main), hence the
# 2.3x;
# audits-q = the created identity audit's scan and placed runs 170-272 (the
# shard 183-277 s) and the run-start hold audit, from audits-c: about
# 471-568 s;
# audits-r = its admit, commit and update runs 135-231 (the shard
# 146-249 s) and the generation high-water audit, from audits-f: about
# 248-375 s;
# audits-s = the AuthMachine source-credential publication audit, 250-479
# hosted (168 s locally): 712-719 s with the created identity removal run
# (158-233), which moved to audits-u, so about 479-489 s;
# audits-t = its archive, unlist, placement and recovered runs: 228-335,
# the shard 320-364 s, and the switch-turn withdrawal audit with --mutants
# (eleven runs, not yet measured hosted; the empty-resume residency audit's
# seven runs take 52-64 s): about 400-460 s;
# audits-u = its preparation and prepverdicts runs 161-237 and the tool
# grant expiry audit with --mutants 13-16 (the shard 228-264 s), and the
# removal run, from audits-s: about 454-497 s;
# audits-v = its reseat and names runs: 500-665, the shard 525-690 s.
# The "about" figures are those runs' job times with the moved audit's own
# time added or taken away, not yet measured as one job. Hosted time ran
# about 1.2-3.8x the local times with 4 TLC workers recorded for these
# audits, so size a new audit by a hosted run. The job times out at 20
# minutes; each shard is sized to stay within about 720 s hosted, 60
# percent of that. audits-n, one audit part, reaches 745-776 s and gets
# shorter only when its part splits. A new audit
# joins the lightest shard, or a new shard when it would take that shard
# past about 720 s hosted. Shard of the two audits the
# `all` lane runs concurrently with machine-verify below:
durable_steer_shard="audits-b"
live_context_outbox_shard="audits-d"

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
