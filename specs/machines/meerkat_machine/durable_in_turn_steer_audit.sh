#!/usr/bin/env bash
# Bounded TLC audit of durable in-turn Steer delivery.
#
# usage: durable_in_turn_steer_audit.sh <max-steps> [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's finite
# model values, its three exactly-once invariants, its step bound and its
# deterministic-prefix constraint. Nothing is hand-copied, so a DSL change
# regenerates ci.cfg and the audit follows it; an anchor this script expects
# but cannot find fails the run instead of silently narrowing the check.
set -euo pipefail

max_steps="${1:?usage: durable_in_turn_steer_audit.sh <max-steps> [extra tlc args...]}"
shift
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 9 )); then
  echo "error: max-steps must be an integer >= 9 (the deterministic prefix is 8 steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/durable-steer-audit.XXXXXX")"
trap 'rm -rf "${work_dir}"' EXIT
audit_cfg="${work_dir}/audit.cfg"

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

cp "${ci_cfg}" "${audit_cfg}"
replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${audit_cfg}"
replace_exact_line "  InputIdValues = {}" '  InputIdValues = {"input_batch", "input_late"}' "${audit_cfg}"
replace_exact_line "  RunIdValues = {}" '  RunIdValues = {"runid_1"}' "${audit_cfg}"
replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1"}' "${audit_cfg}"
replace_exact_line "  StringValues = {}" '  StringValues = {"input_batch", "input_late", "alpha"}' "${audit_cfg}"
replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}" "${audit_cfg}"
replace_exact_line "INVARIANTS" "INVARIANTS
  AuditRetainedJoinNeverRequeued
  AuditPublishedJoinIsStaged
  AuditJoinedInputNotInAnyLane" "${audit_cfg}"
replace_exact_line "  CiStateConstraint" "  AuditStateConstraint" "${audit_cfg}"
printf 'CHECK_DEADLOCK FALSE\n' >> "${audit_cfg}"

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
log="${work_dir}/tlc.log"
cd "${spec_dir}"
set +e
tlc -workers "${workers}" -metadir "${work_dir}/states" -config "${audit_cfg}" "$@" \
  durable_in_turn_steer_audit.tla 2>&1 | tee "${log}"
tlc_status="${PIPESTATUS[0]}"
set -e
if [[ "${tlc_status}" != "0" ]] || ! grep -q "Model checking completed. No error has been found." "${log}"; then
  echo "error: durable in-turn steer audit failed (tlc exit ${tlc_status}, bound ${max_steps})" >&2
  exit 1
fi
echo "durable in-turn steer audit passed at model_step_count <= ${max_steps}"
