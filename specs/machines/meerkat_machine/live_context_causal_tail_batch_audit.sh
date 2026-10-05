#!/usr/bin/env bash
# Bounded TLC audit of the live-context causal-tail batch edge: the queued
# heard-speech replays behind a late summary are delivered as one append over
# their exact cursor run, and the resolve edges settle it like a single row.
#
# usage: live_context_causal_tail_batch_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's finite
# model values, its two action properties, its step bound and its
# deterministic-prefix constraint. It runs TLC once with every generated
# invariant (safety), then once per goal with that goal as an extra property,
# and requires TLC to report each goal violated, so the properties are not
# vacuously true. With --mutants it then seeds four defects into a copy of the
# generated model (the batch edge drops the exact-range length check; the
# batch edge drops the disposition check; the resolve edges drop the pending
# next-cursor match; the enqueue edge drops the in-flight range guard) and
# requires the safety run to refuse each. An anchor this script expects but cannot find fails the run.
set -euo pipefail

usage="usage: live_context_causal_tail_batch_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 24 )); then
  echo "error: max-steps must be an integer >= 24 (the deepest goal needs 24 steps)" >&2
  exit 2
fi
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/live-context-causal-tail-batch-audit.XXXXXX")"
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
replace_exact_line "  StringValues = {}" '  StringValues = {"", "channel_a", "profile_1", "pending_a", "owner_1", "ready_1", "activation_a", "lease_1", "digest_s", "append_boot", "append_2", "append_3", "append_4", "digest_2", "digest_3", "digest_4", "commit_2", "commit_3", "commit_4", "obs_1", "obs_2"}' "${audit_cfg}"
replace_exact_line "INVARIANTS" "PROPERTIES
  AuditPendingRunCoversOnlyHeardSpeechReplays
  AuditPendingRunSkipsNoQueuedRow
  AuditResolveIsPinned
  AuditNewPendingStartsAtCursor
  AuditNoCursorDeliveredTwice
INVARIANTS" "${audit_cfg}"
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
  tlc_run_capped live_context_causal_tail_batch_audit "${name}" "${log}" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE -config "${cfg}" "${extra_tlc_args[@]}" \
    live_context_causal_tail_batch_audit.tla || tlc_status=$?
  grep -E 'states generated|distinct states|is violated|Error:' "${log}" | sed "s/^/[${name}] /" || true
}
extra_tlc_args=("$@")

run_tlc safety "${audit_cfg}"
if [[ "${tlc_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  cat "${work_dir}/safety.log" >&2
  echo "error: live-context causal-tail batch audit safety run failed (tlc exit ${tlc_status}, bound ${max_steps})" >&2
  exit 1
fi

# Each goal must be reachable within the bound: TLC has to report it violated.
for goal in \
  "INVARIANT AuditNeverBatchAuthorized" \
  "INVARIANT AuditNeverBatchDelivered" \
  "INVARIANT AuditNeverBatchRejected" \
  "INVARIANT AuditNeverRedeliveredAfterReject"; do
  kind="${goal%% *}"
  name="${goal#* }"
  goal_cfg="${work_dir}/${name}.cfg"
  cp "${audit_cfg}" "${goal_cfg}"
  printf '%s\n  %s\n' "${kind}" "${name}" >> "${goal_cfg}"
  run_tlc "${name}" "${goal_cfg}"
  if ! grep -q "${name} is violated" "${work_dir}/${name}.log"; then
    cat "${work_dir}/${name}.log" >&2
    echo "error: live-context causal-tail batch audit goal ${name} is not reached within bound ${max_steps}" >&2
    exit 1
  fi
done

if [[ "${run_mutants}" == "true" ]]; then
  # usage: mutate <name> <property> <arms> <old> <new>: copy the specs,
  # replace the <old> anchor in each named Attached arm, and require the safety
  # run to report <property> violated.
  mutate() {
    local name="$1" property="$2" arms="$3" old="$4" new="$5"
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}"/*.tla "${spec_dir}/tlc_run_cap.sh" "${dir}/"
    MODEL="${dir}/model.tla" OLD="${old}" NEW="${new}" ARMS="${arms}" python3 - <<'PY'
import os
path = os.environ["MODEL"]
text = open(path).read()
for arm in os.environ["ARMS"].split():
    start = text.index("\n" + arm + "(") + 1
    end = text.index("\n\n", start)
    block = text[start:end]
    old = os.environ["OLD"]
    if old.startswith("line:"):
        # Replace the guard line(s) of the arm that mention the token.
        token = old[len("line:"):]
        lines = block.split("\n")
        hits = [i for i, line in enumerate(lines) if line.startswith("    /\\ ") and token in line]
        assert len(hits) == 1, f"expected one guard line with {token} in {arm}, found {len(hits)}"
        lines[hits[0]] = "    /\\ " + os.environ["NEW"]
        block = "\n".join(lines)
    else:
        count = block.count(old)
        assert count >= 1, f"expected the anchor in {arm}, found none"
        block = block.replace(old, os.environ["NEW"])
    text = text[:start] + block + text[end:]
open(path, "w").write(text)
PY
    (cd "${dir}" && run_tlc "mutant-${name}" "${audit_cfg}") || true
    if ! grep -qE "(property|Invariant) ${property} is violated" "${work_dir}/mutant-${name}.log"; then
      cat "${work_dir}/mutant-${name}.log" >&2
      echo "error: mutant ${name} was not refused by ${property}" >&2
      exit 1
    fi
    echo "mutant ${name} refused by ${property}"
  }
  batch_arm="AuthorizeLiveContextCausalTailBatchAttached"
  resolve_arms="ResolveLiveContextAppendDeliveredAttached ResolveLiveContextAppendRejectedAttached ResolveLiveContextAppendAmbiguousAttached ResolveLiveContextAppendInterruptedByCloseAttached"
  mutate batch_admits_a_gap AuditPendingRunSkipsNoQueuedRow "${batch_arm}" \
    '(Cardinality(tail_cursors) = (next_cursor - previous_cursor))' 'TRUE'
  mutate batch_admits_any_disposition AuditPendingRunCoversOnlyHeardSpeechReplays "${batch_arm}" \
    '= Some("ReassertCausalTail")) THEN TRUE ELSE' '= Some("ReassertCausalTail")) \/ TRUE THEN TRUE ELSE'
  mutate resolve_drops_pending_next_pin AuditResolveIsPinned "${resolve_arms}" \
    ' /\ ((IF (append_id \in DOMAIN live_context_pending_next_cursor_by_append) THEN Some((IF append_id \in DOMAIN live_context_pending_next_cursor_by_append THEN live_context_pending_next_cursor_by_append[append_id] ELSE 0)) ELSE None) = Some(next_cursor))' \
    ' /\ TRUE'
  # Without the in-flight guard a row of an in-flight batch is queued again;
  # delivering the batch then leaves it at or below the channel cursor.
  mutate enqueue_into_inflight_batch live_context_outbox_is_above_every_seed EnqueueLiveContextRowAttached \
    'line:live_context_pending_next_cursor_by_append' 'TRUE'
fi
echo "live-context causal-tail batch audit passed at model_step_count <= ${max_steps} (4 goals reached)"
