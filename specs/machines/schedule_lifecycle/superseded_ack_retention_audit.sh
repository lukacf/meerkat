#!/usr/bin/env bash
# Bounded TLC audit of the retention of superseded_ack_ids in the generated
# ScheduleLifecycleMachine: the set holds only the acknowledgements of the
# latest supersession. Revise* and Delete* drop every recorded id before the
# new supersession's acknowledgements arrive, ConfirmOccurrencesSuperseded*
# record an acknowledgement only when it answers the current revision, and
# every other acknowledgement is accepted and changes nothing.
#
# usage: superseded_ack_retention_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's step
# bound and its two invariants, and checks them over the whole bounded space
# (safety). It then reruns the same space once per scenario witness, with that
# witness's negation as an extra invariant, and requires TLC to report exactly
# that violation, so no scenario holds vacuously:
#   - a revise dropped an earlier supersession's acknowledgement while the
#     latest one is recorded (ReviseDropsEarlierAck);
#   - a late acknowledgement of a dropped id was accepted and left the set
#     without it (LateAckAccepted);
#   - a delete dropped an earlier supersession's acknowledgement and recorded
#     its own (DeleteDropsEarlierAck).
# With --mutants it seeds one defect at a time into a copy of the generated
# model and requires TLC to refuse each with the invariant it breaks (TLC
# stops at its first violation and reports only the first violated invariant
# of a state, so each mutant run lists its expected invariant first):
#   - ReviseActive, RevisePaused, DeleteActive or DeletePaused keeping the old
#     ids (AuditAckSetAnswersTheLatestSupersession);
#   - ConfirmOccurrencesSupersededActive or ConfirmOccurrencesSupersededPaused
#     recording every acknowledgement, late ones included, as before the
#     bound (AuditAckSetAnswersTheLatestSupersession);
#   - ConfirmOccurrencesSupersededPaused or ConfirmOccurrencesSupersededDeleted
#     recording none (AuditLatestSupersessionAcksAreKept).
# So every arm the bound changed is seeded at least once, and every
# acknowledgement arm in at least one direction.
# Each mutant replaces the one superseded_ack_ids update of its arm, so it
# needs the model generated from the bounded DSL; an arm without exactly one
# such update fails the run. Each TLC run is held to the canonical lane's
# per-run cap (tlc_run_cap.sh) and fails closed as TLC INCOMPLETE.
set -euo pipefail

usage="usage: superseded_ack_retention_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 4 )); then
  echo "error: max-steps must be an integer >= 4 (every witness and mutant needs 4 steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/superseded-ack-retention-audit.XXXXXX")"
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

audit_invariants=(
  AuditAckSetAnswersTheLatestSupersession
  AuditLatestSupersessionAcksAreKept
)

# usage: write_cfg <file> <first invariant or ""> <extra invariant lines>
# The first invariant (an audit or generated one) is listed before every
# other: TLC reports only the first violated invariant of a state.
write_cfg() {
  local cfg="$1" first="$2" extra="$3"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}" "${cfg}"
  local listed="" name
  if [[ -n "${first}" ]]; then
    if grep -qxF -- "  ${first}" "${cfg}"; then
      # A generated invariant moves to the front.
      FIRST="  ${first}" awk '$0 == ENVIRON["FIRST"] { next } { print }' "${cfg}" > "${cfg}.next"
      mv "${cfg}.next" "${cfg}"
    elif [[ " ${audit_invariants[*]} " != *" ${first} "* ]]; then
      echo "error: unknown first invariant ${first}" >&2
      exit 1
    fi
    listed="  ${first}"
  fi
  for name in "${audit_invariants[@]}"; do
    if [[ "${name}" != "${first}" ]]; then
      listed+="${listed:+
}  ${name}"
    fi
  done
  replace_exact_line "INVARIANTS" "INVARIANTS
${listed}${extra}" "${cfg}"
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
# shellcheck source=../meerkat_machine/tlc_run_cap.sh
source "${spec_dir}/../meerkat_machine/tlc_run_cap.sh"
cd "${spec_dir}"

