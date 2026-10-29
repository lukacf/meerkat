#!/usr/bin/env bash
# Bounded TLC audit of scoped tool grant expiry over the generated
# ToolGrantLifecycleMachine model: the deadline timer, the model-request
# boundary that shows a gated name, the dispatch gate's fresh admission,
# revocation, reads that settle a due grant without loading the session,
# crash and restart, and one bounded host clock rollback.
#
# usage: tool_grant_expiry_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's model
# values (one grant, deadline 2, a clock in 0..3), its step bound, its five
# invariants and its action property, and checks them over the whole bounded
# space (safety). It then reruns the space once per reachability witness,
# with that witness's negation as an extra invariant, and requires TLC to
# report exactly that violation, so no property holds vacuously:
#   - the deadline timer observes the expiry (ExpiredByTimer);
#   - a boundary before the deadline showed the name, and the racing call
#     adjudicated at the deadline is refused (ExpiredAtDispatch);
#   - the deadline passes while the process is down, and a read settles it
#     (ExpiredWhileDownSettledOnRead);
#   - a call one millisecond before the deadline is handed on
#     (AdmittedOneTickBeforeDeadline);
#   - after the expiry the clock rolls back before the deadline, and a call
#     there is still refused (RollbackAfterExpiryStillRefused);
#   - a revoked grant refuses a call (RevokedThenRefused);
#   - a revoke at the deadline records expiry (RevokeAfterDeadlineRecordsExpiry).
# With --mutants it seeds six defects and requires TLC to refuse each with
# the property it targets. TLC reports only the first violated invariant of a
# state, and one defect can break different properties in different states
# of the same depth, so each mutant run checks the generated invariants and
# only the one audit property its defect targets, listed first:
#   - expire_guard_one_early: ExpireDue fires one millisecond early, in a copy
#     of the generated model (AuditExpiryNotEarly);
#   - admit_guard_inclusive: AdmitLive admits at the deadline, in a copy of
#     the generated model (AuditNoAdmitAtOrAfterDeadline);
#   - dispatch_reuses_boundary_verdict: the dispatch hands on a call because
#     the boundary showed the name (AuditNoAdmitAtOrAfterDeadline);
#   - restart_reactivates_terminal: a restart reloads an ended grant as live
#     (action property AuditTerminalLatched);
#   - revoked_admits: the dispatch hands on a call the revoked grant refused
#     (AuditTerminalNeverAdmits);
#   - boundary_trusts_durable_phase: the boundary shows the name because the
#     stored row is still Active (AuditVisibleOnlyWhileLive).
# An anchor this script expects but cannot find fails the run. Each TLC run
# is held to the canonical lane's per-run cap (tlc_run_cap.sh) and fails
# closed as TLC INCOMPLETE.
set -euo pipefail

usage="usage: tool_grant_expiry_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 4 )); then
  echo "error: max-steps must be an integer >= 4 (the deepest witness takes 3 machine steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/tool-grant-expiry-audit.XXXXXX")"
# An interrupted or killed run ends its TLC child too (tlc_run_cap.sh).
trap 'if declare -F tlc_run_cap_reap >/dev/null; then tlc_run_cap_reap; fi; rm -rf "${work_dir}"' EXIT

# The audit composes these generated arms; each must be defined exactly once.
MODEL="${spec_dir}/model.tla" python3 - <<'PY'
import os
text = open(os.environ["MODEL"]).read()
arms = [
    "Issue", "IssueRefusedWindow",
    "ExpireDue", "ExpireNotDue", "ExpireSettledExpired", "ExpireSettledRevoked",
    "RevokeActive", "RevokeAfterDeadline", "RevokeSettledExpired", "RevokeSettledRevoked",
    "AdmitLive", "AdmitDue", "AdmitEndedExpired", "AdmitEndedRevoked",
]
for arm in arms:
    count = text.count("\n" + arm + "(")
    assert count == 1, f"expected one {arm} definition in model.tla, found {count}"
declared = [line for line in text.split("\n") if line.startswith("VARIABLES ")]
assert len(declared) == 1, "VARIABLES line not found in model.tla"
variables = sorted(name.strip() for name in declared[0][len("VARIABLES "):].split(","))
expected = sorted(["phase", "model_step_count", "grant_id", "issued_at_ms", "expires_at_ms",
                   "ended_at_ms", "end_reason"])
