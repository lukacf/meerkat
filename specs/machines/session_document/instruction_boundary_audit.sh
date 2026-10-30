#!/usr/bin/env bash
# TLC audit of the operator-instruction region of the generated
# SessionDocumentMachine, driven as the shell drives it and checked on the
# machine's own per-slot state: the slots in effect change only at a FreshRun
# start or a run-start fold, an application reaches the target and coalesces
# every earlier write into one entry, an applied revoke holds until a later
# write, a mid-run fold, a write, a record, a rebind and a release leave the
# slots in effect alone, a session write never lowers a revision, a
# leading-prefix-only model applies only on a leading System prefix, an
# unowned target applies nothing, a mob-owned target changes owner only when
# its own mob releases it, a rebind starts from what is in effect, and the
# target stays within the active budget.
#
# usage: instruction_boundary_audit.sh [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant, invariant and the step constraint, unchanged) and adds
# the audit's invariants. It first checks that the generated model defines
# every instruction arm with the parameters the audit passes, so a renamed
# arm or field fails the run instead of a vacuous pass. It then checks the
# invariants (safety), and reruns with the reachability witnesses' negations
# as extra invariants under -continue, requiring TLC to report each of them
# violated. With --mutants it seeds seven defects into a copy of the
# generated model and requires TLC to refuse each with the invariant it
# targets:
#   - apply on ResumeRun (the FreshRun guard widened to any run);
#   - apply a mid-run fold from the target (the run-start guard dropped);
#   - accept a stale session write (the higher-revision guard dropped);
#   - skip the leading-prefix check (any lowering but Unavailable applies);
#   - accept another mob's record (the owning-mob guard dropped);
#   - let a rebind take a mob-owned target (the rebind guard dropped);
#   - record a session write over the active budget (its budget guard
#     dropped).
# An anchor this script expects but cannot find fails the run.
set -euo pipefail

run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/instruction-boundary-audit.XXXXXX")"
# An interrupted or killed run ends its TLC child too (tlc_run_cap.sh).
trap 'if declare -F tlc_run_cap_reap >/dev/null; then tlc_run_cap_reap; fi; rm -rf "${work_dir}"' EXIT

# The premise: the generated model defines each instruction arm with the
# parameters, in the order, the audit passes them.
MODEL="${spec_dir}/model.tla" python3 - <<'PY'
import os, re
text = open(os.environ["MODEL"]).read()
recover = "session_id, owner, owner_mob, target_generation, entry_sequence"
recover_target = "session_id, revision, effect, digest, bytes"
recover_effective = "session_id, revision, effect, digest, bytes, generation, last_entry_sequence"
record = "session_id, mob_id, generation, revision, effect, digest, bytes"
write = "session_id, slot, revision, effect, digest, body_bytes"
rebind = "session_id, owner, mob_id"
release = "session_id, mob_id"
resolve = "session_id, run_start, lowering, transcript_is_system_prefix"
fold = "session_id, at_run_start, lowering"
arms = {
    "RecoverSessionInstructionState": recover,
    "RecoverSessionInstructionTarget": recover_target,
    "RecoverSessionInstructionEffective": recover_effective,
    "RecordInstructionTargetAdvance": record,
    "RecordInstructionTargetActiveBudgetExceeded": record,
    "RecordInstructionTargetAlreadyRecorded": record,
    "RecordInstructionTargetOwnedBySession": record,
    "RecordInstructionTargetOwnedByOtherMob": record,
    "SetSessionInstructionNew": write,
    "SetSessionInstructionAdvance": write,
    "SetSessionInstructionActiveBudgetExceeded": write,
    "SetSessionInstructionDuplicate": write,
    "SetSessionInstructionStale": write,
    "SetSessionInstructionConflict": write,
    "SetSessionInstructionOwnedByMob": write,
    "SetSessionInstructionSlotLimitExceeded": write,
    "SetSessionInstructionUnknownSlot": write,
    "RebindInstructionOwner": rebind,
    "ReleaseInstructionTarget": release,
    "ReleaseInstructionTargetAlreadyUnowned": release,
    "ReleaseInstructionTargetOwnedByOtherMob": release,
    "ReleaseInstructionTargetOwnedBySession": release,
    "ResolveRunStartInstructionsApply": resolve,
    "ResolveRunStartInstructionsNoChange": resolve,
    "ResolveRunStartInstructionsDeferredLeadingPrefix": resolve,
    "ResolveRunStartInstructionsDeferredUnavailable": resolve,
    "AuthorizeInstructionFoldAtRunStart": fold,
    "AuthorizeInstructionFoldEffective": fold,
}
for arm, params in arms.items():
    heads = re.findall(r"(?m)^" + arm + r"\(([^)]*)\) ==$", text)
    assert heads == [params], f"expected {arm}({params}) in model.tla, found {heads}"
for name in (
    "InstructionRunStartValues",
    "InstructionLoweringValues",
    "instruction_change_pending(",
    "session_document_instruction_active_bytes(",
):
    assert name in text, f"model.tla declares no {name}"
print("premise holds: the generated model defines every instruction arm the audit drives")
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

