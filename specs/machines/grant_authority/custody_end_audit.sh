#!/usr/bin/env bash
# Bounded TLC audit of the custody end of the generated
# GrantAuthorityMachine: the host's close (ReleaseCustody, into the terminal
# Released) and an append whose outcome is unknown (RecordUncertainAppend,
# into Uncertain, which a later close releases). Once the custody ended the
# owner accepts no input but a release, never returns to Active, and keeps
# every fact a successor restores.
#
# usage: custody_end_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's step
# bound, its three invariants and its two action properties, and checks them
# over the whole bounded space (safety). It then reruns the same space once
# per scenario witness, with that witness's negation as an extra invariant,
# and requires TLC to report exactly that violation, so no property holds
# vacuously:
#   - a release after an issuance (ReleasedAfterIssuance);
#   - an uncertain append, then a release (UncertainThenReleased);
#   - a second release (ReleasedTwice);
#   - an uncertain append after a revocation (UncertainAfterRevocation).
# With --mutants it seeds one defect at a time into a copy of the generated
# model and requires TLC to refuse each with the property it targets. TLC
# reports only the first violated invariant of a state, so each mutant run
# checks the generated invariants and only its target audit property:
#   - ReleaseCustody into Active (AuditReleaseEndsInReleased);
#   - ReleaseUncertainCustody staying Uncertain (AuditReleaseEndsInReleased);
#   - RecordUncertainAppend staying Active (AuditUncertainAppendEndsCustody);
#   - IssueRoot after a release, IssueChild after an uncertain append,
#     RevokeAlready and ResolveUse after a release, a second uncertain
#     append after a release, ConsumeUse after an uncertain append, and
#     ConsumeUseRefusedApproverRevoked after a release
#     (AuditEndedOwnerAcceptsNothing);
#   - ReleaseCustodyAgain into Active (action property
#     AuditCustodyEndLatches);
#   - RevokeNew after a release, a release that changes the owner's
#     generation, and a release that changes a spent use (action property
#     AuditCustodyEndKeepsTheFacts; a release that changes the retained
#     records, revocations or spend count breaks the generated revision
#     accounting first).
# An arm this script expects but cannot find fails the run. Each TLC run is
# held to the canonical lane's per-run cap (tlc_run_cap.sh) and fails closed
# as TLC INCOMPLETE.
set -euo pipefail

usage="usage: custody_end_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 5 )); then
  echo "error: max-steps must be an integer >= 5 (the deepest mutant needs 5 machine steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/grant-custody-end-audit.XXXXXX")"
# An interrupted or killed run ends its TLC child too (tlc_run_cap.sh).
trap 'if declare -F tlc_run_cap_reap >/dev/null; then tlc_run_cap_reap; fi; rm -rf "${work_dir}"' EXIT

# The audit drives these generated arms; each must be defined exactly once,
# and the generated variables must be the ones the audit names.
MODEL="${spec_dir}/model.tla" python3 - <<'PY'
import os
text = open(os.environ["MODEL"]).read()
arms = {
    "Configure": "(", "IssueRoot": "(", "IssueChild": "(", "RevokeNew": "(",
    "RevokeAlready": "(", "ResolveUse": "(", "ConsumeUse": "(",
    "ConsumeUseRefusedApproverRevoked": "(", "ReleaseCustody": " ==",
    "RecordUncertainAppend": " ==", "ReleaseUncertainCustody": " ==",
    "ReleaseCustodyAgain": " ==",
}
for arm, head in arms.items():
    count = text.count("\n" + arm + head)
    assert count == 1, f"expected one {arm} definition in model.tla, found {count}"
declared = [line for line in text.split("\n") if line.startswith("VARIABLES ")]
assert len(declared) == 1, "VARIABLES line not found in model.tla"
variables = sorted(name.strip() for name in declared[0][len("VARIABLES "):].split(","))
expected = sorted(["phase", "model_step_count", "root", "namespace", "generation",
                   "incarnation", "revision", "records", "revoked", "spent", "consumed",
                   "carried_decisions"])
assert variables == expected, f"the audit names the generated variables {expected}, model.tla declares {variables}"
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

audit_invariants=(
  AuditEndedOwnerAcceptsNothing
  AuditReleaseEndsInReleased
  AuditUncertainAppendEndsCustody
)
audit_properties=(
  AuditCustodyEndLatches
  AuditCustodyEndKeepsTheFacts
)

