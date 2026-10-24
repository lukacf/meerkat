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
#
# The safety run admits the late Steer input as a Prompt. That stands for every
# content input kind: the admission writes the kind only into input_is_prompt,
# and nothing in the generated model that changes state reads that flag's
# value, so every kind reaches the Prompt run's states up to the flag. (A
# generated verdict classifier, an action that changes no state but the step
# count, may read it: this audit takes none, and the script fails if it names
# one.) This script checks both premises on the generated model before it
# runs TLC and fails if either stops holding. DURABLE_STEER_AUDIT_ALL_KINDS=1
# runs the safety run over every content kind instead (by hand; about twice
# the cost).
#
# It then runs TLC once per content kind with the late input narrowed to that
# kind and AuditLateNeverJoins as an extra invariant, and requires TLC to report
# it violated: the admission lets every kind join within the bound, so the
# exactly-once invariants are not vacuous for any of them.
#
# Last, it substitutes a small run contributor bound for the generated 256:
# under a bound of one the late input never joins (the run's batch input
# fills it) and TLC must reach it waiting for that refused join; under two
# it must join. Every such run keeps the audited run within the bound.
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
replace_exact_line "  InputIdValues = {}" '  InputIdValues = {"input_batch", "input_late"}' "${audit_cfg}"
replace_exact_line "  RunIdValues = {}" '  RunIdValues = {"runid_1"}' "${audit_cfg}"
replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1"}' "${audit_cfg}"
replace_exact_line "  StringValues = {}" '  StringValues = {"input_batch", "input_late", "alpha"}' "${audit_cfg}"
content_kinds=(Prompt PeerMessage PeerRequest ExternalEvent FlowStep)
if [[ "${DURABLE_STEER_AUDIT_ALL_KINDS:-0}" == "1" ]]; then
  safety_kinds="$(printf '"%s", ' "${content_kinds[@]}")"
  safety_kinds="${safety_kinds%, }"
else
  safety_kinds='"Prompt"'
fi
replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}
  AuditLateKinds = {${safety_kinds}}" "${audit_cfg}"
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

# The premises that let the Prompt safety run stand for every content kind.
# 1. The late admission's updates read the kind only to set input_is_prompt
#    (and the execution kind, which is "ContentTurn" for every content kind).
# 2. The generated model never reads input_is_prompt's values: it appears only
#    in its own updates, Init, the variable lists and the DOMAIN sizes of the
#    state constraints. Verdict classifiers are the one exception: an action
#    whose only primed updates keep its phase and count the step (such as the
#    ClassifyStopInputDisposition arms) changes no state, so a kind it reads
#    cannot reach a state the Prompt run misses, and this audit takes none of
#    them. Their bodies are left out of the scan; naming one fails the run.
model="${spec_dir}/model.tla"
if [[ "${DURABLE_STEER_AUDIT_ALL_KINDS:-0}" != "1" ]]; then
  scan_model="${work_dir}/model-without-verdicts.tla"
  MODEL="${model}" OUT="${scan_model}" AUDIT="${spec_dir}/durable_in_turn_steer_audit.tla" python3 - <<'PY'
import os, re
text = open(os.environ["MODEL"]).read()
audit = open(os.environ["AUDIT"]).read()
# Every top-level definition starts a block, one-line definitions included.
blocks = re.split(r"(?m)^(?=[A-Za-z_][A-Za-z0-9_]*(?:\([^)\n]*\))? ==)", text)
kept, verdicts = [], []
for block in blocks:
    lines = block.split("\n")
    head = re.fullmatch(r"([A-Za-z_][A-Za-z0-9_]*)\([^)]*\) ==", lines[0])
    guard = [m.group(1) for m in (re.fullmatch(r'    /\\ phase = "([A-Za-z]+)"', l) for l in lines) if m]
    primed = [l for l in lines if "' = " in l]
    verdict_only = (
        head is not None
        and "input_is_prompt" in block
        and len(guard) == 1
        and sorted(primed) == sorted([
            f'    /\\ phase\' = "{guard[0]}"',
            "    /\\ model_step_count' = model_step_count + 1",
        ])
        and re.search(r"(?m)^    /\\ UnchangedFrame_[0-9a-f]+$", block) is not None
    )
    if not verdict_only:
        kept.append(block)
        continue
    name = head.group(1)
    if re.search(r"\b" + re.escape(name) + r"\b", audit):
        raise SystemExit(f"error: this audit takes {name}, which reads input_is_prompt")
    verdicts.append(name)