assert variables == expected, f"the audit names the generated variables {expected}, model.tla declares {variables}"
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

audit_invariants=(
  AuditNoAdmitAtOrAfterDeadline
  AuditTerminalNeverAdmits
  AuditExpiryNotEarly
  AuditRevokedBeforeDeadline
  AuditVisibleOnlyWhileLive
)
audit_action_property=AuditTerminalLatched

# usage: write_cfg <file> <mutant> <only property or ""> <extra invariant lines>
# With an only property, the run checks that audit property alone (an
# invariant listed first, or the action property) beside every generated
# invariant; without one, it checks every audit property.
write_cfg() {
  local cfg="$1" mutant="$2" only="$3" extra="$4"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}
  AuditDeadline = 2
  AuditMaxLifetime = 3
  AuditClockMax = 3
  AuditGrantId = \"grant_1\"
  AuditMutant = \"${mutant}\"" "${cfg}"
  local properties="" listed="" name
  if [[ -z "${only}" ]]; then
    properties="  ${audit_action_property}"
    for name in "${audit_invariants[@]}"; do
      listed+="${listed:+
}  ${name}"
    done
  elif [[ "${only}" == "${audit_action_property}" ]]; then
    properties="  ${audit_action_property}"
  elif [[ " ${audit_invariants[*]} " == *" ${only} "* ]]; then
    listed="  ${only}"
  else
    echo "error: unknown audit property ${only}" >&2
    exit 1
  fi
  local invariants_line="INVARIANTS"
  if [[ -n "${listed}${extra}" ]]; then
    invariants_line+="
${listed}${extra}"
  fi
  if [[ -n "${properties}" ]]; then
    replace_exact_line "INVARIANTS" "PROPERTIES
${properties}
${invariants_line}" "${cfg}"
  else
    replace_exact_line "INVARIANTS" "${invariants_line}" "${cfg}"
  fi
  replace_exact_line "  CiStateConstraint" "  AuditStateConstraint" "${cfg}"
  printf 'CHECK_DEADLOCK FALSE\n' >> "${cfg}"
}

# Respect an explicit caller stack and GC policy on both JVM layers.
if [[ " ${JAVA_TOOL_OPTIONS:-} " != *" -Xss"* ]]; then
  export JAVA_TOOL_OPTIONS="-Xss512m -XX:+UseParallelGC${JAVA_TOOL_OPTIONS:+ ${JAVA_TOOL_OPTIONS}}"
fi
if [[ " ${JDK_JAVA_OPTIONS:-} " != *" -Xss"* ]]; then
  export JDK_JAVA_OPTIONS="-Xss512m${JDK_JAVA_OPTIONS:+ ${JDK_JAVA_OPTIONS}}"
fi
workers="${TLC_WORKERS:-auto}"
# shellcheck source=../meerkat_machine/tlc_run_cap.sh
source "${spec_dir}/../meerkat_machine/tlc_run_cap.sh"

# The scratch spec directory: the generated model and this audit.
model_dir="${work_dir}/specs"
mkdir -p "${model_dir}"
cp "${spec_dir}/model.tla" "${spec_dir}/tool_grant_expiry_audit.tla" "${model_dir}/"

# usage: run_tlc <name> <cfg> <dir> [extra tlc args...]; leaves the log at
# ${work_dir}/<name>.log
run_tlc() {
  local name="$1" cfg="$2" dir="$3"
  shift 3
  tlc_status=0
  # Witness and mutant runs end in a violation by design: -noGenerateSpecTE
  # keeps TLC from writing trace-exploration files next to the specs.
  tlc_run_capped tool_grant_expiry_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" "${dir}/tool_grant_expiry_audit.tla" || tlc_status=$?
}

