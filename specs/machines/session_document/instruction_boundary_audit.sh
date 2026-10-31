#!/usr/bin/env bash
# TLC audit of the operator-instruction region of the generated
# SessionDocumentMachine, driven as the shell drives it and checked on the
# machine's own per-slot state: the slots in effect change only at a FreshRun
# start or a run-start fold and hold only Active slots (as the shell re-seeds
# them), an application reaches the target and coalesces every earlier write
# into one entry, an applied revoke holds until a later write, a mid-run
# fold, a write, a record, a rebind and a release leave the slots in effect
# alone, a session write never lowers a revision, a leading-prefix-only
# model applies only on a leading System prefix, a mob-owned target changes
# owner only when its own mob releases it, a release empties the target and
# the next fresh run start revokes what the released mob had in effect, a
# rebind starts from the right target, and the target stays within the
# active budget.
#
# usage: instruction_boundary_audit.sh [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant, invariant and the step constraint, unchanged) and adds
# the audit's invariants. It first checks that the generated model defines
# every instruction arm with the parameters the audit passes, so a renamed
# arm or field fails the run instead of a vacuous pass. It then checks the
# invariants (safety), and reruns once per reachability witness, with that
# witness's negation as an extra invariant, requiring TLC to report exactly
# that violation. With --mutants it seeds ten defects into a copy of the
# generated model and requires TLC to refuse each with the invariant it
# targets, checked as the run's only invariant:
#   - apply on ResumeRun (the FreshRun guard widened to any run);
#   - keep a revoked slot in effect at an application (the slots in effect
#     take the target's whole effect map);
#   - apply a mid-run fold from the target (the run-start guard dropped);
#   - accept a stale session write (the higher-revision guard dropped);
#   - skip the leading-prefix check (any lowering but Unavailable applies);
#   - accept another mob's record (the owning-mob guard dropped);
#   - let a rebind take a mob-owned target (the attach's ownership guard
#     dropped, alone);
#   - record a session write over the active budget (its budget guard
#     dropped);
#   - keep a released mob's instructions at a run start (the no-change arm
#     taken whatever is pending);
#   - keep the target on a release (its four slot maps left as they were).
# No run uses -continue. TLC checks the invariants of every new state and of
# every successor past the step constraint, each time it generates one, and
# under -continue prints a whole trace for each violation but names only the
# first invariant a state violates: one run of the five witnesses had about
# 200,000 violations to print and did not end within the per-run cap, and
# could never name MobOwnedEffective, which implies Applied. Without
# -continue a run stops at its first violation: a witness run at the first
# state it finds that reaches the witness, and a mutant run, whose only
# invariant is its target, exactly when some reachable state of the mutant
# breaks the target, whatever else that state breaks.
# Each TLC run is held to the canonical lane's per-run cap
# (`TLC_RUN_CAP_SECS`, default 900 s, see tlc_run_cap.sh); a run that hits
# it fails closed as TLC INCOMPLETE (exit 3).
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
    "RecordInstructionTargetSlotLimitExceeded": record,
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
    "RebindInstructionOwnerUnowned": rebind,
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
    "session_document_instruction_active_slots(",
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
  AuditEffectiveHoldsOnlyActiveSlots
  AuditApplicationReachesTheTarget
  AuditOneEntryPerRunStart
  AuditAppliedRevokeStaysAbsent
  AuditMidRunFoldKeepsEffective
  AuditSessionWriteNeverLowersRevision
  AuditPrefixOnlyAppliesInLeadingPrefix
  AuditMobTargetBoundUntilReleased
  AuditReleaseEmptiesTheTarget
  AuditUnownedRunStartRevokesWhatIsInEffect
  AuditRebindStartsFromTheRightTarget
  AuditTargetWithinBudget"

witnesses="Applied RevokeApplied MobOwnedEffective ReboundAfterRelease ReleasedInstructionsRevoked"

# usage: write_cfg <file> <extra invariant lines>
write_cfg() {
  local cfg="$1" extra="$2"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "INVARIANTS" "INVARIANTS
${audit_invariants}${extra}" "${cfg}"
  printf 'CHECK_DEADLOCK FALSE\n' >> "${cfg}"
}