open(os.environ["OUT"], "w").write("".join(kept))
print(f"input_is_prompt premise: {len(verdicts)} verdict-only actions read the flag, none taken by this audit")
PY
  admission_kind_updates="$(
    awk '/^ResolveAdmissionPlanRequestedSteerRunning\(/ { body = 1 } body && /^$/ { exit } body' "${model}" \
      | grep -E "^ +/\\\\ [a-z_]+' = " | grep -F 'input_kind' || true
  )"
  expected_admission_kind_updates="$(printf '%s\n' \
    "    /\\ input_runtime_execution_kind' = MapSet(input_runtime_execution_kind, input_id, (IF (input_kind = \"Continuation\") THEN \"ResumePending\" ELSE \"ContentTurn\"))" \
    "    /\\ input_is_prompt' = MapSet(input_is_prompt, input_id, (input_kind = \"Prompt\"))")"
  if [[ "${admission_kind_updates}" != "${expected_admission_kind_updates}" ]]; then
    printf '%s\n' "${admission_kind_updates}" >&2
    echo "error: ResolveAdmissionPlanRequestedSteerRunning now writes the input kind elsewhere; the Prompt safety run no longer stands for every content kind (run with DURABLE_STEER_AUDIT_ALL_KINDS=1 and widen the default)" >&2
    exit 1
  fi
  flag_reads="$(
    grep -n -F 'input_is_prompt' "${scan_model}" \
      | grep -v -E "^[0-9]+:(UnchangedFrame_|VARIABLES |vars == |[A-Za-z]+StateConstraint == )" \
      | grep -v -E "^[0-9]+: +/\\\\ input_is_prompt' = " \
      | grep -v -F -x "$(grep -n -F -x '    /\ input_is_prompt = [x \in {} |-> None]' "${scan_model}" || true)" \
      || true
  )"
  map_reads="$(grep -o -E 'Map[A-Za-z]*\(input_is_prompt,' "${scan_model}" | grep -v -E '^Map(Set|Remove)\(' || true)"
  if [[ -n "${flag_reads}" || -n "${map_reads}" ]] || grep -q -F 'input_is_prompt[' "${scan_model}"; then
    printf '%s\n' "${flag_reads}" "${map_reads}" >&2
    echo "error: the generated model now reads input_is_prompt; the Prompt safety run no longer stands for every content kind (run with DURABLE_STEER_AUDIT_ALL_KINDS=1 and widen the default)" >&2
    exit 1
  fi
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
  tlc_run_capped durable_in_turn_steer_audit "${name}" "${log}" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE -config "${cfg}" "${extra_tlc_args[@]}" \
    durable_in_turn_steer_audit.tla || tlc_status=$?
  grep -E 'states generated|distinct states|is violated|Error:' "${log}" | sed "s/^/[${name}] /" || true
}
extra_tlc_args=("$@")

