#!/usr/bin/env bash
# Bounded TLC audit of a host's settle of the run its session retained
# (SettleRetainedRun): the machine's verdict, the stop an accepted settle
# records, and the runtime's settlement of that run, which never resumes.
#
# usage: retained_run_settle_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then gives the audit two
# inputs, two runs and one session, adds its six action properties, its
# invariant and its step bound, and checks them over the whole bounded space.
# It then reruns the same space once per reachability witness, with that
# witness's negation as an extra invariant, and requires TLC to report exactly
# that violation, so the properties are not vacuous. With --mutants it then
# seeds eight defects into a copy of the generated model (an accept of a run
# other than the marked one, an accept with no run marked, an accept with a
# run current, an accept while the registration drains, an accept that
# records no stop, a busy settle that records the stop, a resume that ignores
# the stop, and a settlement of a stopped run as a refused resume) and
# requires TLC to refuse each with the property it breaks. A mutant run uses
# one worker, so the violation TLC reports is the shallowest one. An anchor
# this script expects but cannot find fails the run. Each TLC run is held to
# the canonical lane's per-run cap (`TLC_RUN_CAP_SECS`, default 900 s, see
# tlc_run_cap.sh); a run that hits it fails closed as TLC INCOMPLETE (exit 3).
set -euo pipefail

usage="usage: retained_run_settle_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 10 )); then
  echo "error: max-steps must be an integer >= 10 (the longest witness is 10 steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/retained-run-settle-audit.XXXXXX")"
# An interrupted or killed run ends its TLC child too (tlc_run_cap.sh).
trap 'if declare -F tlc_run_cap_reap >/dev/null; then tlc_run_cap_reap; fi; rm -rf "${work_dir}"' EXIT

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

