#!/usr/bin/env bash
# Bounded TLC audit of the generated GrantAuthorityMachine's approval-carried
# child grants: a grant under a child rule has children only through a
# person's decision for the rule's approver id; such a child serves only its
# own requester, work and operation, at most its uses, each spent in the
# owner's journal before it enters, and never for an approver who is no
# longer current.
#
# usage: approval_grant_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's step
# bound and its eleven invariants, and checks them over the whole bounded
# space (safety) with TLC coverage: every wrapper in covered_actions below
# must generate a successor state, and the space must keep at least 90% of
# the distinct states its first green run found, so a guard that silently
# disables wrappers fails. It then reruns the same space once per scenario
# witness, with that witness's negation as an extra invariant, and requires
# TLC to report exactly that violation, so no scenario holds vacuously:
#   - an approval child issued (ApprovalChildIssued);
#   - a use entered once (UsedOnce);
#   - a second use refused (SecondUseRefused);
#   - a use with another operation, another requester or another work
#     refused, and one after the window, each where the valid use is open
#     (OtherDigestRefused, OtherRequesterRefused, OtherWorkRefused,
#     WindowElapsedRefused);
#   - a use refused after the parent was revoked
#     (ParentRevokedBeforeUseRefused);
#   - a spend refused for an approver no longer current, nothing spent
#     (ApproverRevokedBeforeUseRefused);
#   - a device-trusted decision refused under a rule that refuses device
#     trust, and a plain child refused under a rule, each where the valid
#     child is open (DeviceTrustRefusedUnderRule, PlainChildRefusedUnderRule);
#   - a spent use still spent after a restart (SpentThenCrashStillSpent);
#   - two attempts prepared at one revision, one entered, the other stale and
#     refused (RacingSecondAttemptRefused);
#   - an uncertain spend that landed, never entered and still spent after
#     the restart (UncertainSpendNotEntered).
# With --mutants it seeds one defect into a copy of the specs per mutant (the
# generated model, or the audit's journal for journal_drops_consumption) and
# requires TLC to refuse each with its property. TLC stops at its first
# violation and reports only the first violated invariant of a state, so
# each mutant run lists its expected invariant first:
#   - a spend that ignores the uses (consume_ignores_uses:
#     AuditNoUseBeyondUses);
#   - a use that ignores the bound operation (resolve_ignores_digest:
#     AuditUseOnlyExactOperation);
#   - a use that ignores the requester or the work (resolve_ignores_requester,
#     resolve_ignores_work: AuditUseOnlyBoundRequesterAndWork);
#   - an issue that skips the parent's rule (issue_skips_approval_rule:
#     AuditNoChildWithoutApprovalUnderRule);
#   - an issue that ignores the decision's assurance
#     (issue_ignores_assurance: AuditRuleAssuranceHolds);
#   - an approval child of a grant without a rule (issue_without_parent_rule:
#     AuditNoApprovalChildWithoutRule);
#   - an issue whose terms differ from the approved request, and one that
#     allows depth (issue_terms_differ_from_the_approved_request,
#     issue_allows_depth: AuditIssuedTermsAreApproved);
#   - a spend that ignores the approver's currentness
#     (consume_ignores_approver_currentness: AuditNoSpendAfterApproverRevoked);
#   - a second grant for one decision (duplicate_decision_issue:
#     AuditOneGrantPerDecision);
#   - a spend that does not advance the revision (consume_without_revision:
#     AuditSpendAdvancesPublication);
#   - a journal that drops the spend (journal_drops_consumption:
#     AuditSpentSurvivesRestart).
# The rebuild after a restart is assumed, not checked: the audit restores
# the state the journal acknowledged. The owner's rebuild writes that state
# directly from the journal's changes (it does not replay them through this
# machine), and what makes the fold safe is what it checks after: the
# machine's generated invariants and the owner's lineage verification, which
# refuse a state no run of the machine reaches. So is an attempt's
# discipline: it enters only while the revision it prepared at is current.
# An anchor this script expects but cannot find fails the run. Each TLC run is
# held to the canonical lane's per-run cap (tlc_run_cap.sh) and fails closed
# as TLC INCOMPLETE.
set -euo pipefail

usage="usage: approval_grant_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 6 )); then
  echo "error: max-steps must be an integer >= 6 (two attempts prepared and one entered need 6 machine steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/approval-grant-audit.XXXXXX")"
