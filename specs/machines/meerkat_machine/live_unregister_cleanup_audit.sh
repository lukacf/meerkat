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
replace_exact_line "  StringValues = {}" '  StringValues = {"", "channel_a", "channel_b", "profile_1", "pending_a", "owner_1", "ready_1", "activation_a", "lease_1", "run_1", "input_1", "stopped", "append_1", "digest_1", "commit_1"}' "${audit_cfg}"
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
# run_tlc_dump <name> <cfg> <dump-base> -> TLC also writes <dump-base>.dot
run_tlc_dump() {
  local name="$1" cfg="$2" dump="$3"
  local log="${work_dir}/${name}.log"
  tlc_status=0
  tlc_run_capped live_unregister_cleanup_audit "${name}" "${log}" \
    -workers 1 -metadir "${work_dir}/${name}-states" -noGenerateSpecTE -dump dot "${dump}" -config "${cfg}" "${extra_tlc_args[@]}" \
    live_unregister_cleanup_audit.tla || tlc_status=$?
  if [[ "${tlc_status}" != "0" ]] || [[ ! -s "${dump}.dot" ]]; then
    cat "${log}" >&2
    echo "error: TLC state-graph dump for ${name} failed (tlc exit ${tlc_status})" >&2
    exit 1
  fi
}
extra_tlc_args=("$@")

# AUDIT_STARTS narrows the run to some start states (debugging only; the lane
# runs them all).
for start in ${AUDIT_STARTS:-admitted staged bound running retired closing-idle closing-attached closing-running closing-retired closing-stopped closing-retired-recovery closing-stopped-recovery}; do
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
  if [[ "${start}" == closing-*-recovery ]]; then
    rec_cfg="${work_dir}/${start}-recovery.cfg"
    cp "${start_cfg}" "${rec_cfg}"
    printf 'INVARIANT\n  AuditNeverCancelsRecovery\n' >> "${rec_cfg}"
    run_tlc "AuditNeverCancelsRecovery-${start}" "${rec_cfg}"
    if ! grep -q "AuditNeverCancelsRecovery is violated" "${work_dir}/AuditNeverCancelsRecovery-${start}.log"; then
      cat "${work_dir}/AuditNeverCancelsRecovery-${start}.log" >&2
      echo "error: the closed channel's forward recovery is never settled from ${start} within bound ${max_steps}" >&2
      exit 1
    fi
  fi
  # Per-state no wedge (close-first starts): every reachable state must still
  # reach a completed unregister. TLC dumps the explored state graph and the
  # check walks it backwards from the unregistered states. States within the
  # finishing margin of the step bound are excluded: their successors are cut
  # off by the bound, not by the model.
  if [[ "${start}" == closing-* ]]; then
    dump="${work_dir}/${start}-graph"
    run_tlc_dump "graph-${start}" "${start_cfg}" "${dump}"
    python3 - "${dump}.dot" "${max_steps}" "${start}" <<'PY'
import re, sys
from collections import defaultdict
path, bound, start = sys.argv[1], int(sys.argv[2]), sys.argv[3]
prefix = {"closing-idle": 4, "closing-attached": 8, "closing-running": 9, "closing-retired": 10,
          "closing-stopped": 9, "closing-retired-recovery": 15, "closing-stopped-recovery": 14}[start]
limit = prefix + bound - 2
margin = 7
node_re = re.compile(r'^(-?\d+) \[label="(.*)"')
edge_re = re.compile(r'^(-?\d+) -> (-?\d+)')
steps, done, succ = {}, set(), defaultdict(set)
with open(path) as f:
    for line in f:
        m = edge_re.match(line)
        if m:
            succ[m.group(1)].add(m.group(2))
            continue
        m = node_re.match(line)
        if m:
            node, label = m.group(1), m.group(2)
            sm = re.search(r'\\n/\\\\ model_step_count = (\d+)', label)
            steps[node] = int(sm.group(1)) if sm else 0
            if re.search(r'\\n/\\\\ session_id = \[tag \|-> \\"none\\"', label):
                done.add(node)
pred = defaultdict(set)
for a, bs in succ.items():
    for b in bs:
        pred[b].add(a)
reach, todo = set(done), list(done)
while todo:
    n = todo.pop()
    for p in pred[n]:
        if p not in reach:
            reach.add(p); todo.append(p)
checked = [n for n in steps if steps[n] >= prefix and steps[n] <= limit - margin]
wedged = [n for n in checked if n not in reach]
print("[per-state %s] %d states, %d checked (step %d..%d), %d unregistered, %d wedged"
      % (start, len(steps), len(checked), prefix, limit - margin, len(done), len(wedged)))
if not done or wedged:
    sys.exit(1)
PY
    if [[ $? -ne 0 ]]; then
      echo "error: per-state no-wedge failed from ${start}: a reachable state cannot reach unregister" >&2
      exit 1
    fi
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
echo "live unregister cleanup audit passed at model_step_count <= ${max_steps} (unregister reachable from every start; per-state no-wedge holds from every close-first start)"