# usage: write_target_cfg <file> <invariant>; the generated constants and
# step constraint, unchanged, with <invariant> as the only invariant.
write_target_cfg() {
  local cfg="$1" target="$2"
  write_cfg "${cfg}" ""
  CFG="${cfg}" TARGET="${target}" python3 - <<'PY'
import os, re
path, target = os.environ["CFG"], os.environ["TARGET"]
lines = open(path).read().split("\n")
start = lines.index("INVARIANTS") + 1
end = start
while end < len(lines) and lines[end].startswith("  "):
    end += 1
assert end < len(lines) and re.fullmatch(r"[A-Z_]+", lines[end]), f"no section follows the invariants in {path}"
lines[start:end] = ["  " + target]
open(path, "w").write("\n".join(lines))
PY
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

# One run per witness: the safety run showed that every other invariant
# holds, so the run stops at the first state it finds that reaches the
# witness.
for witness in ${witnesses}; do
  witness_cfg="${work_dir}/witness-${witness}.cfg"
  write_cfg "${witness_cfg}" "
  NotAuditWitness${witness}"
  run_tlc "witness-${witness}" "${witness_cfg}" "${spec_dir}"
  witness_log="${work_dir}/witness-${witness}.log"
  if ! grep -q "Error: Invariant NotAuditWitness${witness} is violated" "${witness_log}"; then
    cat "${witness_log}"
    echo "error: witness ${witness} is not reached (tlc exit ${tlc_status})" >&2
    exit 1
  fi
  depth="$(grep -cE '^State [0-9]+:' "${witness_log}" || true)"
  echo "witness ${witness} reached in ${depth} states"
done

if [[ "${run_mutants}" == "true" ]]; then
  # usage: mutate <name> <target invariant> <arm> <anchor> <new line> [<anchor> <new line>...]
  # Copies the specs, replaces the one line of <arm> that contains each
  # <anchor> with its <new line>, runs the target as the only invariant, and
  # requires TLC to report it violated.
  mutate() {
    local name="$1" target="$2" arm="$3"
    shift 3
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}/model.tla" "${spec_dir}/instruction_boundary_audit.tla" "${dir}/"
    MODEL="${dir}/model.tla" ARM="${arm}" python3 - "$@" <<'PY'
import os, sys
path, arm = os.environ["MODEL"], os.environ["ARM"]
edits = sys.argv[1:]
assert edits and len(edits) % 2 == 0, "edits come in (anchor, new line) pairs"
text = open(path).read()
start = text.index("\n" + arm + "(") + 1
end = text.index("\n\n", start)
lines = text[start:end].split("\n")
for anchor, line in zip(edits[0::2], edits[1::2]):
    hits = [i for i, current in enumerate(lines) if anchor in current]
    assert len(hits) == 1, f"expected one line with {anchor!r} in {arm}, found {len(hits)}"
    lines[hits[0]] = line
text = text[:start] + "\n".join(lines) + text[end:]
open(path, "w").write(text)
PY
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_target_cfg "${cfg}" "${target}"
    run_tlc "mutant-${name}" "${cfg}" "${dir}"
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -q "Error: Invariant ${target} is violated" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by ${target} (tlc exit ${tlc_status})" >&2
      exit 1
    fi
    local depth
    depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
    echo "mutant ${name} refused by ${target} in ${depth} states"
  }

  mutate apply_on_resume_run AuditEffectiveChangesOnlyAtRunStart \
    ResolveRunStartInstructionsApply 'run_start = "FreshRun"' \
    '    /\ (run_start # "NoRun")'
  mutate revoked_slot_kept_in_effect AuditEffectiveHoldsOnlyActiveSlots \
    ResolveRunStartInstructionsApply "session_instruction_effective_effect' =" \
    "    /\\ session_instruction_effective_effect' = MapSet(session_instruction_effective_effect, session_id, session_instruction_target_effect[session_id])"
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
    RebindInstructionOwner '((session_id \in DOMAIN session_instruction_owner) /\ ((session_instruction_owner)[session_id] = "Session"))' \
    '    /\ TRUE'
  mutate budget_exceeded_write_accepted AuditTargetWithinBudget \
    SetSessionInstructionNew '1048576 >= ' \
    '    /\ TRUE'
  mutate unowned_run_start_keeps_released_instructions AuditUnownedRunStartRevokesWhatIsInEffect \
    ResolveRunStartInstructionsNoChange 'instruction_change_pending(' \
    '    /\ TRUE'
  # The release leaves the whole target as it was: its four slot maps stay
  # in lockstep, so the audit's own operators still read it.
  mutate release_keeps_the_target AuditReleaseEmptiesTheTarget \
    ReleaseInstructionTarget \
    "session_instruction_target_revision' =" \
    "    /\\ session_instruction_target_revision' = session_instruction_target_revision" \
    "session_instruction_target_effect' =" \
    "    /\\ session_instruction_target_effect' = session_instruction_target_effect" \
    "session_instruction_target_digest' =" \
    "    /\\ session_instruction_target_digest' = session_instruction_target_digest" \
    "session_instruction_target_bytes' =" \
    "    /\\ session_instruction_target_bytes' = session_instruction_target_bytes"
fi
echo "instruction boundary audit passed"