run_tlc safety "${audit_cfg}"
if [[ "${tlc_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  cat "${work_dir}/safety.log" >&2
  echo "error: durable in-turn steer audit failed (tlc exit ${tlc_status}, bound ${max_steps})" >&2
  exit 1
fi

# Every content kind must reach a durable join within the bound: TLC has to
# report AuditLateNeverJoins violated with the late input narrowed to it.
for kind in "${content_kinds[@]}"; do
  name="join-${kind}"
  goal_cfg="${work_dir}/${name}.cfg"
  if ! FROM="  AuditLateKinds = {${safety_kinds}}" TO="  AuditLateKinds = {\"${kind}\"}" \
    awk '$0 == ENVIRON["FROM"] { print ENVIRON["TO"]; found = 1; next } { print } END { exit found ? 0 : 1 }' \
    "${audit_cfg}" > "${goal_cfg}"; then
    echo "error: the audit config lost its AuditLateKinds line" >&2
    exit 1
  fi
  printf 'INVARIANT\n  AuditLateNeverJoins\n' >> "${goal_cfg}"
  run_tlc "${name}" "${goal_cfg}"
  if ! grep -q "AuditLateNeverJoins is violated" "${work_dir}/${name}.log"; then
    cat "${work_dir}/${name}.log" >&2
    echo "error: durable in-turn steer audit: a late ${kind} steer never joins within bound ${max_steps}" >&2
    exit 1
  fi
done
# Run contributor bound. The generated guard caps a run's staged
# contributors at meerkat_machine_run_contributor_limit (256), which no bounded
# run reaches, so these runs substitute a small bound for it: under a bound of
# one the late input must never join and must reach the refused join, and
# under two it must join once, every run keeping the audited run within the
# bound in force.
# capacity_cfg <name> <bound operator> -> ${work_dir}/<name>.cfg
capacity_cfg() {
  local name="$1" bound="$2"
  local cfg="${work_dir}/${name}.cfg"
  cp "${audit_cfg}" "${cfg}"
  printf 'CONSTANT\n  meerkat_machine_run_contributor_limit <- %s\nINVARIANT\n  AuditRunContributorsWithinBound\n' \
    "${bound}" >> "${cfg}"
}

capacity_cfg capacity-one-safety AuditOneContributor
printf 'INVARIANT\n  AuditLateNeverJoins\n' >> "${work_dir}/capacity-one-safety.cfg"
run_tlc capacity-one-safety "${work_dir}/capacity-one-safety.cfg"
if [[ "${tlc_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/capacity-one-safety.log"; then
  cat "${work_dir}/capacity-one-safety.log" >&2
  echo "error: durable in-turn steer audit: under a contributor bound of one the late input joined or the run passed the bound (tlc exit ${tlc_status}, bound ${max_steps})" >&2
  exit 1
fi

capacity_cfg capacity-one-refusal AuditOneContributor
printf 'INVARIANT\n  AuditCapacityRefusalNeverReached\n' >> "${work_dir}/capacity-one-refusal.cfg"
run_tlc capacity-one-refusal "${work_dir}/capacity-one-refusal.cfg"
if ! grep -q "AuditCapacityRefusalNeverReached is violated" "${work_dir}/capacity-one-refusal.log"; then
  cat "${work_dir}/capacity-one-refusal.log" >&2
  echo "error: durable in-turn steer audit: under a contributor bound of one the late input never waits for a refused join within bound ${max_steps}" >&2
  exit 1
fi

capacity_cfg capacity-two-join AuditTwoContributors
printf 'INVARIANT\n  AuditLateNeverJoins\n' >> "${work_dir}/capacity-two-join.cfg"
run_tlc capacity-two-join "${work_dir}/capacity-two-join.cfg"
if ! grep -q "AuditLateNeverJoins is violated" "${work_dir}/capacity-two-join.log"; then
  cat "${work_dir}/capacity-two-join.log" >&2
  echo "error: durable in-turn steer audit: under a contributor bound of two the late input never joins within bound ${max_steps}" >&2
  exit 1
fi

echo "durable in-turn steer audit passed at model_step_count <= ${max_steps} (safety kinds {${safety_kinds}}; ${#content_kinds[@]} content kinds join; a contributor bound of one refuses the join, two takes it)"
