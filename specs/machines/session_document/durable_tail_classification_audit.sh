#!/usr/bin/env bash
# TLC audit of the generated SessionDocumentMachine durable-tail
# classification: the four ClassifyDurableTail* arms partition the whole
# observation product, a store-proven rewrite descendant classifies exactly
# like a strict descendant except where only the store binding of a strict
# continuation attributes a tail with no run rows, and a run cursor never
# upgrades an Ambiguous tail outside that one designed case.
#
# usage: durable_tail_classification_audit.sh [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged) and adds the audit's
# invariants. The audit checks the initial state alone, with ENABLED over
# every observation tuple; that stands for every reachable state only while
# the four arms are verdicts whose guards read no state but phase, so this
# script checks that premise on the generated model first and fails if it
# stops holding. It then checks the invariants (safety), and reruns with the
# reachability witness's negation as an extra invariant, requiring TLC to
# report exactly that violation. With --mutants it seeds seventeen defects
# into a copy of the generated model and requires TLC to refuse each with
# the invariant it targets; with -continue it records every invariant each
# defect breaks, and AuditReportGaps names the tuples the defect leaves with
# no arm or several. The defects:
#   - each relation conjunct back to strict only (Completed, Repairable);
#   - Ambiguous counting the rewrite relation as a foreign one;
#   - Repairable ignoring the attribution (any run-id count);
#   - each of Ambiguous's disjuncts dropped alone (foreign relation,
#     attribution, orphan results, messages after the terminal, Other stop,
#     dangling tool use, a disagreeing cursor, a cursor on EndTurn);
#   - a rewrite tail ending at ToolUse completing;
#   - EndTurn repairing and Absent completing;
#   - ToolUse turning Ambiguous;
#   - Continuable ignoring whether the bound inputs are bound;
#   - Repairable attributing a rewrite tail with no run rows and no cursor.
# An anchor this script expects but cannot find fails the run.
set -euo pipefail

run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/durable-tail-classification-audit.XXXXXX")"
# An interrupted or killed run ends its TLC child too (tlc_run_cap.sh).
trap 'if declare -F tlc_run_cap_reap >/dev/null; then tlc_run_cap_reap; fi; rm -rf "${work_dir}"' EXIT

# The premise: each classification arm keeps phase Ready, changes only the
# step count, and its guard reads no state variable but phase.
MODEL="${spec_dir}/model.tla" python3 - <<'PY'
import os, re
text = open(os.environ["MODEL"]).read()
declared = re.search(r"^VARIABLES (.*)$", text, re.M)
assert declared, "VARIABLES line not found in model.tla"
variables = {name.strip() for name in declared.group(1).split(",")}
for arm in (
    "ClassifyDurableTailCompleted",
    "ClassifyDurableTailRepairable",
    "ClassifyDurableTailContinuable",
    "ClassifyDurableTailAmbiguous",
):
    heads = re.findall(r"(?m)^" + arm + r"\(([^)]*)\) ==$", text)
    assert len(heads) == 1, f"expected one {arm} definition, found {len(heads)}"
    start = text.index("\n" + arm + "(") + 1
    end = text.index("\n\n", start)
    lines = text[start:end].split("\n")[1:]
    primed = sorted(line for line in lines if "'" in line)
    assert primed == sorted([
        "    /\\ phase' = \"Ready\"",
        "    /\\ model_step_count' = model_step_count + 1",
    ]), f"{arm} now changes state: {primed}"
    frames = [line for line in lines if re.fullmatch(r"    /\\ UnchangedFrame_[0-9a-f]+", line)]
    assert len(frames) == 1, f"{arm} has no single UNCHANGED frame"
    guards = [line for line in lines if "'" not in line and line not in frames]
    assert "    /\\ phase = \"Ready\"" in guards, f"{arm} is no longer guarded on phase Ready"
    for line in guards:
        if line == "    /\\ phase = \"Ready\"":
            continue
        bare = re.sub(r'"[^"]*"', '""', line)
        read = set(re.findall(r"[A-Za-z_][A-Za-z0-9_]*", bare)) & variables
        assert not read, f"{arm} now reads state {sorted(read)}: the initial state no longer stands for every state"
print("premise holds: the four classification arms are verdicts that read no state but phase")
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

audit_invariants="  AuditClassificationIsTotalAndDisjoint
  AuditRewriteClassifiesLikeStrict
  AuditRewriteUnattributedHolds
  AuditRewriteEndTurnCompletes
  AuditRewriteToolUseRepairs
  AuditAgreeingBoundCursorContinues
  AuditStrictNoRunRowsRepairs
  AuditDisagreeingCursorHolds
  AuditCursorOnEndTurnHolds
  AuditContinuableNeedsAgreeingBoundCursor
  AuditCursorNeverUpgradesAmbiguous"