extra_tlc_args=("$@")

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" none "" ""
run_tlc safety "${safety_cfg}" "${model_dir}" "${extra_tlc_args[@]+"${extra_tlc_args[@]}"}"
cat "${work_dir}/safety.log"
if [[ "${tlc_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: tool grant expiry audit failed (tlc exit ${tlc_status}, bound ${max_steps})" >&2
  exit 1
fi

for witness in ExpiredByTimer ExpiredAtDispatch ExpiredWhileDownSettledOnRead \
  AdmittedOneTickBeforeDeadline RollbackAfterExpiryStillRefused RevokedThenRefused \
  RevokeAfterDeadlineRecordsExpiry; do
  cfg="${work_dir}/witness-${witness}.cfg"
  write_cfg "${cfg}" none "" "
  NotAuditWitness${witness}"
  run_tlc "witness-${witness}" "${cfg}" "${model_dir}" "${extra_tlc_args[@]+"${extra_tlc_args[@]}"}"
  log="${work_dir}/witness-${witness}.log"
  if ! grep -q "Error: Invariant NotAuditWitness${witness} is violated" "${log}"; then
    cat "${log}"
    echo "error: witness ${witness} is not reachable at bound ${max_steps} (tlc exit ${tlc_status})" >&2
    exit 1
  fi
  depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
  echo "witness ${witness} reached in ${depth} states"
done

if [[ "${run_mutants}" == "true" ]]; then
  # usage: mutate <name> <expected violation> <property> <mutant> [edit]
  # Copies the specs; an edit "arm|old|new" replaces exactly one <old> line
  # of the named generated arm's definition with <new> in the copy of the
  # generated model. Then it checks the one property and requires TLC to
  # report the expected violation.
  mutate() {
    local name="$1" violation="$2" property="$3" mutant="$4" edit="${5:-}"
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${model_dir}"/*.tla "${dir}/"
    if [[ -n "${edit}" ]]; then
      MODEL="${dir}/model.tla" EDIT="${edit}" python3 - <<'PY'
import os
path = os.environ["MODEL"]
arm, old, new = os.environ["EDIT"].split("|")
text = open(path).read()
heads = text.count("\n" + arm + "(")
assert heads == 1, f"expected one {arm} definition, found {heads}"
start = text.index("\n" + arm + "(") + 1
end = text.index("\n\n", start)
lines = text[start:end].split("\n")
hits = [i for i, line in enumerate(lines) if line == old]
assert len(hits) == 1, f"expected one line {old!r} in {arm}, found {len(hits)}"
lines[hits[0]] = new
text = text[:start] + "\n".join(lines) + text[end:]
open(path, "w").write(text)
PY
    fi
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${cfg}" "${mutant}" "${property}" ""
    run_tlc "mutant-${name}" "${cfg}" "${dir}" "${extra_tlc_args[@]+"${extra_tlc_args[@]}"}"
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -q "Error: ${violation} is violated" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by ${violation} (tlc exit ${tlc_status})" >&2
      exit 1
    fi
    local depth
    depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
    echo "mutant ${name} refused by ${violation} in ${depth} states"
  }

  # The machine's own deadline guards, seeded into a copy of the model.
  mutate expire_guard_one_early "Invariant AuditExpiryNotEarly" AuditExpiryNotEarly none \
    'ExpireDue|    /\ (expires_at_ms <= now_ms)|    /\ (expires_at_ms <= now_ms + 1)'
  mutate admit_guard_inclusive "Invariant AuditNoAdmitAtOrAfterDeadline" \
    AuditNoAdmitAtOrAfterDeadline none \
    'AdmitLive|    /\ (now_ms < expires_at_ms)|    /\ (now_ms <= expires_at_ms)'

  # The shell's use of the machine, seeded through AuditMutant.
  mutate dispatch_reuses_boundary_verdict "Invariant AuditNoAdmitAtOrAfterDeadline" \
    AuditNoAdmitAtOrAfterDeadline dispatch_reuses_boundary_verdict
  mutate restart_reactivates_terminal "Action property AuditTerminalLatched" \
    AuditTerminalLatched restart_reactivates_terminal
  mutate revoked_admits "Invariant AuditTerminalNeverAdmits" AuditTerminalNeverAdmits \
    revoked_admits
  mutate boundary_trusts_durable_phase "Invariant AuditVisibleOnlyWhileLive" \
    AuditVisibleOnlyWhileLive boundary_trusts_durable_phase
fi
echo "tool grant expiry audit passed at model_step_count <= ${max_steps}"
