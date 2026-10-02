#!/usr/bin/env bash
# Bounded TLC audit of live channel binding cleanup on UnregisterSession
# (#1476).
#
# usage: live_unregister_cleanup_audit.sh <max-steps> [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's finite
# model values, its step bound and its deterministic-prefix constraint. It runs
# TLC once with every generated invariant (safety), then once per goal with
# that goal as an extra property, and requires TLC to report each goal
# violated: the bound provably reaches the transitions the cleanup invariant
# is about, so they are not vacuously true. Nothing is hand-copied, so a DSL
# change regenerates ci.cfg and the audit follows it; an anchor this script
# expects but cannot find fails the run instead of silently narrowing the
# check.
set -euo pipefail

max_steps="${1:?usage: live_unregister_cleanup_audit.sh <max-steps> [extra tlc args...]}"
shift
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 14 )); then
  echo "error: max-steps must be an integer >= 20 (the deepest goal needs 14 steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/live-unregister-cleanup-audit.XXXXXX")"
# An interrupted or killed run ends its TLC child too (tlc_run_cap.sh).
trap 'if declare -F tlc_run_cap_reap >/dev/null; then tlc_run_cap_reap; fi; rm -rf "${work_dir}"' EXIT
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
replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1"}' "${audit_cfg}"
replace_exact_line "  AgentRuntimeIdValues = {}" '  AgentRuntimeIdValues = {"runtime_1"}' "${audit_cfg}"
replace_exact_line "  SessionLlmIdentityValues = {}" '  SessionLlmIdentityValues = {"identity_1"}' "${audit_cfg}"
replace_exact_line "  StringValues = {}" '  StringValues = {"", "channel_a", "profile_1", "pending_a", "owner_1", "ready_1", "activation_a", "lease_1", "run_1", "input_1"}' "${audit_cfg}"
replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}
  AuditStart = AUDIT_START" "${audit_cfg}"
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
# Each TLC run is held to the canonical lane's per-run cap (fails closed as
# TLC INCOMPLETE, exit 3).
# shellcheck source=tlc_run_cap.sh
source "${spec_dir}/tlc_run_cap.sh"
cd "${spec_dir}"

# run_tlc <name> <cfg> -> leaves the log at ${work_dir}/<name>.log
run_tlc() {
  local name="$1" cfg="$2"
  local log="${work_dir}/${name}.log"
  tlc_status=0
  # Goal runs end in a violation by design: -noGenerateSpecTE keeps TLC from
  # writing trace-explorer specs next to the model.
  tlc_run_capped live_unregister_cleanup_audit "${name}" "${log}" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE -config "${cfg}" "${extra_tlc_args[@]}" \
    live_unregister_cleanup_audit.tla || tlc_status=$?
  grep -E 'states generated|distinct states|is violated|Error:' "${log}" | sed "s/^/[${name}] /" || true
}
extra_tlc_args=("$@")

for start in admitted staged bound running retired; do
  start_cfg="${work_dir}/${start}.cfg"
  sed 's/AUDIT_START/"'"${start}"'"/' "${audit_cfg}" > "${start_cfg}"
  printf 'PROPERTY\n  AuditUnregisterNeverWhileBound\n' >> "${start_cfg}"
  run_tlc "safety-${start}" "${start_cfg}"
  if [[ "${tlc_status}" != "0" ]] \
    || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety-${start}.log"; then
    cat "${work_dir}/safety-${start}.log" >&2
    echo "error: live unregister cleanup audit safety run from ${start} failed (tlc exit ${tlc_status}, bound ${max_steps})" >&2
    exit 1
  fi
  # No wedge: unregister must be reachable from this start within the bound.
  goal_cfg="${work_dir}/${start}-goal.cfg"
  cp "${start_cfg}" "${goal_cfg}"
  printf 'PROPERTY\n  AuditNeverUnregisters\n' >> "${goal_cfg}"
  run_tlc "AuditNeverUnregisters-${start}" "${goal_cfg}"
  if ! grep -q "AuditNeverUnregisters is violated" "${work_dir}/AuditNeverUnregisters-${start}.log"; then
    cat "${work_dir}/AuditNeverUnregisters-${start}.log" >&2
    echo "error: unregister is not reachable from ${start} within bound ${max_steps}" >&2
    exit 1
  fi
  if [[ "${start}" == "staged" ]]; then
    prep_cfg="${work_dir}/${start}-prep.cfg"
    cp "${start_cfg}" "${prep_cfg}"
    printf 'PROPERTY\n  AuditNeverUnregistersWithPreparation\n' >> "${prep_cfg}"
    run_tlc "AuditNeverUnregistersWithPreparation-${start}" "${prep_cfg}"
    if ! grep -q "AuditNeverUnregistersWithPreparation is violated" "${work_dir}/AuditNeverUnregistersWithPreparation-${start}.log"; then
      cat "${work_dir}/AuditNeverUnregistersWithPreparation-${start}.log" >&2
      echo "error: unregister never meets context-preparation records from ${start} within bound ${max_steps}" >&2
      exit 1
    fi
  fi
done
echo "live unregister cleanup audit passed at model_step_count <= ${max_steps} (unregister reachable from admitted, staged, bound, running and retired)"