# usage: write_cfg <file> <extra invariant lines>
write_cfg() {
  local cfg="$1" extra="$2"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "INVARIANTS" "INVARIANTS
${audit_invariants}${extra}" "${cfg}"
  # The audit takes no step: the initial state is the whole space.
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
extra_tlc_args=("$@")
# Each TLC run is held to the canonical lane's per-run cap (fails closed as
# TLC INCOMPLETE, exit 3).
# shellcheck source=../meerkat_machine/tlc_run_cap.sh
source "${spec_dir}/../meerkat_machine/tlc_run_cap.sh"

# usage: run_tlc <name> <cfg> <dir> [tlc args...]; leaves the log at
# ${work_dir}/<name>.log
run_tlc() {
  local name="$1" cfg="$2" dir="$3"
  shift 3
  tlc_status=0
  # Witness and mutant runs end in a violation by design: -noGenerateSpecTE
  # keeps TLC from writing trace-exploration files next to the specs.
  tlc_run_capped durable_tail_classification_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" "${extra_tlc_args[@]}" "${dir}/durable_tail_classification_audit.tla" \
    || tlc_status=$?
}

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" ""
run_tlc safety "${safety_cfg}" "${spec_dir}"
cat "${work_dir}/safety.log"
if [[ "${tlc_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: durable-tail classification audit failed (tlc exit ${tlc_status})" >&2
  exit 1
fi

witness_cfg="${work_dir}/witness-Ready.cfg"
write_cfg "${witness_cfg}" "
  NotAuditWitnessReady"
run_tlc witness-Ready "${witness_cfg}" "${spec_dir}"
if ! grep -q "Error: Invariant NotAuditWitnessReady is violated" "${work_dir}/witness-Ready.log"; then
  cat "${work_dir}/witness-Ready.log"
  echo "error: witness Ready is not reached (tlc exit ${tlc_status})" >&2
  exit 1
fi
echo "witness Ready reached"

if [[ "${run_mutants}" == "true" ]]; then
  # usage: mutate <name> <target invariant> <arm> <old> <new> [<arm> <old> <new>...]
  # Copies the specs, replaces exactly one <old> by <new> in each named arm,
  # runs every invariant with -continue and AuditReportGaps, and requires
  # the target among the invariants TLC reports violated.
  mutate() {
    local name="$1" target="$2"
    shift 2
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}/model.tla" "${spec_dir}/durable_tail_classification_audit.tla" "${dir}/"
    MODEL="${dir}/model.tla" python3 - "$@" <<'PY'
import os, sys
path = os.environ["MODEL"]
text = open(path).read()
edits = sys.argv[1:]
assert edits and len(edits) % 3 == 0, "edits come in (arm, old, new) triples"
for i in range(0, len(edits), 3):
    arm, old, new = edits[i:i + 3]
    start = text.index("\n" + arm + "(") + 1
    end = text.index("\n\n", start)
    block = text[start:end]
    count = block.count(old)
    assert count == 1, f"expected one {old!r} in {arm}, found {count}"
    text = text[:start] + block.replace(old, new) + text[end:]
open(path, "w").write(text)
PY
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${cfg}" "
  AuditReportGaps"
    run_tlc "mutant-${name}" "${cfg}" "${dir}" -continue
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -q "Error: Invariant ${target} is violated" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by ${target} (tlc exit ${tlc_status})" >&2
      exit 1
    fi
    local broken gaps
    broken="$(grep -oE 'Error: Invariant [A-Za-z]+ is violated' "${log}" \
      | sed -E 's/Error: Invariant ([A-Za-z]+) is violated/\1/' | sort -u | tr '\n' ' ')"
    # TLC pretty-prints a long PrintT value over several lines.
    gaps="$(LOG="${log}" python3 -c '
import os, re
text = open(os.environ["LOG"]).read()
found = re.findall(r"<<\s*\"tuples with.*?(?:>>\s*>>|\"none\">>)", text, re.S)
print("; ".join(" ".join(item.split()) for item in found))
')"
    echo "mutant ${name} refused by ${target}; breaks: ${broken}; ${gaps}"
  }

  C='ClassifyDurableTailCompleted'
  R='ClassifyDurableTailRepairable'
  K='ClassifyDurableTailContinuable'
  A='ClassifyDurableTailAmbiguous'
  attributed='(IF (run_id_cardinality = "SingleRunId") THEN TRUE ELSE'
  either='(IF (relation = "VerifiedStrictDescendant") THEN TRUE ELSE (relation = "VerifiedRewriteDescendant"))'
  strict_only='(relation = "VerifiedStrictDescendant")'
  foreign='((relation # "VerifiedStrictDescendant") /\ (relation # "VerifiedRewriteDescendant"))'
  repairable_stops='(IF (terminal_stop_reason = "ToolUse") THEN TRUE ELSE (terminal_stop_reason = "Absent"))'

  # C1: each relation conjunct back to strict only leaves the rewrite tail
  # with no arm; Ambiguous counting the rewrite relation as foreign overlaps.
  mutate completed_drops_rewrite AuditClassificationIsTotalAndDisjoint \
    "${C}" "${either}" "${strict_only}"
  mutate repairable_drops_rewrite AuditClassificationIsTotalAndDisjoint \
    "${R}" "${either}" "${strict_only}"
  mutate ambiguous_counts_rewrite_as_foreign AuditClassificationIsTotalAndDisjoint \
    "${A}" "${foreign}" '(relation # "VerifiedStrictDescendant")'
  # C3: Repairable ignores the attribution and admits any run-id count.
  mutate repairable_ignores_attribution AuditRewriteUnattributedHolds \
    "${R}" "${attributed}" '(IF TRUE THEN TRUE ELSE'
  # C1, one disjunct of the Ambiguous complement at a time: each leaves the
  # tuples where it alone refuses Completed and Repairable with no arm.
  mutate ambiguous_drops_foreign_relation AuditClassificationIsTotalAndDisjoint \
    "${A}" "${foreign}" 'FALSE'
  mutate ambiguous_drops_attribution AuditClassificationIsTotalAndDisjoint \
    "${A}" "${attributed}" '(IF TRUE THEN TRUE ELSE'
  mutate ambiguous_drops_disagreeing_cursor AuditDisagreeingCursorHolds \
    "${A}" '(run_cursor = "Disagrees")' 'FALSE'
  mutate ambiguous_drops_cursor_on_end_turn AuditCursorOnEndTurnHolds \
    "${A}" '((terminal_stop_reason = "EndTurn") /\ (run_cursor # "Absent"))' 'FALSE'
  mutate ambiguous_drops_orphan_results AuditClassificationIsTotalAndDisjoint \
    "${A}" '(orphan_tool_result_count # 0)' 'FALSE'
  mutate ambiguous_drops_messages_after_terminal AuditClassificationIsTotalAndDisjoint \
    "${A}" '(messages_after_terminal = TRUE)' 'FALSE'
  mutate ambiguous_drops_other_stop AuditClassificationIsTotalAndDisjoint \
    "${A}" '(terminal_stop_reason = "Other")' 'FALSE'
  mutate ambiguous_drops_dangling_tool_use AuditClassificationIsTotalAndDisjoint \
    "${A}" '(dangling_tool_use_count # 0)' 'FALSE'
  # C2 alone (the partition kept): a rewrite tail ending at ToolUse
  # completes while a strict one repairs.
  mutate rewrite_tool_use_completes AuditRewriteClassifiesLikeStrict \
    "${C}" "${either} /\\ (run_id_cardinality = \"SingleRunId\") /\\ (terminal_stop_reason = \"EndTurn\")" \
    '(IF (relation = "VerifiedStrictDescendant") THEN (terminal_stop_reason = "EndTurn") ELSE ((relation = "VerifiedRewriteDescendant") /\ (IF (terminal_stop_reason = "EndTurn") THEN TRUE ELSE (terminal_stop_reason = "ToolUse")))) /\ (run_id_cardinality = "SingleRunId")' \
    "${R}" "${repairable_stops}" \
    '(IF (terminal_stop_reason = "ToolUse") THEN (relation = "VerifiedStrictDescendant") ELSE (terminal_stop_reason = "Absent"))'
  # C4 alone (the partition kept): EndTurn repairs and Absent completes.
  mutate end_turn_repairs AuditRewriteEndTurnCompletes \
    "${C}" '(terminal_stop_reason = "EndTurn")' '(terminal_stop_reason = "Absent")' \
    "${R}" "${repairable_stops}" \
    '(IF (terminal_stop_reason = "ToolUse") THEN TRUE ELSE (terminal_stop_reason = "EndTurn"))'
  # Continuable ignores whether the cursor's inputs are still bound.
  mutate continuable_ignores_bound_inputs AuditContinuableNeedsAgreeingBoundCursor \
    "${K}" '(cursor_inputs_open = TRUE)' 'TRUE'
  # Repairable lets the store binding attribute a rewrite tail with no run
  # rows: a cursor-less rewrite tail is closed instead of held.
  mutate repairable_attributes_rewrite_without_cursor AuditRewriteUnattributedHolds \
    "${R}" '(IF (relation = "VerifiedStrictDescendant") THEN TRUE ELSE (run_cursor = "Agrees"))' 'TRUE'
  # The ToolUse pin alone (the partition kept): ToolUse turns Ambiguous.
  mutate tool_use_turns_ambiguous AuditRewriteToolUseRepairs \
    "${R}" "${repairable_stops}" '(terminal_stop_reason = "Absent")' \
    "${A}" '(terminal_stop_reason = "Other")' \
    '(IF (terminal_stop_reason = "Other") THEN TRUE ELSE (terminal_stop_reason = "ToolUse"))'
fi
echo "durable-tail classification audit passed over the whole observation product"