# usage: write_cfg <file> <only property or ""> <extra invariant lines>
# With an only property, the run checks that audit property alone (an
# invariant or an action property) beside every generated invariant;
# without one, it checks every audit property.
write_cfg() {
  local cfg="$1" only="$2" extra="$3"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}" "${cfg}"
  local listed="" properties="" name
  if [[ -z "${only}" ]]; then
    for name in "${audit_invariants[@]}"; do
      listed+="${listed:+
}  ${name}"
    done
    for name in "${audit_properties[@]}"; do
      properties+="${properties:+
}  ${name}"
    done
  elif [[ " ${audit_invariants[*]} " == *" ${only} "* ]]; then
    listed="  ${only}"
  elif [[ " ${audit_properties[*]} " == *" ${only} "* ]]; then
    properties="  ${only}"
  else
    echo "error: unknown audit property ${only}" >&2
    exit 1
  fi
  # The generated invariants stay; the audit's are listed first.
  local invariants_line="INVARIANTS"
  if [[ -n "${listed}${extra}" ]]; then
    invariants_line+="
${listed}${extra}"
  fi
  if [[ -n "${properties}" ]]; then
    replace_exact_line "INVARIANTS" "PROPERTIES
${properties}
${invariants_line}" "${cfg}"
  else
    replace_exact_line "INVARIANTS" "${invariants_line}" "${cfg}"
  fi
  replace_exact_line "  CiStateConstraint" "  AuditStateConstraint" "${cfg}"
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
# shellcheck source=../meerkat_machine/tlc_run_cap.sh
source "${spec_dir}/../meerkat_machine/tlc_run_cap.sh"

# The scratch spec directory: the generated model and this audit.
model_dir="${work_dir}/specs"
mkdir -p "${model_dir}"
cp "${spec_dir}/model.tla" "${spec_dir}/custody_end_audit.tla" "${model_dir}/"

# usage: run_tlc <name> <cfg> <dir> [extra tlc args...]; leaves the log at
# ${work_dir}/<name>.log
run_tlc() {
  local name="$1" cfg="$2" dir="$3"
  shift 3
  tlc_status=0
  # Witness and mutant runs end in a violation by design: -noGenerateSpecTE
  # keeps TLC from writing trace-exploration files next to the specs.
  tlc_run_capped custody_end_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" "${dir}/custody_end_audit.tla" || tlc_status=$?
}