# usage: write_cfg <file> <extra invariant lines>
write_cfg() {
  local cfg="$1" extra="$2"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "  InputIdValues = {}" '  InputIdValues = {"input_1", "input_2"}' "${cfg}"
  replace_exact_line "  RunIdValues = {}" '  RunIdValues = {"runid_1", "runid_2"}' "${cfg}"
  replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1"}' "${cfg}"
  replace_exact_line "  StringValues = {}" '  StringValues = {"input_1", "input_2"}' "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}" "${cfg}"
  replace_exact_line "INVARIANTS" "PROPERTIES
  AuditSettleChangesOnlyTheStop
  AuditAcceptedSettlesTheRetainedRun
  AuditSettleNotAcceptedWhileDraining
  AuditRefusedSettleRecordsNothing
  AuditSettledRunNeverResumes
  AuditSettledRunEndsCancelled
INVARIANTS
  AuditRunningHasACurrentRun${extra}" "${cfg}"
  replace_exact_line "  CiStateConstraint" "  AuditStateConstraint" "${cfg}"
  printf 'CHECK_DEADLOCK FALSE\n' >> "${cfg}"
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
# shellcheck source=tlc_run_cap.sh
source "${spec_dir}/tlc_run_cap.sh"
cd "${spec_dir}"

# usage: run_tlc <name> <cfg> [extra tlc args...]; runs in the current
# directory and leaves the log at ${work_dir}/<name>.log
run_tlc() {
  local name="$1" cfg="$2"
  shift 2
  local status=0
  # Witness runs end in a violation by design: -noGenerateSpecTE keeps TLC
  # from writing trace-exploration files into the spec directory.
  tlc_run_capped retained_run_settle_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" retained_run_settle_audit.tla || status=$?
  return "${status}"
}

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" ""
safety_status=0
run_tlc safety "${safety_cfg}" "$@" || safety_status=$?
cat "${work_dir}/safety.log"
if [[ "${safety_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: retained-run settle audit failed (tlc exit ${safety_status}, bound ${max_steps})" >&2
  exit 1
fi

for witness in AcceptedUnderHostHold SettledUnderHostHold RepeatAccepted BusyWithRunCurrent \
  BusyStaleObservation BusyWhileDraining RetryNotRetained BusyNotServingInitializing \
  BusyNotServingRetired BusyNotServingStopped InertDestroyed; do
  cfg="${work_dir}/witness-${witness}.cfg"
  write_cfg "${cfg}" "
  NotAuditWitness${witness}"
  status=0
  run_tlc "witness-${witness}" "${cfg}" "$@" || status=$?
  log="${work_dir}/witness-${witness}.log"
  if ! grep -q "Error: Invariant NotAuditWitness${witness} is violated" "${log}"; then
    cat "${log}"
    echo "error: witness ${witness} is not reachable at bound ${max_steps} (tlc exit ${status})" >&2
    exit 1
  fi
  depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
  echo "witness ${witness} reached in ${depth} states"
done

if [[ "${run_mutants}" == "true" ]]; then
  # Shared by every mutant edit: edit_arms rewrites, in each named arm of the
  # generated model, the lines `select` matches, and requires exactly
  # `per_arm` of them in every arm. frame_of names the unchanged frame an arm
  # ends with.
  mutate_prelude='
import os, re
path = os.environ["MODEL"]
text = open(path).read()

def arm_block(arm):
    found = re.compile(r"^" + arm + r"\(.*?\n\n", re.S | re.M).search(text)
    assert found, f"{arm} not found"
    return found

def edit_arms(arms, select, rewrite, per_arm=1):
    global text
    for arm in arms:
        found = arm_block(arm)
        lines = found.group(0).split("\n")
        hits = [index for index, line in enumerate(lines) if select(line)]
        assert len(hits) == per_arm, f"expected {per_arm} matching line(s) in {arm}, found {len(hits)}"
        for index in hits:
            lines[index] = rewrite(lines[index])
        text = text[: found.start()] + "\n".join(lines) + text[found.end():]

def frame_of(arm):
    frames = re.findall(r"^    /\\ (UnchangedFrame_[0-9a-f]+)$", arm_block(arm).group(0), re.M)
    assert len(frames) == 1, f"expected one unchanged frame in {arm}, found {frames}"
    return frames[0]

ACCEPTED = ["SettleRetainedRunAcceptedIdle", "SettleRetainedRunAcceptedAttached"]
'

  # usage: mutate <name> <expected violation regex>; reads a python edit on
  # stdin (run after the prelude above) that rewrites model.tla in place
  # through edit_arms, which fails on any anchor it cannot find.
  mutate() {
    local name="$1" expected="$2"
    shift 2
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}"/*.tla "${dir}/"
    { printf '%s\n' "${mutate_prelude}"; cat; printf '%s\n' 'open(path, "w").write(text)'; } \
      | MODEL="${dir}/model.tla" python3 -
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${cfg}" ""
    local status=0
    # One worker: TLC explores breadth first in order, so it reports the
    # shallowest violation, the one each mutant names.
    local workers=1
    (cd "${dir}" && run_tlc "mutant-${name}" "${cfg}" "$@") || status=$?
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -qE "${expected}" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by '${expected}' (tlc exit ${status})" >&2
      exit 1
    fi
    echo "mutant ${name} refused: ${expected}"
  }

  # The settle is accepted for a run other than the marked one (8 steps): it
  # records the stop of a run that is not retained.
  mutate accept_other_run "property AuditAcceptedSettlesTheRetainedRun is violated" "$@" <<'PY'
edit_arms(
    ACCEPTED,
    lambda line: line == "    /\\ (resumable_run_id = run_id)",
    lambda line: "    /\\ TRUE",
)
PY

  # The settle is accepted with no run marked (10 steps: after the kept run
  # was settled, a settle that observed no run).
  mutate accept_with_no_run_retained "property AuditAcceptedSettlesTheRetainedRun is violated" "$@" <<'PY'
edit_arms(
    ACCEPTED,
    lambda line: line == "    /\\ (resumable_run_id # None)",
    lambda line: "    /\\ TRUE",
)
PY

  # The settle is accepted while a run is current (9 steps: the kept run
  # started with nothing held and has not resumed yet): the Idle arm also
  # answers a Running registration, with no current-run guard.
  mutate accept_with_a_current_run "property AuditAcceptedSettlesTheRetainedRun is violated" "$@" <<'PY'
edit_arms(
    ["SettleRetainedRunAcceptedIdle"],
    lambda line: line == "    /\\ phase = \"Idle\"",
    lambda line: "    /\\ phase \\in {\"Idle\", \"Running\"}",
)
edit_arms(
    ["SettleRetainedRunAcceptedIdle"],
    lambda line: line == "    /\\ phase' = \"Idle\"",
    lambda line: "    /\\ phase' = phase",
)
edit_arms(
    ["SettleRetainedRunAcceptedIdle"],
    lambda line: line == "    /\\ (current_run_id = None)",
    lambda line: "    /\\ TRUE",
)
PY

  # The settle is accepted while the registration drains for its unregister
  # (9 steps), whose next registration would find the run open again.
  mutate accept_while_draining "property AuditSettleNotAcceptedWhileDraining is violated" "$@" <<'PY'
edit_arms(
    ACCEPTED,
    lambda line: line == "    /\\ (registration_phase # \"Draining\")",
    lambda line: "    /\\ TRUE",
)
PY

  # An accepted settle records no stop (8 steps): the run would resume.
  mutate accept_records_no_stop "property AuditAcceptedSettlesTheRetainedRun is violated" "$@" <<'PY'
edit_arms(
    ACCEPTED,
    lambda line: line == "    /\\ run_stop_requested' = run_id",
    lambda line: "    /\\ run_stop_requested' = run_stop_requested",
)
PY

  # A settle refused as busy for a stale observation records the stop
  # anyway (8 steps).
  mutate busy_records_the_stop "property AuditRefusedSettleRecordsNothing is violated" "$@" <<'PY'
accepted_frame = frame_of(ACCEPTED[0])
for arm in ["SettleRetainedRunBusyStaleRunIdle", "SettleRetainedRunBusyStaleRunAttached"]:
    busy_frame = frame_of(arm)
    edit_arms(
        [arm],
        lambda line, busy_frame=busy_frame: line == "    /\\ " + busy_frame,
        lambda line: "    /\\ run_stop_requested' = run_id /\\ " + accepted_frame,
    )
PY

  # The resume ignores the stop the settle recorded (10 steps): the settled
  # run starts and resumes.
  mutate resume_ignores_the_stop "property AuditSettledRunNeverResumes is violated" "$@" <<'PY'
edit_arms(
    ["ResumeRunRunning"],
    lambda line: line == "    /\\ (run_stop_requested # Some(run_id))",
    lambda line: "    /\\ TRUE",
)
PY

  # The runtime settles a run whose settle was accepted as a refused resume
  # (9 steps).
  mutate settled_run_refused "property AuditSettledRunEndsCancelled is violated" "$@" <<'PY'
edit_arms(
    ["SettleResumableRun" + phase
     for phase in ("Initializing", "Idle", "Attached", "Retired", "Stopped")],
    lambda line: line == "    /\\ (IF (run_stop_requested # Some(run_id)) THEN TRUE ELSE (reason # \"ResumeRefused\"))",
    lambda line: "    /\\ TRUE",
)
PY
fi
echo "retained-run settle audit passed at model_step_count <= ${max_steps}"
