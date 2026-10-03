#!/usr/bin/env bash
# Bounded TLC audit of the live-context result barrier: a delegation result
# is released after the bootstrap summary ACK, not after queued replays.
#
# usage: live_context_result_barrier_audit.sh <max-steps> [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's finite
# model values, its step bound and its deterministic-prefix constraint. It runs
# TLC once with every generated invariant (safety), then once per goal with
# that goal as an extra property, and requires TLC to report each goal
# violated: the bound provably reaches the transitions the barrier invariants
# are about, so they are not vacuously true. Nothing is hand-copied, so a DSL
# change regenerates ci.cfg and the audit follows it; an anchor this script
# expects but cannot find fails the run instead of silently narrowing the
# check.
set -euo pipefail

max_steps="${1:?usage: live_context_result_barrier_audit.sh <max-steps> [extra tlc args...]}"
shift
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 21 )); then
  echo "error: max-steps must be an integer >= 21 (the goal needs 21 steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/live-context-result-barrier-audit.XXXXXX")"
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
replace_exact_line "  OperationIdValues = {}" '  OperationIdValues = {"operation_1"}' "${audit_cfg}"
replace_exact_line "  StringValues = {}" '  StringValues = {"", "channel_a", "profile_1", "pending_a", "owner_1", "ready_1", "activation_a", "lease_1", "digest_s", "append_boot", "append_2", "digest_1", "commit_1", "interaction_1", "turn_1", "worker_1", "result_1"}' "${audit_cfg}"
replace_exact_line "INVARIANTS" "INVARIANTS
  AuditResultFollowsSummary" "${audit_cfg}"
replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}" "${audit_cfg}"
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
  tlc_run_capped live_context_result_barrier_audit "${name}" "${log}" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE -config "${cfg}" "${extra_tlc_args[@]}" \
    live_context_result_barrier_audit.tla || tlc_status=$?
  grep -E 'states generated|distinct states|is violated|Error:' "${log}" | sed "s/^/[${name}] /" || true
}
extra_tlc_args=("$@")

run_tlc safety "${audit_cfg}"
if [[ "${tlc_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  cat "${work_dir}/safety.log" >&2
  echo "error: live-context result barrier audit safety run failed (tlc exit ${tlc_status}, bound ${max_steps})" >&2
  exit 1
fi

# Each goal must be reachable within the bound: TLC has to report it violated.
for goal in \
  "INVARIANT AuditNeverDeliveredWithQueuedReplay"; do
  kind="${goal%% *}"
  name="${goal#* }"
  goal_cfg="${work_dir}/${name}.cfg"
  cp "${audit_cfg}" "${goal_cfg}"
  printf '%s\n  %s\n' "${kind}" "${name}" >> "${goal_cfg}"
  run_tlc "${name}" "${goal_cfg}"
  if ! grep -q "${name} is violated" "${work_dir}/${name}.log"; then
    cat "${work_dir}/${name}.log" >&2
    echo "error: live-context result barrier audit goal ${name} is not reached within bound ${max_steps}" >&2
    exit 1
  fi
done
echo "live-context result barrier audit passed at model_step_count <= ${max_steps} (1 goal reached)"
