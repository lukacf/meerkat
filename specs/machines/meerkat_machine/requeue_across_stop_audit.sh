#!/usr/bin/env bash
# Bounded TLC audit of keeping queued inputs across a graceful stop: the
# unregister's queued-input disposition, the generated per-input stop
# classification, and the restart's recovery and pre-serve expiry settlement.
#
# usage: requeue_across_stop_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's finite
# model values, its step bound and its invariants. The restart is a new
# process, so the audit needs every generated variable's initial value as an
# action: it derives requeue_across_stop_fresh.tla (AuditFreshMachine) from
# the generated Init on every run and fails when Init and the declared
# variables disagree. TLC runs in a scratch copy of the specs. It checks the
# whole bounded space once (safety), then reruns it once per reachability
# witness with that witness's negation as an extra invariant and requires
# TLC to report exactly that violation, so the invariants are not vacuous.
# With --mutants it then seeds seventeen defects into a copy of the generated
# model (or, for the serve-before-settle control, into the audit's settlement
# order) and requires TLC to refuse each with the invariant it breaks: a
# verdict that ignores the phase and lane, one that reads a fact the
# unregister changes, one that differs across the exit, a stop cleanup that
# skips staged inputs, an expiry abandon that keeps the input queued,
# serving before the settlement, a verdict without the disposition, four
# verdicts each without one eligibility conjunct, an Abandon arm that
# overlaps Retain, a requeued wait-failure arm that writes state, is
# guarded on the session or answers any failure, a suspend of a run whose
# turn terminal is published, and a staged-row normalization that requeues
# the suspended run's input. It then seeds twelve stop-intent, fence-gap and
# direct-start defects (a fence-gap suspension that leaves the run's turn
# mid-flight, one that spends a stage attempt, one that ignores the draining
# registration, one that drops a cancel the run accepted before the gap, one
# that drops only an ambient cancel accepted before the gap, one that
# suspends a run whose turn already ended, one that unbinds the run's input,
# a resume of a run whose stop was asked, a requeue stop that keeps the input
# of a resumable run whose stop was asked, a settlement of a stopped run as a
# refused resume, a direct conversation run start beside a run marked
# resumable, and a direct immediate append start beside one) and checks each
# against its target invariant alone, with the generated invariants and the
# audit's others left out of that run. An anchor this script expects but
# cannot find fails the run.
set -euo pipefail

usage="usage: requeue_across_stop_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 32 )); then
  echo "error: max-steps must be an integer >= 32 (the deepest witness is 32 steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/requeue-across-stop-audit.XXXXXX")"
# An interrupted or killed run ends its TLC child too (tlc_run_cap.sh).
trap 'if declare -F tlc_run_cap_reap >/dev/null; then tlc_run_cap_reap; fi; rm -rf "${work_dir}"' EXIT

# The scratch spec directory: the generated model, this audit, and the fresh
# machine derived from the generated Init.
model_dir="${work_dir}/specs"
mkdir -p "${model_dir}"
cp "${spec_dir}/model.tla" "${spec_dir}/requeue_across_stop_audit.tla" "${model_dir}/"
MODEL="${model_dir}/model.tla" OUT="${model_dir}/requeue_across_stop_fresh.tla" python3 - <<'PY'
import os, re
text = open(os.environ["MODEL"]).read()
declared = re.search(r"^VARIABLES (.*)$", text, re.M)
assert declared, "VARIABLES line not found in model.tla"
variables = [name.strip() for name in declared.group(1).split(",")]
init = re.search(r"^Init ==\n((?:    /\\ .*\n)+)\n", text, re.M)
assert init, "Init block not found in model.tla"
lines = init.group(1).rstrip("\n").split("\n")
assigned = []
fresh = []
for line in lines:
    match = re.fullmatch(r"    /\\ ([a-z_][a-z_0-9]*) = (.+)", line)
    assert match, f"Init line is not a plain assignment: {line[:120]}"
    name, value = match.groups()
    assigned.append(name)
    if name == "model_step_count":
        # The step bound counts both processes.
        fresh.append("    /\\ model_step_count' = model_step_count + 1")
    else:
        fresh.append(f"    /\\ {name}' = {value}")
assert sorted(assigned) == sorted(variables), (
    "Init assigns a different variable set than VARIABLES declares: "
    f"missing {sorted(set(variables) - set(assigned))}, extra {sorted(set(assigned) - set(variables))}"
)
assert len(assigned) == len(set(assigned)), "Init assigns a variable twice"
with open(os.environ["OUT"], "w") as out:
    out.write("---- MODULE requeue_across_stop_fresh ----\n")
    out.write("\\* Derived by requeue_across_stop_audit.sh from the generated Init: a new\n")
    out.write("\\* process's machine, every variable at its initial value.\n")
    out.write("EXTENDS model\n\n")
    out.write("AuditFreshMachine ==\n")
    out.write("\n".join(fresh))
    out.write("\n\n\\* Every generated variable but the step count.\n")
    out.write("AuditStateButStep == << " + ", ".join(v for v in assigned if v != "model_step_count") + " >>\n")
    out.write("====\n")