audit_invariants="  AuditEffectiveChangesOnlyAtRunStart
  AuditApplicationReachesTheTarget
  AuditOneEntryPerRunStart
  AuditAppliedRevokeStaysAbsent
  AuditMidRunFoldKeepsEffective
  AuditSessionWriteNeverLowersRevision
  AuditPrefixOnlyAppliesInLeadingPrefix
  AuditUnownedAppliesNothing
  AuditMobTargetBoundUntilReleased
  AuditRebindStartsFromWhatIsInEffect
  AuditTargetWithinBudget"

witnesses="NotAuditWitnessApplied NotAuditWitnessRevokeApplied NotAuditWitnessMobOwnedEffective NotAuditWitnessReboundAfterRelease"

# usage: write_cfg <file> <extra invariant lines>
write_cfg() {
  local cfg="$1" extra="$2"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "INVARIANTS" "INVARIANTS
${audit_invariants}${extra}" "${cfg}"
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
  tlc_run_capped instruction_boundary_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" "${extra_tlc_args[@]}" "${dir}/instruction_boundary_audit.tla" \
    || tlc_status=$?
}

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" ""
run_tlc safety "${safety_cfg}" "${spec_dir}"
cat "${work_dir}/safety.log"
if [[ "${tlc_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: instruction boundary audit failed (tlc exit ${tlc_status})" >&2
  exit 1
fi

witness_lines=""
for witness in ${witnesses}; do
  witness_lines+="
  ${witness}"
done
witness_cfg="${work_dir}/witnesses.cfg"
write_cfg "${witness_cfg}" "${witness_lines}"
run_tlc witnesses "${witness_cfg}" "${spec_dir}" -continue
for witness in ${witnesses}; do
  if ! grep -q "Error: Invariant ${witness} is violated" "${work_dir}/witnesses.log"; then
    cat "${work_dir}/witnesses.log"
    echo "error: witness ${witness#Not} is not reached (tlc exit ${tlc_status})" >&2
    exit 1
  fi
  echo "witness ${witness#Not} reached"
done

if [[ "${run_mutants}" == "true" ]]; then
  # usage: mutate <name> <target invariant> <arm> <anchor> <new line>
  # Copies the specs, replaces the one line of <arm> that contains <anchor>
  # with <new line>, runs every invariant with -continue, and requires the
  # target among the invariants TLC reports violated.
  mutate() {
    local name="$1" target="$2" arm="$3" anchor="$4" line="$5"
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}/model.tla" "${spec_dir}/instruction_boundary_audit.tla" "${dir}/"
    MODEL="${dir}/model.tla" ARM="${arm}" ANCHOR="${anchor}" LINE="${line}" python3 - <<'PY'
import os
path = os.environ["MODEL"]
arm, anchor, line = os.environ["ARM"], os.environ["ANCHOR"], os.environ["LINE"]
text = open(path).read()
start = text.index("\n" + arm + "(") + 1
end = text.index("\n\n", start)
lines = text[start:end].split("\n")
hits = [i for i, current in enumerate(lines) if anchor in current]
assert len(hits) == 1, f"expected one line with {anchor!r} in {arm}, found {len(hits)}"
lines[hits[0]] = line
text = text[:start] + "\n".join(lines) + text[end:]
open(path, "w").write(text)
PY
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${cfg}" ""
    run_tlc "mutant-${name}" "${cfg}" "${dir}" -continue
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -q "Error: Invariant ${target} is violated" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by ${target} (tlc exit ${tlc_status})" >&2
      exit 1
    fi
    local broken
    broken="$(grep -oE 'Error: Invariant [A-Za-z_]+ is violated' "${log}" \
      | sed -E 's/Error: Invariant ([A-Za-z_]+) is violated/\1/' | sort -u | tr '\n' ' ')"
    echo "mutant ${name} refused by ${target}; breaks: ${broken}"
  }

  mutate apply_on_resume_run AuditEffectiveChangesOnlyAtRunStart \
    ResolveRunStartInstructionsApply 'run_start = "FreshRun"' \
    '    /\ (run_start # "NoRun")'
  mutate mid_run_fold_applies_target AuditMidRunFoldKeepsEffective \
    AuthorizeInstructionFoldAtRunStart 'at_run_start = TRUE' \
    '    /\ TRUE'
  mutate stale_session_write_accepted AuditSessionWriteNeverLowersRevision \
    SetSessionInstructionAdvance '(revision > ' \
    '    /\ TRUE'
  mutate leading_prefix_check_skipped AuditPrefixOnlyAppliesInLeadingPrefix \
    ResolveRunStartInstructionsApply 'instruction_entry_lowers(' \
    '    /\ (lowering # "Unavailable")'
  mutate foreign_mob_record_accepted AuditMobTargetBoundUntilReleased \
    RecordInstructionTargetAdvance '= mob_id)' \
    '    /\ TRUE'
  mutate rebind_takes_a_mob_target AuditMobTargetBoundUntilReleased \
    RebindInstructionOwner 'Cardinality(DOMAIN' \
    '    /\ TRUE'
  mutate budget_exceeded_write_accepted AuditTargetWithinBudget \
    SetSessionInstructionNew '1048576 >= ' \
    '    /\ TRUE'
fi
echo "instruction boundary audit passed"