# usage: run_tlc <name> <cfg> [extra tlc args...]; runs in the current
# directory and leaves the log at ${work_dir}/<name>.log
run_tlc() {
  local name="$1" cfg="$2"
  shift 2
  local status=0
  # Witness and mutant runs end in a violation by design: -noGenerateSpecTE
  # keeps TLC from writing trace-exploration files into the spec directory.
  tlc_run_capped superseded_ack_retention_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" superseded_ack_retention_audit.tla || status=$?
  return "${status}"
}

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" "" ""
safety_status=0
run_tlc safety "${safety_cfg}" "$@" || safety_status=$?
safety_log="${work_dir}/safety.log"
cat "${safety_log}"
if [[ "${safety_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${safety_log}"; then
  echo "error: superseded-ack retention audit failed (tlc exit ${safety_status}, bound ${max_steps})" >&2
  exit 1
fi

for witness in ReviseDropsEarlierAck LateAckAccepted DeleteDropsEarlierAck; do
  cfg="${work_dir}/witness-${witness}.cfg"
  write_cfg "${cfg}" "" "
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
  tlc_args=("$@")
  # usage: mutate <name> <expected violation> <first invariant> <arm> <new right-hand side>
  # Copies the specs and, in the copy of the generated model, replaces the
  # right-hand side of the arm's one superseded_ack_ids update. Then it runs
  # every invariant, the expected one first, and requires TLC to report the
  # expected violation.
  mutate() {
    local name="$1" violation="$2" first="$3" arm="$4" rhs="$5"
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}/model.tla" "${spec_dir}/superseded_ack_retention_audit.tla" "${dir}/"
    MODEL="${dir}/model.tla" ARM="${arm}" RHS="${rhs}" python3 - <<'PY'
import os
import re

path = os.environ["MODEL"]
arm = os.environ["ARM"]
text = open(path).read()
heads = text.count("\n" + arm + "(")
assert heads == 1, f"expected one {arm} definition, found {heads}"
start = text.index("\n" + arm + "(") + 1
end = text.index("\n\n", start)
block = text[start:end]
updates = [line for line in block.split("\n") if re.match(r"\s*/\\ superseded_ack_ids' = ", line)]
assert len(updates) == 1, (
    f"expected one superseded_ack_ids update in {arm}, found {len(updates)}: "
    "regenerate the model from the DSL (cargo xtask machine-codegen --all)"
)
prefix = updates[0][: updates[0].index("superseded_ack_ids'")]
text = text[:start] + block.replace(updates[0], prefix + "superseded_ack_ids' = " + os.environ["RHS"]) + text[end:]
open(path, "w").write(text)
PY
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${cfg}" "${first}" ""
    local status=0
    (cd "${dir}" && run_tlc "mutant-${name}" "${cfg}" "${tlc_args[@]+"${tlc_args[@]}"}") || status=$?
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -q "Error: ${violation} is violated" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by ${violation} (tlc exit ${status})" >&2
      exit 1
    fi
    local depth
    depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
    echo "mutant ${name} refused by ${violation} in ${depth} states"
  }

  for arm in ReviseActive RevisePaused DeleteActive DeletePaused; do
    mutate "${arm}_keeps_old_acks" "Invariant AuditAckSetAnswersTheLatestSupersession" \
      AuditAckSetAnswersTheLatestSupersession "${arm}" "superseded_ack_ids"
  done
  for arm in ConfirmOccurrencesSupersededActive ConfirmOccurrencesSupersededPaused; do
    mutate "${arm}_records_late_acks" "Invariant AuditAckSetAnswersTheLatestSupersession" \
      AuditAckSetAnswersTheLatestSupersession "${arm}" '(superseded_ack_ids \cup {occurrence_id})'
  done
  for arm in ConfirmOccurrencesSupersededPaused ConfirmOccurrencesSupersededDeleted; do
    mutate "${arm}_drops_current_acks" "Invariant AuditLatestSupersessionAcksAreKept" \
      AuditLatestSupersessionAcksAreKept "${arm}" "superseded_ack_ids"
  done
fi
echo "superseded-ack retention audit passed at model_step_count <= ${max_steps}"