print(f"derived AuditFreshMachine from Init ({len(assigned)} variables)")
PY

replace_exact_line() {
  local from="$1" to="$2" file="$3"
  local count
  count="$(grep -cxF -- "${from}" "${file}" || true)"
  if [[ "${count}" != "1" ]]; then
    echo "error: expected exactly one line '${from}' in ${ci_cfg}, found ${count}" >&2
    exit 1
  fi
  FROM="${from}" TO="${to}" awk '$0 == ENVIRON["FROM"] { print ENVIRON["TO"]; next } { print }' \
    "${file}" > "${file}.next"
  mv "${file}.next" "${file}"
}

# usage: write_cfg <file> <settle-before-serve> <extra invariant lines>
write_cfg() {
  local cfg="$1" settle_first="$2" extra="$3"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "  InputIdValues = {}" '  InputIdValues = {"input_1", "input_2"}' "${cfg}"
  # A second run id: the run a direct turn starts (AuditDirectRun).
  replace_exact_line "  RunIdValues = {}" '  RunIdValues = {"runid_1", "runid_2"}' "${cfg}"
  replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1"}' "${cfg}"
  replace_exact_line "  StringValues = {}" '  StringValues = {"input_1", "input_2"}' "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}
  AuditSettleBeforeServe = ${settle_first}" "${cfg}"
  replace_exact_line "INVARIANTS" "PROPERTIES
  AuditRequeuedWaitAnswerChangesNothing
INVARIANTS
  AuditRetainedNeverRunBeforeRestart
  AuditSuspendedRunNeverRestaged
  AuditSuspendedRunNotTerminal
  AuditRetainedGetsNoCarrier
  AuditStoppedInputGetsItsCarrier
  AuditOneRunAcrossStopAndRestart
  AuditExpiredNeverRuns
  AuditRetainOnlyUnderRequeue
  AuditRetainOnlyEligible
  AuditRetainNeverObjectiveBound
  AuditStopVerdictIsAFunction
  AuditRequeuedWaitResolvesInEveryPhase
  AuditFenceGapTurnResettable
  AuditFenceGapKeepsAttempts
  AuditFenceGapNeverWhileDraining
  AuditFenceGapKeepsStopIntent
  AuditStoppedGapRunNeverResumes
  AuditStoppedRunInputNotRetained
  AuditOnlyTheMarkedRunStarts
  AuditStoppedGapRunSettledCancelled${extra}" "${cfg}"
  replace_exact_line "  CiStateConstraint" "  AuditStateConstraint" "${cfg}"
  printf 'CHECK_DEADLOCK FALSE\n' >> "${cfg}"
}

# usage: write_target_cfg <file> <target invariant>: the audit's config with
# no property and one invariant, the one a mutant targets: the generated
# invariants and the audit's others are left out of that run.
write_target_cfg() {
  local cfg="$1" target="$2"
  write_cfg "${cfg}" TRUE ""
  TARGET="${target}" python3 - "${cfg}" <<'PY'
import os, sys
path = sys.argv[1]
out = []
skipping = False
for line in open(path).read().split("\n"):
    if line in ("PROPERTIES", "INVARIANTS"):
        if line == "INVARIANTS":
            out.extend(["INVARIANTS", "  " + os.environ["TARGET"]])
        skipping = True
        continue
    if skipping and line and not line.startswith(" "):
        skipping = False
    if not skipping:
        out.append(line)
assert "  " + os.environ["TARGET"] in out, "the target invariant is not in the config"
assert out.count("INVARIANTS") == 1, "the config has one invariant section"
open(path, "w").write("\n".join(out))
PY
}

# The generated model's initial predicate needs a deep JVM stack on both the
# launcher (JDK_JAVA_OPTIONS) and the VM (JAVA_TOOL_OPTIONS); respect an
# explicit caller policy.
if [[ " ${JAVA_TOOL_OPTIONS:-} " != *" -Xss"* ]]; then
  export JAVA_TOOL_OPTIONS="-Xss512m -XX:+UseParallelGC${JAVA_TOOL_OPTIONS:+ ${JAVA_TOOL_OPTIONS}}"
