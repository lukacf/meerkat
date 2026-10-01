#!/usr/bin/env bash
# Bounded TLC audit of the live delegation steer edges.
#
# usage: live_delegation_steer_audit.sh <max-steps> [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), adds the audit's finite model
# values, its invariants, its step bound and its deterministic-prefix
# constraint, then:
#   1. checks every invariant over the audit's state space (must pass), and
#   2. proves each steer outcome reachable: for each goal it checks the goal's
#      negation and requires TLC to report that invariant violated.
# An anchor this script expects but cannot find fails the run instead of
# silently narrowing the check. Each TLC run is held to the canonical lane's
# per-run cap (`TLC_RUN_CAP_SECS`, default 900 s); a run that hits it fails as
# INCOMPLETE (exit 3).
set -euo pipefail

max_steps="${1:?usage: live_delegation_steer_audit.sh <max-steps> [extra tlc args...]}"
shift
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 11 )); then
  echo "error: max-steps must be an integer >= 11 (the deterministic prefix is 8 steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/live-steer-audit.XXXXXX")"
trap 'rm -rf "${work_dir}"' EXIT

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

# $1: output cfg; $2: invariant block appended after the generated ones.
derive_cfg() {
  local cfg="$1" extra_invariants="$2"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"session_1"}' "${cfg}"
  replace_exact_line "  AgentRuntimeIdValues = {}" '  AgentRuntimeIdValues = {"runtime_1"}' "${cfg}"
  replace_exact_line "  OperationIdValues = {}" '  OperationIdValues = {"operation_1"}' "${cfg}"
  replace_exact_line "  SessionLlmIdentityValues = {}" '  SessionLlmIdentityValues = {"identity_1"}' "${cfg}"
  replace_exact_line "  StringValues = {}" '  StringValues = {"session_1", "channel_1", "interaction_1", "turn_1", "worker_1", "continuation_1", "continuation_2", "digest_1"}' "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}" "${cfg}"
  replace_exact_line "INVARIANTS" "INVARIANTS
${extra_invariants}" "${cfg}"
  replace_exact_line "  CiStateConstraint" "  AuditStateConstraint" "${cfg}"
  printf 'CHECK_DEADLOCK FALSE\n' >> "${cfg}"
}

if [[ " ${JAVA_TOOL_OPTIONS:-} " != *" -Xss"* ]]; then
  export JAVA_TOOL_OPTIONS="-Xss512m -XX:+UseParallelGC${JAVA_TOOL_OPTIONS:+ ${JAVA_TOOL_OPTIONS}}"
fi
if [[ " ${JDK_JAVA_OPTIONS:-} " != *" -Xss"* ]]; then
  export JDK_JAVA_OPTIONS="-Xss512m${JDK_JAVA_OPTIONS:+ ${JDK_JAVA_OPTIONS}}"
fi
workers="${TLC_WORKERS:-auto}"
# The canonical TLC lane's per-run wall-clock cap (`TLC_RUN_CAP_SECS`,
# default 900 s, see `xtask machine-verify`): a run that hits it is killed and
# fails as INCOMPLETE, never as a pass.
cap_secs="${TLC_RUN_CAP_SECS:-900}"
if ! [[ "${cap_secs}" =~ ^[0-9]+$ ]] || (( cap_secs < 1 )); then
  echo "error: TLC_RUN_CAP_SECS must be whole seconds >= 1, got '${cap_secs}'" >&2
  exit 2
fi
cd "${spec_dir}"

run_tlc() {
  local name="$1" cfg="$2"
  local log="${work_dir}/${name}.log"
  local capped="${work_dir}/${name}.capped"
  set +e
  tlc -workers "${workers}" -metadir "${work_dir}/${name}-states" -config "${cfg}" "${@:3}" \
    live_delegation_steer_audit.tla > "${log}" 2>&1 &
  local tlc_pid=$!
  # The cap watchdog ends TLC (which execs java) when the cap elapses first,
  # and is itself ended, with its sleep, when TLC exits first.
  (
    trap 'kill "${sleep_pid}" 2>/dev/null; exit 0' TERM
    sleep "${cap_secs}" &
    sleep_pid=$!
    wait "${sleep_pid}"
    : > "${capped}"
    kill -TERM "${tlc_pid}" 2>/dev/null
  ) &
  local watchdog_pid=$!
  wait "${tlc_pid}"
  local status=$?
  kill -TERM "${watchdog_pid}" 2>/dev/null
  wait "${watchdog_pid}" 2>/dev/null
  set -e
  if [[ -e "${capped}" ]]; then
    echo "TLC INCOMPLETE for live_delegation_steer_audit (${name}): hit the ${cap_secs} s per-run cap and was killed; an unexhausted state space is not a pass" >&2
    exit 3
  fi
  grep -E 'states generated|distinct states|Invariant .* is violated|Error:|Model checking completed' "${log}" | sed "s/^/[${name}] /"
  echo "${status}" > "${work_dir}/${name}.status"
}

safety_cfg="${work_dir}/safety.cfg"
derive_cfg "${safety_cfg}" "  AuditSteerBelongsToTheDelegation
  AuditSettledSteerStaysSettled"
run_tlc safety "${safety_cfg}" "$@"
if [[ "$(cat "${work_dir}/safety.status")" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: live delegation steer audit safety check failed (bound ${max_steps})" >&2
  tail -40 "${work_dir}/safety.log" >&2
  exit 1
fi

for goal in DeliveredConfirmed NotDelivered MaterialConflict Missing; do
  cfg="${work_dir}/goal-${goal}.cfg"
  derive_cfg "${cfg}" "  NotGoal${goal}"
  run_tlc "goal-${goal}" "${cfg}" "$@"
  if ! grep -q "Invariant NotGoal${goal} is violated" "${work_dir}/goal-${goal}.log"; then
    echo "error: steer outcome ${goal} is not reachable within ${max_steps} steps" >&2
    tail -40 "${work_dir}/goal-${goal}.log" >&2
    exit 1
  fi
done
echo "live delegation steer audit passed at model_step_count <= ${max_steps}; every steer outcome reachable"