# An interrupted or killed run ends its TLC child too (tlc_run_cap.sh).
trap 'if declare -F tlc_run_cap_reap >/dev/null; then tlc_run_cap_reap; fi; rm -rf "${work_dir}"' EXIT

# The safety run's distinct-state floor is 90% of the whole space its first
# green run found at the lane's bound; a run at a smaller bound is not held
# to it.
default_steps=6
safety_states=1589
min_distinct_states=$(( safety_states * 9 / 10 ))

# Every wrapper must fire. TLC names a step after the audit's wrapper.
covered_actions=(
  AuditConfigure
  AuditIssueRoot
  AuditIssueChild
  AuditRevoke
  AuditResolve
  AuditSpend
  AuditSpendRefusedApproverRevoked
  AuditSpendUncertain
  AuditRestart
  AuditPrepare
  AuditEnter
  AuditEnterStale
)

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
  AuditNoUseBeyondUses
  AuditUseOnlyExactOperation
  AuditUseOnlyBoundRequesterAndWork
  AuditNoChildWithoutApprovalUnderRule
  AuditNoApprovalChildWithoutRule
  AuditRuleAssuranceHolds
  AuditIssuedTermsAreApproved
  AuditNoSpendAfterApproverRevoked
  AuditOneGrantPerDecision
  AuditSpendAdvancesPublication
  AuditSpentSurvivesRestart
)

# usage: write_cfg <file> <first invariant or ""> <extra invariant lines>
# The first invariant (an audit or generated one) is listed before every
# other: TLC reports only the first violated invariant of a state.
write_cfg() {
  local cfg="$1" first="$2" extra="$3"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}" "${cfg}"
  local listed="" name
  if [[ -n "${first}" ]]; then
    if grep -qxF -- "  ${first}" "${cfg}"; then
      # A generated invariant moves to the front.
      FIRST="  ${first}" awk '$0 == ENVIRON["FIRST"] { next } { print }' "${cfg}" > "${cfg}.next"
      mv "${cfg}.next" "${cfg}"
    elif [[ " ${audit_invariants[*]} " != *" ${first} "* ]]; then
      echo "error: unknown first invariant ${first}" >&2
      exit 1
    fi
    listed="  ${first}"
  fi
  for name in "${audit_invariants[@]}"; do
    if [[ "${name}" != "${first}" ]]; then
      listed+="${listed:+
}  ${name}"
    fi
  done
  replace_exact_line "INVARIANTS" "INVARIANTS
${listed}${extra}" "${cfg}"
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
cd "${spec_dir}"