fi
if [[ " ${JDK_JAVA_OPTIONS:-} " != *" -Xss"* ]]; then
  export JDK_JAVA_OPTIONS="-Xss512m${JDK_JAVA_OPTIONS:+ ${JDK_JAVA_OPTIONS}}"
fi
workers="${TLC_WORKERS:-auto}"
extra_tlc_args=("$@")
# Each TLC run is held to the canonical lane's per-run cap (fails closed as
# TLC INCOMPLETE, exit 3).
# shellcheck source=tlc_run_cap.sh
source "${spec_dir}/tlc_run_cap.sh"

# usage: run_tlc <name> <cfg> <dir>; leaves the log at ${work_dir}/<name>.log
run_tlc() {
  local name="$1" cfg="$2" dir="$3"
  tlc_status=0
  # Witness and mutant runs end in a violation by design: -noGenerateSpecTE
  # keeps TLC from writing trace-exploration files next to the specs.
  tlc_run_capped requeue_across_stop_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "${extra_tlc_args[@]}" "${dir}/requeue_across_stop_audit.tla" || tlc_status=$?
}

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" TRUE ""
run_tlc safety "${safety_cfg}" "${model_dir}"
cat "${work_dir}/safety.log"
if [[ "${tlc_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: requeue-across-stop audit failed (tlc exit ${tlc_status}, bound ${max_steps})" >&2
  exit 1
fi

for witness in RetainedRunsAfterGracefulStop RetainedRunsAfterCrash ExpiredSettled \
  StagedNotRetained StopAbandonsEligible PlainStopAbandons RequeuedWaitAnswered \
  SuspendedRunResumes FenceGapRunResumes IdleFenceGapRunResumes \
  StoppedSuspendedRunEnds StoppedGapRunSettled AmbientCancelledGapRunSettled \
  DirectRunStartsAfterSettlement DirectAppendStartsAfterSettlement; do
  cfg="${work_dir}/witness-${witness}.cfg"
  write_cfg "${cfg}" TRUE "
  NotAuditWitness${witness}"
  run_tlc "witness-${witness}" "${cfg}" "${model_dir}"
  log="${work_dir}/witness-${witness}.log"
  if ! grep -q "Error: Invariant NotAuditWitness${witness} is violated" "${log}"; then
    cat "${log}"
    echo "error: witness ${witness} is not reachable at bound ${max_steps} (tlc exit ${tlc_status})" >&2
    exit 1
  fi
  depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
  steps="$(grep -oE 'model_step_count = [0-9]+' "${log}" | tail -1 | grep -oE '[0-9]+$' || true)"
  echo "witness ${witness} reached in ${depth} states (model_step_count ${steps})"
done

if [[ "${run_mutants}" == "true" ]]; then
  stop_arms=""
  for verdict in Retain Abandon; do
    for arm_phase in Initializing Idle Attached Running Retired Stopped; do
      stop_arms+=" ClassifyStopInputDisposition${verdict}${arm_phase}"
    done
  done

  # usage: mutate <name> <expected invariant> <settle-before-serve>; reads a
  # python edit on stdin that rewrites model.tla in place (MODEL) and must
  # find each anchor it names. ARMS names the stop classifier arms.
  mutate() {
    local name="$1" property="$2" settle_first="$3"
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${model_dir}"/*.tla "${dir}/"
    MODEL="${dir}/model.tla" ARMS="${stop_arms}" PYTHONPATH="${work_dir}" python3 -
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${cfg}" "${settle_first}" ""
    # The stop-intent invariants are left to their own targeted mutants
    # (below): a general mutant that keeps the in-flight input also keeps it
    # after a stop of its run, which they would report first. A renamed one
    # must not stay in silently, so the filter must drop exactly two lines.
    awk '$0 != "  AuditStoppedRunInputNotRetained" && $0 != "  AuditStoppedGapRunSettledCancelled"' \
      "${cfg}" > "${cfg}.next"
    local dropped
    dropped=$(( $(wc -l < "${cfg}") - $(wc -l < "${cfg}.next") ))
    if [[ "${dropped}" != "2" ]]; then
      echo "error: mutant ${name}'s config dropped ${dropped} stop-intent invariant lines, not 2" >&2
      exit 1
    fi
    mv "${cfg}.next" "${cfg}"
    run_tlc "mutant-${name}" "${cfg}" "${dir}"
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -qE "Error: (Invariant|Action property) ${property} is violated" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by ${property} (tlc exit ${tlc_status})" >&2
      exit 1
    fi
    echo "mutant ${name} refused by ${property}"
  }

  # Shared helper for the classifier mutants: apply (old, new) to the guard
  # of every stop classifier arm of the given verdicts, each exactly once.
  cat > "${work_dir}/requeue_mutation.py" <<'PY'
import os

def edit_arms(text, old, new, verdicts=("Retain", "Abandon")):
    for arm in os.environ["ARMS"].split():
        if not any(arm.startswith("ClassifyStopInputDisposition" + v) for v in verdicts):
            continue
        start = text.index("\n" + arm + "(") + 1
        end = text.index("\n\n", start)
        block = text[start:end]
        count = block.count(old)
        assert count == 1, f"expected one {old!r} in {arm}, found {count}"
        text = text[:start] + block.replace(old, new) + text[end:]
    return text
PY

  # P1: a verdict that ignores the phase and the Queue-lane entry (each
  # alone already refuses a staged input: StageForRun removes its lane)
  # keeps the in-flight input, staged when the stop came.
  mutate retains_staged_input AuditRetainedNeverRunBeforeRestart TRUE <<'PY'
import os
from requeue_mutation import edit_arms
path = os.environ["MODEL"]
text = open(path).read()
text = edit_arms(text, ' /\\ ((input_phases)[input_id] = "Queued")', '')
text = edit_arms(text, ' /\\ ((IF (input_id \\in DOMAIN input_lane) THEN Some((IF input_id \\in DOMAIN input_lane THEN input_lane[input_id] ELSE "None")) ELSE None) = Some("Queue"))', '')
open(path, "w").write(text)
PY

  # P2: a verdict that reads a fact the unregister changes between stop
  # transactions answers a replay differently: the input kept by the first
  # stop gets the replay's runless carrier.
  mutate verdict_changes_during_unregister AuditRetainedGetsNoCarrier TRUE <<'PY'
import os
from requeue_mutation import edit_arms
path = os.environ["MODEL"]
text = open(path).read()
text = edit_arms(text, '((registration_phase = "Draining") /\\ (unregister_queued_inputs = "Requeue") /\\ (input_id',
                 '((registration_phase = "Draining") /\\ (unregister_runtime_loop_drain_pending = TRUE) /\\ (unregister_queued_inputs = "Requeue") /\\ (input_id')
open(path, "w").write(text)
PY

  # P2, converse: a verdict that differs across the executor exit (no Retain
  # arm once Stopped) ends an input the carrier pass kept, without its
  # carrier.
  mutate verdict_differs_across_exit AuditStoppedInputGetsItsCarrier TRUE <<'PY'
import os
path = os.environ["MODEL"]
text = open(path).read()
for arm, guard in (("ClassifyStopInputDispositionRetainStopped", "FALSE"),
                   ("ClassifyStopInputDispositionAbandonStopped", "TRUE")):
    start = text.index("\n" + arm + "(") + 1
    end = text.index("\n\n", start)
    lines = text[start:end].split("\n")
    # The queued-prompt guard line; an Abandon arm's resumable-run guard
    # keeps a line of its own.
    hits = [
        i
        for i, line in enumerate(lines)
        if line.startswith("    /\\ ") and "registration_phase" in line and '"Queued"' in line
    ]
    assert len(hits) == 1, f"expected one queued-prompt guard line in {arm}, found {len(hits)}"
    lines[hits[0]] = "    /\\ " + guard
    text = text[:start] + "\n".join(lines) + text[end:]
open(path, "w").write(text)
PY

  # P3: a stop cleanup that ends only queued inputs (as a helper refusing
  # non-Queued inputs would) leaves the in-flight input staged; the restart
  # recovers it and runs it a second time.
  mutate stop_skips_staged_input AuditOneRunAcrossStopAndRestart TRUE <<'PY'
import os
path = os.environ["MODEL"]
text = open(path).read()
head = "AbandonInputStopped(input_id, reason, attempt_count) =="
assert text.count("\n" + head + "\n") == 1, "AbandonInputStopped not found"
start = text.index("\n" + head + "\n") + 1
end = text.index("\n\n", start)
renamed = text[start:end].replace(head, "AbandonInputStoppedGenerated(input_id, reason, attempt_count) ==", 1)
wrapper = (
    "\n\n" + head + "\n"
    "    IF reason = \"Stopped\" /\\ input_id \\in DOMAIN input_phases /\\ input_phases[input_id] = \"Staged\"\n"
    "    THEN phase = \"Stopped\" /\\ UNCHANGED vars\n"
    "    ELSE AbandonInputStoppedGenerated(input_id, reason, attempt_count)"
)
text = text[:start] + renamed + wrapper + text[end:]
open(path, "w").write(text)
PY

  # P4: an expiry abandon that does not end the input lets the expired input
  # run after the restart.
  mutate expiry_keeps_input_queued AuditExpiredNeverRuns TRUE <<'PY'
import os
path = os.environ["MODEL"]
text = open(path).read()
head = "AbandonInputIdle(input_id, reason, attempt_count) =="
assert text.count("\n" + head + "\n") == 1, "AbandonInputIdle not found"
start = text.index("\n" + head + "\n") + 1
end = text.index("\n\n", start)
renamed = text[start:end].replace(head, "AbandonInputIdleGenerated(input_id, reason, attempt_count) ==", 1)
wrapper = (
    "\n\n" + head + "\n"
    "    IF reason = \"RequeueExpired\"\n"
    "    THEN phase = \"Idle\" /\\ UNCHANGED vars\n"
    "    ELSE AbandonInputIdleGenerated(input_id, reason, attempt_count)"
)
text = text[:start] + renamed + wrapper + text[end:]
open(path, "w").write(text)
PY

  # P4, shell order: serving before the settlement runs a kept input that
  # has not passed its expiry check. The model is unchanged; the audit's
  # settlement order is not.
  mutate serves_before_settlement AuditExpiredNeverRuns FALSE <<'PY'
PY

  # P5: a verdict without the disposition keeps inputs under Stop.
  mutate retain_ignores_disposition AuditRetainOnlyUnderRequeue TRUE <<'PY'
import os
from requeue_mutation import edit_arms
path = os.environ["MODEL"]
text = open(path).read()
text = edit_arms(text, '((registration_phase = "Draining") /\\ (unregister_queued_inputs = "Requeue") /\\ (input_id',
                 '((registration_phase = "Draining") /\\ (input_id')
open(path, "w").write(text)
PY

  # Eligibility, one conjunct at a time: each must be load-bearing.
  mutate retains_non_prompt AuditRetainOnlyEligible TRUE <<'PY'
import os
from requeue_mutation import edit_arms
path = os.environ["MODEL"]
text = open(path).read()
text = edit_arms(text, ' /\\ ((IF (input_id \\in DOMAIN input_is_prompt) THEN Some((IF input_id \\in DOMAIN input_is_prompt THEN input_is_prompt[input_id] ELSE FALSE)) ELSE None) = Some(TRUE))', '')
open(path, "w").write(text)
PY
  mutate retains_directed AuditRetainOnlyEligible TRUE <<'PY'
import os
from requeue_mutation import edit_arms
path = os.environ["MODEL"]
text = open(path).read()
text = edit_arms(text, ' /\\ (directed = FALSE)', '')
open(path, "w").write(text)
PY
  mutate retains_non_durable AuditRetainOnlyEligible TRUE <<'PY'
import os
from requeue_mutation import edit_arms
path = os.environ["MODEL"]
text = open(path).read()
text = edit_arms(text, ' /\\ (durability = "Durable")', '')
open(path, "w").write(text)
PY

  # The objective conjunct: a member's kickoff (an objective-bound prompt)
  # is never kept, so its failure record stays true.
  mutate retains_objective_bound AuditRetainNeverObjectiveBound TRUE <<'PY'
import os
from requeue_mutation import edit_arms
path = os.environ["MODEL"]
text = open(path).read()
text = edit_arms(text, ' /\\ (objective_bound = FALSE)', '')
open(path, "w").write(text)
PY

  # The verdict is a function: an Abandon arm that also answers eligible
  # inputs overlaps Retain.
  mutate abandon_overlaps_retain AuditStopVerdictIsAFunction TRUE <<'PY'
import os
path = os.environ["MODEL"]
text = open(path).read()
for arm in os.environ["ARMS"].split():
    if not arm.startswith("ClassifyStopInputDispositionAbandon"):
        continue
    start = text.index("\n" + arm + "(") + 1
    end = text.index("\n\n", start)
    lines = text[start:end].split("\n")
    # The queued-prompt guard line; an Abandon arm's resumable-run guard
    # keeps a line of its own.
    hits = [
        i
        for i, line in enumerate(lines)
        if line.startswith("    /\\ ") and "registration_phase" in line and '"Queued"' in line
    ]
    assert len(hits) == 1, f"expected one queued-prompt guard line in {arm}, found {len(hits)}"
    lines[hits[0]] = "    /\\ TRUE"
    text = text[:start] + "\n".join(lines) + text[end:]
open(path, "w").write(text)
PY

  # The suspend refuses a run whose turn terminal is published: without that
  # guard, a run cancelled before the requeue stop is marked resumable.
  mutate suspends_terminal_run AuditSuspendedRunNotTerminal TRUE <<'PY'
import os
path = os.environ["MODEL"]
text = open(path).read()
for arm in ("SuspendRunForStopRunningToIdle", "SuspendRunForStopRunningToAttached"):
    start = text.index("\n" + arm + "(") + 1
    end = text.index("\n\n", start)
    block = text[start:end]
    old = "    /\\ (turn_terminal_run_id # Some(run_id))\n"
    assert block.count(old) == 1, f"turn terminal guard not found in {arm}"
    text = text[:start] + block.replace(old, "") + text[end:]
open(path, "w").write(text)
PY

  # The staged-row normalization requeues only a row the marked run does not
  # bind: without that guard, the suspended run's input can be queued again
  # and staged into a run of process 2. That state also breaks P3 (A ran in
  # process 1); the config lists this invariant first, so TLC names it.
  mutate requeues_bound_staged_row AuditSuspendedRunNeverRestaged TRUE <<'PY'
import os
path = os.environ["MODEL"]
text = open(path).read()
for arm_phase in ("Initializing", "Idle", "Attached", "Running", "Retired", "Stopped"):
    arm = "NormalizeRecoveredInputStaged" + arm_phase
    start = text.index("\n" + arm + "(") + 1
    end = text.index("\n\n", start)
    block = text[start:end]
    old = "    /\\ (bound_run_resumable = FALSE)\n"
    assert block.count(old) == 1, f"bound guard not found in {arm}"
    text = text[:start] + block.replace(old, "") + text[end:]
open(path, "w").write(text)
PY

  # The requeued wait-failure arm, which has no session guard and changes
  # no state: one that writes state, one guarded on the session, one that
  # answers any failure.
  wait_arms=""
  for arm_phase in Initializing Idle Attached Running Retired Stopped; do
    wait_arms+=" ResolveRuntimeCompletionWaitFailureRequeuedAcrossStop${arm_phase}"
  done
  export WAIT_ARMS="${wait_arms}"
  mutate wait_answer_writes_state AuditRequeuedWaitAnswerChangesNothing TRUE <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
for arm in os.environ["WAIT_ARMS"].split():
    start = text.index("\n" + arm + "(") + 1
    end = text.index("\n\n", start)
    lines = text[start:end].split("\n")
    frames = [i for i, line in enumerate(lines) if re.fullmatch(r"    /\\ UnchangedFrame_[0-9a-f]+", line)]
    assert len(frames) == 1, f"expected one frame in {arm}"
    name = lines[frames[0]].split()[-1]
    definition = re.search(r"(?m)^" + name + r" == UNCHANGED << (.*) >>$", text)
    assert definition, f"frame {name} not found"
    kept = [v.strip() for v in definition.group(1).split(",") if v.strip() != "next_admission_seq"]
    assert len(kept) + 1 == len(definition.group(1).split(",")), "next_admission_seq is not in the frame"
    lines[frames[0]] = ("    /\\ next_admission_seq' = next_admission_seq + 1\n"
                        "    /\\ UNCHANGED << " + ", ".join(kept) + " >>")
    text = text[:start] + "\n".join(lines) + text[end:]
open(path, "w").write(text)
PY
  mutate wait_answer_guarded_on_session AuditRequeuedWaitResolvesInEveryPhase TRUE <<'PY'
import os
path = os.environ["MODEL"]
text = open(path).read()
for arm in os.environ["WAIT_ARMS"].split():
    start = text.index("\n" + arm + "(") + 1
    end = text.index("\n\n", start)
    block = text[start:end]
    old = '    /\\ (failure = "RequeuedAcrossStop")'
    assert block.count(old) == 1, f"guard not found in {arm}"
    block = block.replace(old, old + "\n    /\\ (session_id = Some(arg_session_id))")
    text = text[:start] + block + text[end:]
open(path, "w").write(text)
PY
  mutate wait_answer_any_failure AuditRequeuedWaitResolvesInEveryPhase TRUE <<'PY'
import os
path = os.environ["MODEL"]
text = open(path).read()
for arm in os.environ["WAIT_ARMS"].split():
    start = text.index("\n" + arm + "(") + 1
    end = text.index("\n\n", start)
    block = text[start:end]
    old = '    /\\ (failure = "RequeuedAcrossStop")'
    assert block.count(old) == 1, f"guard not found in {arm}"
    text = text[:start] + block.replace(old, "    /\\ TRUE") + text[end:]
open(path, "w").write(text)
PY

  # The fence-gap defects, each run against its target invariant alone, on
  # one worker so TLC reports the shallowest violation. Each edit keeps the
  # state maps consistent: it changes one fact of the two fence-gap arms.
  export GAP_ARMS="SuspendRunForStopFenceGapRunningToIdle SuspendRunForStopFenceGapRunningToAttached"
  cat > "${work_dir}/gap_mutation.py" <<'PY'
import os, re

def edit_arm(text, arm, select, rewrite):
    start = text.index("\n" + arm + "(") + 1
    end = text.index("\n\n", start)
    lines = text[start:end].split("\n")
    hits = [i for i, line in enumerate(lines) if select(line)]
    assert len(hits) == 1, f"expected one matching line in {arm}, found {len(hits)}"
    lines[hits[0]] = rewrite(lines[hits[0]], text)
    return text[:start] + "\n".join(lines) + text[end:]

def edit_gap_arms(text, select, rewrite):
    for arm in os.environ["GAP_ARMS"].split():
        text = edit_arm(text, arm, select, rewrite)
    return text

def is_frame(line):
    return re.fullmatch(r"    /\\ UnchangedFrame_[0-9a-f]+", line) is not None

def frame_with(var, assignment):
    """Rewrite an arm's frame line so that `var` takes `assignment` and the
    frame keeps every other variable unchanged."""
    def rewrite(line, model):
        name = line.split()[-1]
        definition = re.search(r"(?m)^" + name + r" == UNCHANGED << (.*) >>$", model)
        assert definition, f"frame {name} not found"
        listed = [v.strip() for v in definition.group(1).split(",")]
        kept = [v for v in listed if v != var]
        assert len(kept) + 1 == len(listed), f"{var} is not in the frame"
        return (f"    /\\ {var}' = {assignment}\n"
                "    /\\ UNCHANGED << " + ", ".join(kept) + " >>")
    return rewrite
PY

  # usage: mutate_target <name> <target invariant>; reads a python edit on
  # stdin, as mutate does, and runs the mutant against <target> alone.
  mutate_target() {
    local name="$1" target="$2"
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${model_dir}"/*.tla "${dir}/"
    MODEL="${dir}/model.tla" PYTHONPATH="${work_dir}" python3 -
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_target_cfg "${cfg}" "${target}"
    local saved_workers="${workers}"
    workers=1
    run_tlc "mutant-${name}" "${cfg}" "${dir}"
    workers="${saved_workers}"
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -q "Error: Invariant ${target} is violated" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by ${target} (tlc exit ${tlc_status})" >&2
      exit 1
    fi
    echo "mutant ${name} refused by ${target}"
  }

  # The suspension leaves the run's turn where it was: the in-process resume
  # could not start the turn again.
  mutate_target gap_keeps_turn_phase AuditFenceGapTurnResettable <<'PY'
import os
from gap_mutation import edit_gap_arms
path = os.environ["MODEL"]
text = open(path).read()
text = edit_gap_arms(
    text,
    lambda line: line.startswith("    /\\ turn_phase' = "),
    lambda line, _: "    /\\ turn_phase' = turn_phase",
)
open(path, "w").write(text)
PY

  # The suspension spends a stage attempt of every tracked input, as a
  # release to the lanes and a new staging would.
  mutate_target gap_spends_an_attempt AuditFenceGapKeepsAttempts <<'PY'
import os
from gap_mutation import edit_gap_arms, frame_with, is_frame
path = os.environ["MODEL"]
text = open(path).read()
text = edit_gap_arms(
    text,
    is_frame,
    frame_with(
        "input_attempt_counts",
        "[i \\in DOMAIN input_attempt_counts |-> input_attempt_counts[i] + 1]",
    ),
)
open(path, "w").write(text)
PY

  # The suspension unbinds A from its run, as a release to the lanes would.
  mutate_target gap_drops_run_association AuditFenceGapKeepsAttempts <<'PY'
import os
from gap_mutation import edit_gap_arms, frame_with, is_frame
path = os.environ["MODEL"]
text = open(path).read()
text = edit_gap_arms(
    text,
    is_frame,
    frame_with(
        "input_run_associations",
        '[i \\in (DOMAIN input_run_associations) \\ {"input_1"} |-> input_run_associations[i]]',
    ),
)
open(path, "w").write(text)
PY

  # The suspension ignores a registration draining for its unregister.
  mutate_target gap_while_draining AuditFenceGapNeverWhileDraining <<'PY'
import os
from gap_mutation import edit_gap_arms
path = os.environ["MODEL"]
text = open(path).read()
text = edit_gap_arms(
    text,
    lambda line: line.startswith("    /\\ ") and 'cause = "FenceGap"' in line,
    lambda line, _: '    /\\ (cause = "FenceGap")',
)
open(path, "w").write(text)
PY

  # The suspension drops a cancel the run accepted before the gap: the
  # resumed turn's start would then clear it.
  mutate_target gap_drops_cancel_intent AuditFenceGapKeepsStopIntent <<'PY'
import os
from gap_mutation import edit_gap_arms
path = os.environ["MODEL"]
text = open(path).read()
text = edit_gap_arms(
    text,
    lambda line: line.startswith("    /\\ run_stop_requested' = "),
    lambda line, _: "    /\\ run_stop_requested' = run_stop_requested",
)
open(path, "w").write(text)
PY

  # The suspension carries only the turn's own cancel flag: an ambient
  # cancel accepted before the gap (its boundary dispatch outstanding) is
  # dropped, and the run resumes under its own id.
  mutate_target gap_drops_ambient_cancel AuditFenceGapKeepsStopIntent <<'PY'
import os
from gap_mutation import edit_gap_arms
path = os.environ["MODEL"]
text = open(path).read()
needle = "boundary_cancel_dispatch_pending = TRUE"

def drop_ambient(line, _):
    assert line.count(needle) == 1, f"expected one '{needle}' in: {line}"
    return line.replace(needle, "FALSE")

text = edit_gap_arms(
    text,
    lambda line: line.startswith("    /\\ run_stop_requested' = ") and needle in line,
    drop_ambient,
)
open(path, "w").write(text)
PY

  # The suspension ignores a turn that already ended: a run cancelled before
  # the gap is marked resumable.
  mutate_target gap_suspends_terminal_run AuditSuspendedRunNotTerminal <<'PY'
import os
from gap_mutation import edit_gap_arms
path = os.environ["MODEL"]
text = open(path).read()
text = edit_gap_arms(
    text,
    lambda line: line == "    /\\ (turn_terminal_run_id # Some(run_id))",
    lambda line, _: "    /\\ TRUE",
)
open(path, "w").write(text)
PY

  # The resume ignores the run's stop intent: a run stopped before or during
  # the gap runs again.
  mutate_target resume_ignores_stop AuditStoppedGapRunNeverResumes <<'PY'
import os
from gap_mutation import edit_arm
path = os.environ["MODEL"]
text = open(path).read()
text = edit_arm(
    text,
    "ResumeRunRunning",
    lambda line: line == "    /\\ (run_stop_requested # Some(run_id))",
    lambda line, _: "    /\\ TRUE",
)
open(path, "w").write(text)
PY

  # A requeue stop keeps the input of a resumable run whose stop was asked:
  # the next start would resume the stopped run.
  mutate_target retain_ignores_stop AuditStoppedRunInputNotRetained <<'PY'
import os
from gap_mutation import edit_arm
path = os.environ["MODEL"]
text = open(path).read()
conjunct = "(run_stop_requested # resumable_run_id)"
for phase in ("Initializing", "Idle", "Attached", "Running", "Retired", "Stopped"):
    text = edit_arm(
        text,
        "ClassifyStopInputDispositionRetainResumable" + phase,
        lambda line: line.startswith("    /\\ ") and conjunct in line,
        lambda line, _: line.replace(conjunct, "TRUE"),
    )
open(path, "w").write(text)
PY

  # A suspended run whose stop was asked is settled as a refused resume.
  mutate_target settle_ignores_stop AuditStoppedGapRunSettledCancelled <<'PY'
import os
from gap_mutation import edit_arm
path = os.environ["MODEL"]
text = open(path).read()
for arm in ("SettleResumableRunIdle", "SettleResumableRunAttached"):
    text = edit_arm(
        text,
        arm,
        lambda line: line.startswith("    /\\ ") and '"ResumeRefused"' in line,
        lambda line, _: "    /\\ TRUE",
    )
open(path, "w").write(text)
PY

  # A direct run starts beside a run marked resumable, as it could before
  # the starts from Idle, Initializing and Attached carried Prepare's guard:
  # a direct turn retried during a fence gap starts a run of its own.
  mutate_target direct_start_beside_marked_run AuditOnlyTheMarkedRunStarts <<'PY'
import os
from gap_mutation import edit_arm
path = os.environ["MODEL"]
text = open(path).read()
for arm in (
    "StartConversationRunIdleWithBinding",
    "StartConversationRunInitializing",
    "StartConversationRunAttached",
):
    text = edit_arm(
        text,
        arm,
        lambda line: line == "    /\\ (resumable_run_id = None)",
        lambda line, _: "    /\\ TRUE",
    )
open(path, "w").write(text)
PY

  # The same for a direct immediate append, from Initializing and Attached.
  mutate_target immediate_append_beside_marked_run AuditOnlyTheMarkedRunStarts <<'PY'
import os
from gap_mutation import edit_arm
path = os.environ["MODEL"]
text = open(path).read()
for arm in ("StartImmediateAppendInitializing", "StartImmediateAppendAttached"):
    text = edit_arm(
        text,
        arm,
        lambda line: line == "    /\\ (resumable_run_id = None)",
        lambda line, _: "    /\\ TRUE",
    )
open(path, "w").write(text)
PY
fi
echo "requeue-across-stop audit passed at model_step_count <= ${max_steps}"