extra_tlc_args=("$@")

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" "" ""
run_tlc safety "${safety_cfg}" "${model_dir}" "${extra_tlc_args[@]+"${extra_tlc_args[@]}"}"
cat "${work_dir}/safety.log"
if [[ "${tlc_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: grant custody end audit failed (tlc exit ${tlc_status}, bound ${max_steps})" >&2
  exit 1
fi

for witness in ReleasedAfterIssuance UncertainThenReleased ReleasedTwice \
  UncertainAfterRevocation; do
  cfg="${work_dir}/witness-${witness}.cfg"
  write_cfg "${cfg}" "" "
  NotAuditWitness${witness}"
  run_tlc "witness-${witness}" "${cfg}" "${model_dir}" "${extra_tlc_args[@]+"${extra_tlc_args[@]}"}"
  log="${work_dir}/witness-${witness}.log"
  if ! grep -q "Error: Invariant NotAuditWitness${witness} is violated" "${log}"; then
    cat "${log}"
    echo "error: witness ${witness} is not reachable at bound ${max_steps} (tlc exit ${tlc_status})" >&2
    exit 1
  fi
  depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
  echo "witness ${witness} reached in ${depth} states"
done

if [[ "${run_mutants}" == "true" ]]; then
  # usage: mutate <name> <expected violation> <property> <edit>
  # Copies the specs and applies one edit to the copy of the generated model:
  # "arm|old|new" replaces exactly one line <old> of the named arm's
  # definition with <new>; "arm|FRAME|variable|value" replaces the arm's one
  # UNCHANGED frame, read from the model, with an update of <variable> to
  # <value> and the rest of the frame. Then it checks the one property and
  # requires TLC to report the expected violation.
  mutate() {
    local name="$1" violation="$2" property="$3" edit="$4"
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${model_dir}"/*.tla "${dir}/"
    MODEL="${dir}/model.tla" EDIT="${edit}" python3 - <<'PY'
import os, re
path = os.environ["MODEL"]
parts = os.environ["EDIT"].split("|")
arm = parts[0]
text = open(path).read()
heads = [m.start() for m in re.finditer("\n" + re.escape(arm) + r"(\(| ==)", text)]
assert len(heads) == 1, f"expected one {arm} definition, found {len(heads)}"
start = heads[0] + 1
end = text.index("\n\n", start)
lines = text[start:end].split("\n")
if parts[1] == "FRAME":
    variable, value = parts[2], parts[3]
    frames = [i for i, line in enumerate(lines) if re.fullmatch(r"\s*/\\ UnchangedFrame_[0-9a-f]+", line)]
    assert len(frames) == 1, f"expected one frame in {arm}, found {len(frames)}"
    frame = lines[frames[0]].strip()[3:]
    match = re.search(r"\n" + frame + r" == UNCHANGED << (.*) >>\n", text)
    assert match, f"no definition of {frame}"
    names = [name.strip() for name in match.group(1).split(",")]
    assert variable in names, f"{variable} is not in {arm}'s frame {names}"
    kept = ", ".join(name for name in names if name != variable)
    lines[frames[0]:frames[0] + 1] = [f"    /\\ {variable}' = {value}", f"    /\\ UNCHANGED << {kept} >>"]
else:
    old, new = parts[1], parts[2]
    hits = [i for i, line in enumerate(lines) if line == old]
    assert len(hits) == 1, f"expected one line {old!r} in {arm}, found {len(hits)}"
    lines[hits[0]] = new
text = text[:start] + "\n".join(lines) + text[end:]
open(path, "w").write(text)
PY
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${cfg}" "${property}" ""
    run_tlc "mutant-${name}" "${cfg}" "${dir}" "${extra_tlc_args[@]+"${extra_tlc_args[@]}"}"
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -q "Error: ${violation} is violated" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by ${violation} (tlc exit ${tlc_status})" >&2
      exit 1
    fi
    local depth
    depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
    echo "mutant ${name} refused by ${violation} in ${depth} states"
  }

  mutate release_into_active "Invariant AuditReleaseEndsInReleased" AuditReleaseEndsInReleased \
    'ReleaseCustody|    /\ phase'"'"' = "Released"|    /\ phase'"'"' = "Active"'
  mutate uncertain_release_stays_uncertain "Invariant AuditReleaseEndsInReleased" \
    AuditReleaseEndsInReleased \
    'ReleaseUncertainCustody|    /\ phase'"'"' = "Released"|    /\ phase'"'"' = "Uncertain"'
  mutate uncertain_append_stays_active "Invariant AuditUncertainAppendEndsCustody" \
    AuditUncertainAppendEndsCustody \
    'RecordUncertainAppend|    /\ phase'"'"' = "Uncertain"|    /\ phase'"'"' = "Active"'
  mutate issue_root_after_release "Invariant AuditEndedOwnerAcceptsNothing" \
    AuditEndedOwnerAcceptsNothing \
    'IssueRoot|    /\ phase = "Active"|    /\ phase \in {"Active", "Released"}'
  mutate issue_child_after_uncertain "Invariant AuditEndedOwnerAcceptsNothing" \
    AuditEndedOwnerAcceptsNothing \
    'IssueChild|    /\ phase = "Active"|    /\ phase \in {"Active", "Uncertain"}'
  mutate repeated_revoke_after_release "Invariant AuditEndedOwnerAcceptsNothing" \
    AuditEndedOwnerAcceptsNothing \
    'RevokeAlready|    /\ phase = "Active"|    /\ phase \in {"Active", "Released"}'
  mutate use_after_release "Invariant AuditEndedOwnerAcceptsNothing" \
    AuditEndedOwnerAcceptsNothing \
    'ResolveUse|    /\ phase = "Active"|    /\ phase \in {"Active", "Released"}'
  mutate uncertain_append_after_release "Invariant AuditEndedOwnerAcceptsNothing" \
    AuditEndedOwnerAcceptsNothing \
    'RecordUncertainAppend|    /\ phase = "Active"|    /\ phase \in {"Active", "Released"}'
  mutate spend_after_uncertain "Invariant AuditEndedOwnerAcceptsNothing" \
    AuditEndedOwnerAcceptsNothing \
    'ConsumeUse|    /\ phase = "Active"|    /\ phase \in {"Active", "Uncertain"}'
  mutate refused_spend_after_release "Invariant AuditEndedOwnerAcceptsNothing" \
    AuditEndedOwnerAcceptsNothing \
    'ConsumeUseRefusedApproverRevoked|    /\ phase = "Active"|    /\ phase \in {"Active", "Released"}'
  mutate second_release_reactivates "Action property AuditCustodyEndLatches" \
    AuditCustodyEndLatches \
    'ReleaseCustodyAgain|    /\ phase'"'"' = "Released"|    /\ phase'"'"' = "Active"'
  mutate revoke_after_release "Action property AuditCustodyEndKeepsTheFacts" \
    AuditCustodyEndKeepsTheFacts \
    'RevokeNew|    /\ phase = "Active"|    /\ phase \in {"Active", "Released"}'
  # A change to the retained records or revocations also breaks the
  # generated revision accounting, which TLC reports first; a changed
  # identity breaks only this property.
  mutate release_changes_identity "Action property AuditCustodyEndKeepsTheFacts" \
    AuditCustodyEndKeepsTheFacts 'ReleaseCustody|FRAME|generation|generation + 1'
  # A spent use that changes at the close, with the spend count kept, breaks
  # only this property: an approval child's one use counted spent.
  mutate release_changes_a_spent_use "Action property AuditCustodyEndKeepsTheFacts" \
    AuditCustodyEndKeepsTheFacts \
    'ReleaseCustody|FRAME|spent|IF DOMAIN spent = {} THEN spent ELSE MapSet(spent, CHOOSE id \in DOMAIN spent : TRUE, 1)'
fi
echo "grant custody end audit passed at model_step_count <= ${max_steps}"