# usage: run_tlc <name> <cfg> [extra tlc args...]; runs in the current
# directory and leaves the log at ${work_dir}/<name>.log
run_tlc() {
  local name="$1" cfg="$2"
  shift 2
  local status=0
  # Witness and mutant runs end in a violation by design: -noGenerateSpecTE
  # keeps TLC from writing trace-exploration files into the spec directory.
  tlc_run_capped approval_grant_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" approval_grant_audit.tla || status=$?
  return "${status}"
}

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" "" ""
safety_status=0
run_tlc safety "${safety_cfg}" -coverage 1 "$@" || safety_status=$?
safety_log="${work_dir}/safety.log"
cat "${safety_log}"
if [[ "${safety_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${safety_log}"; then
  echo "error: approval grant audit failed (tlc exit ${safety_status}, bound ${max_steps})" >&2
  exit 1
fi

distinct="$(sed -nE 's/^[0-9]+ states generated, ([0-9]+) distinct states found, 0 states left on queue\.$/\1/p' \
  "${safety_log}" | tail -1)"
if [[ -z "${distinct}" ]]; then
  echo "error: the safety run reported no final distinct-state count" >&2
  exit 1
fi
if (( max_steps >= default_steps && distinct < min_distinct_states )); then
  echo "error: safety run: ${distinct} distinct states, below the floor ${min_distinct_states} (90% of ${safety_states})" >&2
  exit 1
fi
echo "safety distinct states: ${distinct} (floor ${min_distinct_states} at bound >= ${default_steps})"

# TLC prints a coverage block every minute and one at the end; read the
# last. An action's line is
# `<Action line L, col C to line L, col C of module approval_grant_audit>: D:T`,
# with D the new distinct states it found and T the successor states it
# generated; an action that fires only into known states has D = 0, so T
# counts.
coverage="${work_dir}/safety-coverage.txt"
awk '/^The coverage statistics at / { block = ""; next } { block = block $0 "\n" } END { printf "%s", block }' \
  "${safety_log}" > "${coverage}"
for action in "${covered_actions[@]}"; do
  generated="$(awk -v head="<${action} line " '
    index($0, head) == 1 && $0 ~ / of module approval_grant_audit>: [0-9]+:[0-9]+$/ {
      split($NF, counts, ":"); total += counts[2]; found = 1
    }
    END { if (found) print total }' "${coverage}")"
  if [[ -z "${generated}" || "${generated}" == "0" ]]; then
    echo "error: safety run: ${action} never fired (coverage '${generated:-absent}')" >&2
    exit 1
  fi
  echo "covered ${action}: ${generated} successor states"
done

for witness in ApprovalChildIssued UsedOnce SecondUseRefused OtherDigestRefused \
  OtherRequesterRefused OtherWorkRefused WindowElapsedRefused ParentRevokedBeforeUseRefused \
  ApproverRevokedBeforeUseRefused DeviceTrustRefusedUnderRule SpentThenCrashStillSpent \
  RacingSecondAttemptRefused PlainChildRefusedUnderRule UncertainSpendNotEntered; do
  cfg="${work_dir}/witness-${witness}.cfg"
  write_cfg "${cfg}" "" "
  NotAuditWitness${witness}"
  status=0
  run_tlc "witness-${witness}" "${cfg}" "$@" || status=$?
  log="${work_dir}/witness-${witness}.log"
  if ! grep -q "Error: Invariant NotAuditWitness${witness} is violated" "${log}"; then
    cat "${log}"
    echo "error: witness ${witness} is not reachable at bound ${max_steps} (tlc exit ${status})" >&2
    exit 1
  fi
  depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
  echo "witness ${witness} reached in ${depth} states"
done

if [[ "${run_mutants}" == "true" ]]; then
  # usage: mutate <name> <expected invariant> <edits file>
  # Copies the specs and applies the edits file to the copies: each edit is
  # three lines (definition, old, new), separated by a line holding only
  # "--"; it replaces exactly one <old> by <new> in the named definition,
  # found once across the generated model and the audit (old and new may span
  # lines, written with "\n"). A definition runs from its head to the next
  # line that is blank or starts a new definition. Then it runs every
  # invariant, the expected one first, and requires TLC to report it
  # violated.
  mutate() {
    local name="$1" violation="$2" edits="$3"
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}/model.tla" "${spec_dir}/approval_grant_audit.tla" "${dir}/"
    MODEL="${dir}/model.tla" AUDIT="${dir}/approval_grant_audit.tla" EDITS="${edits}" python3 - <<'PY'
import os
import re

paths = [os.environ["MODEL"], os.environ["AUDIT"]]
texts = {path: open(path).read() for path in paths}
edits = [block.strip("\n").split("\n") for block in open(os.environ["EDITS"]).read().split("\n--\n")]
assert edits, "no edits"
for edit in edits:
    assert len(edit) == 3, f"an edit is three lines (definition, old, new): {edit}"
    name, old, new = edit[0], edit[1].replace("\\n", "\n"), edit[2].replace("\\n", "\n")
    heads = [
        (path, match.start() + 1)
        for path in paths
        for match in re.finditer(r"\n" + re.escape(name) + r"(\(| ==)", texts[path])
    ]
    assert len(heads) == 1, f"expected one {name} definition, found {len(heads)}"
    path, start = heads[0]
    text = texts[path]
    end_match = re.search(r"\n(?=\S|\n)", text[start:])
    end = start + end_match.start() if end_match else len(text)
    block = text[start:end]
    count = block.count(old)
    assert count == 1, f"expected one {old!r} in {name}, found {count}"
    texts[path] = text[:start] + block.replace(old, new) + text[end:]
for path in paths:
    open(path, "w").write(texts[path])
PY
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${cfg}" "${violation}" ""
    local status=0
    (cd "${dir}" && run_tlc "mutant-${name}" "${cfg}" "${tlc_args[@]+"${tlc_args[@]}"}") || status=$?
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -q "Error: Invariant ${violation} is violated" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by ${violation} (tlc exit ${status})" >&2
      exit 1
    fi
    local depth
    depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
    echo "mutant ${name} refused by ${violation} in ${depth} states"
  }
  tlc_args=("$@")

  edits="${work_dir}/edits"

  # A spend that ignores the uses left.
  cat > "${edits}" <<'EOF'
ConsumeUse
/\ ((leaf.id \in DOMAIN spent) /\ ((spent)[leaf.id] < (IF "value" \in DOMAIN leaf.approval THEN leaf.approval["value"] ELSE None).uses))
/\ (leaf.id \in DOMAIN spent)
EOF
  mutate consume_ignores_uses AuditNoUseBeyondUses "${edits}"

  # A use whose operation, requester or work is not compared.
  cat > "${edits}" <<'EOF'
approval_use_bound
((IF "value" \in DOMAIN leaf.approval THEN leaf.approval["value"] ELSE None).operation = operation)
TRUE
EOF
  mutate resolve_ignores_digest AuditUseOnlyExactOperation "${edits}"

  cat > "${edits}" <<'EOF'
approval_use_bound
(Some((IF "value" \in DOMAIN leaf.approval THEN leaf.approval["value"] ELSE None).requester) = requester)
TRUE
EOF
  mutate resolve_ignores_requester AuditUseOnlyBoundRequesterAndWork "${edits}"

  cat > "${edits}" <<'EOF'
approval_use_bound
(Some((IF "value" \in DOMAIN leaf.approval THEN leaf.approval["value"] ELSE None).work) = work)
TRUE
EOF
  mutate resolve_ignores_work AuditUseOnlyBoundRequesterAndWork "${edits}"

  # An issue that skips the parent's rule, and one that ignores the
  # decision's assurance.
  cat > "${edits}" <<'EOF'
approval_rule_admits
(IF (parent.child_approval = None) THEN TRUE ELSE ((record.approval # None)
(IF TRUE THEN TRUE ELSE ((record.approval # None)
EOF
  mutate issue_skips_approval_rule AuditNoChildWithoutApprovalUnderRule "${edits}"

  cat > "${edits}" <<'EOF'
approval_rule_admits
(IF ((IF "value" \in DOMAIN record.approval THEN record.approval["value"] ELSE None).assurance = "PersonAuthenticated") THEN TRUE ELSE
(IF TRUE THEN TRUE ELSE
EOF
  mutate issue_ignores_assurance AuditRuleAssuranceHolds "${edits}"

  # An approval child of a grant without a rule.
  cat > "${edits}" <<'EOF'
approval_rule_admits
(IF (record.approval = None) THEN TRUE ELSE (parent.child_approval # None))
TRUE
EOF
  mutate issue_without_parent_rule AuditNoApprovalChildWithoutRule "${edits}"

  # An issue whose terms differ from the approved request, and one that
  # allows a depth above 0.
  cat > "${edits}" <<'EOF'
IssueChild
(IF (record.approval = None) THEN TRUE ELSE (terms_digest = (IF "value" \in DOMAIN record.approval THEN record.approval["value"] ELSE None).request_digest))
TRUE
EOF
  mutate issue_terms_differ_from_the_approved_request AuditIssuedTermsAreApproved "${edits}"

  cat > "${edits}" <<'EOF'
approval_bounded
(record.restrictions.delegation_depth.remaining_edges = 0)
TRUE
EOF
  mutate issue_allows_depth AuditIssuedTermsAreApproved "${edits}"

  # A spend that ignores the host's answer for the approver.
  cat > "${edits}" <<'EOF'
ConsumeUse
/\ approver_current
/\ TRUE
EOF
  mutate consume_ignores_approver_currentness AuditNoSpendAfterApproverRevoked "${edits}"

  # A second grant for one decision.
  cat > "${edits}" <<'EOF'
IssueChild
(IF (record.approval = None) THEN TRUE ELSE (((IF "value" \in DOMAIN record.approval THEN record.approval["value"] ELSE None).decision_id \in carried_decisions) = FALSE))
TRUE
EOF
  mutate duplicate_decision_issue AuditOneGrantPerDecision "${edits}"

  # A spend that leaves the revision, and so every prepared decision,
  # current.
  cat > "${edits}" <<'EOF'
ConsumeUse
/\ revision' = (revision) + 1
/\ revision' = revision
EOF
  mutate consume_without_revision AuditSpendAdvancesPublication "${edits}"

  # A journal that drops the spend: the owner's rebuild would forget it.
  cat > "${edits}" <<'EOF'
AuditJournalConsumed
audit_durable' = AuditSnapshotAfter
audit_durable' = audit_durable
EOF
  mutate journal_drops_consumption AuditSpentSurvivesRestart "${edits}"
fi
echo "approval grant audit passed at model_step_count <= ${max_steps}"
