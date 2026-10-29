#!/usr/bin/env bash
# Bounded TLC audit of the machine-owned controller holds: a kept governed
# input that waits, held by the machine, for its host to supply its
# controller client again.
#
# usage: input_controller_hold_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then gives the audit two
# inputs, two runs and one session, adds its three action properties and its
# step bound, and checks them over the whole bounded space. It then reruns the
# same space once per reachability witness, with that witness's negation as an
# extra invariant, and requires TLC to report exactly that violation, so the
# properties are not vacuous. With --mutants it then seeds seven defects into
# a copy of the generated model (a stage, an apply and a resume that ignore
# the hold, a settlement and an abandon that keep it, a hold that ignores the
# input's phase, a release that clears every hold) and requires TLC to refuse
# each with the property or invariant it breaks. A mutant run uses one worker,
# so the violation TLC reports is the shallowest one. An anchor this script
# expects but cannot find fails the run. Each TLC run is held to the canonical
# lane's per-run cap (`TLC_RUN_CAP_SECS`, default 900 s, see tlc_run_cap.sh);
# a run that hits it fails closed as TLC INCOMPLETE (exit 3).
set -euo pipefail

usage="usage: input_controller_hold_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 13 )); then
  echo "error: max-steps must be an integer >= 13 (the longest witness is 13 steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/input-controller-hold-audit.XXXXXX")"
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
  AuditHeldWorkNeverAdvances
  AuditHeldRunNeverResumes
  AuditHoldAndReleaseTouchOnlyHolds
INVARIANTS${extra}" "${cfg}"
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
  tlc_run_capped input_controller_hold_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" input_controller_hold_audit.tla || status=$?
  return "${status}"
}

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" ""
safety_status=0
run_tlc safety "${safety_cfg}" "$@" || safety_status=$?
cat "${work_dir}/safety.log"
if [[ "${safety_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: input controller hold audit failed (tlc exit ${safety_status}, bound ${max_steps})" >&2
  exit 1
fi

for witness in HeldWhileRunCurrent ReleasedThenStaged HeldEndsRefused HeldRunWaits \
  ReleasedRunResumes SettledDropsHold ReleasedRunRequeues; do
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
  # `per_arm` of them in every arm.
  mutate_prelude='
import os, re
path = os.environ["MODEL"]
text = open(path).read()

def edit_arms(arms, select, rewrite, per_arm=1):
    global text
    for arm in arms:
        found = re.compile(r"^" + arm + r"\(.*?\n\n", re.S | re.M).search(text)
        assert found, f"{arm} not found"
        lines = found.group(0).split("\n")
        hits = [index for index, line in enumerate(lines) if select(line)]
        assert len(hits) == per_arm, f"expected {per_arm} matching line(s) in {arm}, found {len(hits)}"
        for index in hits:
            lines[index] = rewrite(lines[index])
        text = text[: found.start()] + "\n".join(lines) + text[found.end():]

def arms(name, phases):
    return [name + phase for phase in phases]

ALL = ("Idle", "Attached", "Running", "Retired", "Stopped")

def guard_on_holds(line):
    return (line.startswith("    /\\ ")
            and re.search(r"\\(?:not)?in input_controller_holds\b", line) is not None
            and "input_controller_holds'"'"'" not in line)

def holds_update(line):
    return line.startswith("    /\\ input_controller_holds'"'"' = ")
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

  hold_invariant="Invariant input_controller_holds_are_kept_unstaged_governed_inputs is violated"

  # A held queued input stages into the marked run (12 steps): only the
  # property sees it, since the staged input is bound to the marked run.
  mutate stage_ignores_hold "property AuditHeldWorkNeverAdvances is violated" "$@" <<'PY'
edit_arms(arms("StageForRun", ALL), guard_on_holds, lambda line: "    /\\ TRUE")
PY

  # The held input bound to the marked run is applied (11 steps).
  mutate apply_ignores_hold "property AuditHeldWorkNeverAdvances is violated" "$@" <<'PY'
edit_arms(arms("MarkApplied", ALL), guard_on_holds, lambda line: "    /\\ TRUE")
PY

  # The marked run resumes with its input held (12 steps): the held input is
  # then bound to a run no longer marked, which the generated invariant
  # refuses before the action property is checked.
  mutate resume_ignores_hold "${hold_invariant}" "$@" <<'PY'
edit_arms(["ResumeRunRunning"], guard_on_holds, lambda line: "    /\\ TRUE")
PY

  mutate settle_keeps_hold "${hold_invariant}" "$@" <<'PY'
edit_arms(
    arms("SettleResumableRun", ("Initializing",) + tuple(p for p in ALL if p != "Running")),
    holds_update,
    lambda line: "    /\\ input_controller_holds' = input_controller_holds",
)
PY

  mutate abandon_keeps_hold "${hold_invariant}" "$@" <<'PY'
edit_arms(
    arms("AbandonInput", ALL),
    holds_update,
    lambda line: "    /\\ input_controller_holds' = input_controller_holds",
)
PY

  mutate hold_ignores_phase "${hold_invariant}" "$@" <<'PY'
edit_arms(
    arms("HoldInputController", ALL),
    lambda line: line.startswith("    /\\ ") and "resumable_run_id" in line and "'" not in line,
    lambda line: "    /\\ TRUE",
)
PY

  # Two inputs held, one released (12 steps).
  mutate release_clears_every_hold "property AuditHoldAndReleaseTouchOnlyHolds is violated" "$@" <<'PY'
edit_arms(
    arms("ReleaseInputController", ALL),
    holds_update,
    lambda line: "    /\\ input_controller_holds' = {}",
)
PY
fi
echo "input controller hold audit passed at model_step_count <= ${max_steps}"
