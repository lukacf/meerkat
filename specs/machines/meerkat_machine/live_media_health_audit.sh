#!/usr/bin/env bash
# Bounded TLC audit of the live media health edges.
#
# usage: live_media_health_audit.sh <max-steps> [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), adds the audit's finite model
# values, its invariants, its step bound and its deterministic-prefix
# constraint, then:
#   1. checks every invariant over the audit's state space (must pass), and
#   2. proves each judgement reachable: for each goal it checks the goal's
#      negation and requires TLC to report that invariant violated, and
#   3. proves each new transition fires: for each Never* action property it
#      requires TLC to report that property violated.
# It prints the safety run's distinct states and search depth, and each goal's
# counterexample length (a real multi-step witness past the 5-step prefix).
# An anchor this script expects but cannot find fails the run instead of
# silently narrowing the check. Each TLC run is held to the canonical lane's
# per-run cap (`TLC_RUN_CAP_SECS`, default 900 s, see tlc_run_cap.sh); a run
# that hits it fails as INCOMPLETE (exit 3).
set -euo pipefail

max_steps="${1:?usage: live_media_health_audit.sh <max-steps> [extra tlc args...]}"
shift
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 12 )); then
  echo "error: max-steps must be an integer >= 12 (the deterministic prefix is 5 steps; the exhausted budget needs 7 more)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/live-media-health-audit.XXXXXX")"
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

# $1: output cfg; $2: invariant block appended after the generated ones;
# $3: optional action properties.
derive_cfg() {
  local cfg="$1" extra_invariants="$2" properties="${3:-}"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"session_1"}' "${cfg}"
  replace_exact_line "  AgentRuntimeIdValues = {}" '  AgentRuntimeIdValues = {"runtime_1"}' "${cfg}"
  replace_exact_line "  SessionLlmIdentityValues = {}" '  SessionLlmIdentityValues = {"identity_1"}' "${cfg}"
  replace_exact_line "  RunIdValues = {}" '  RunIdValues = {"run_1"}' "${cfg}"
  replace_exact_line "  StringValues = {}" '  StringValues = {"session_1", "channel_1", "channel_2", "output_1", "output_2"}' "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}" "${cfg}"
  replace_exact_line "INVARIANTS" "INVARIANTS
${extra_invariants}" "${cfg}"
  replace_exact_line "  CiStateConstraint" "  AuditStateConstraint" "${cfg}"
  if [[ -n "${properties}" ]]; then
    printf 'PROPERTIES\n%s\n' "${properties}" >> "${cfg}"
  fi
  printf 'CHECK_DEADLOCK FALSE\n' >> "${cfg}"
}

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

run_tlc() {
  local name="$1" cfg="$2"
  local log="${work_dir}/${name}.log"
  local status=0
  # Goal runs end in a violation by design: -noGenerateSpecTE keeps TLC from
  # writing trace-explorer specs next to the model.
  tlc_run_capped live_media_health_audit "${name}" "${log}" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "${@:3}" live_media_health_audit.tla || status=$?
  grep -E 'states generated|distinct states|depth of the complete state graph|Invariant .* is violated|Temporal properties were violated|Action property .* is violated|Error:|Model checking completed' "${log}" | sed "s/^/[${name}] /"
  echo "${status}" > "${work_dir}/${name}.status"
}

safety_cfg="${work_dir}/safety.cfg"
derive_cfg "${safety_cfg}" "  AuditAtMostOneRecommendedReopen
  AuditVerdictsFollowTheBudget" "  AuditJudgedOnce
  AuditFirstOutputOnly"
run_tlc safety "${safety_cfg}" "$@"
if [[ "$(cat "${work_dir}/safety.status")" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: live media health audit safety check failed (bound ${max_steps})" >&2
  tail -40 "${work_dir}/safety.log" >&2
  exit 1
fi

for goal in Audible SilentReopen SilentExhausted FaultedChannelReportsClosed; do
  cfg="${work_dir}/goal-${goal}.cfg"
  derive_cfg "${cfg}" "  NotGoal${goal}"
  run_tlc "goal-${goal}" "${cfg}" "$@"
  if ! grep -q "Invariant NotGoal${goal} is violated" "${work_dir}/goal-${goal}.log"; then
    echo "error: media health judgement ${goal} is not reachable within ${max_steps} steps" >&2
    tail -40 "${work_dir}/goal-${goal}.log" >&2
    exit 1
  fi
  states="$(grep -cE '^State [0-9]+:' "${work_dir}/goal-${goal}.log" || true)"
  echo "[goal-${goal}] counterexample length ${states} states"
done
for edge in RequestAttached RequestRunning AudibleAttached AudibleRunning \
  SilentReopenAttached SilentReopenRunning SilentExhaustedAttached SilentExhaustedRunning; do
  cfg="${work_dir}/fires-${edge}.cfg"
  derive_cfg "${cfg}" "" "  Never${edge}"
  run_tlc "fires-${edge}" "${cfg}" "$@"
  if ! grep -q "Action property Never${edge} is violated" "${work_dir}/fires-${edge}.log"; then
    echo "error: media health transition ${edge} never fires within ${max_steps} steps" >&2
    tail -40 "${work_dir}/fires-${edge}.log" >&2
    exit 1
  fi
  states="$(grep -cE '^State [0-9]+:' "${work_dir}/fires-${edge}.log" || true)"
  echo "[fires-${edge}] witness length ${states} states"
done
echo "live media health audit passed at model_step_count <= ${max_steps}; every judgement reachable; every transition fires"
