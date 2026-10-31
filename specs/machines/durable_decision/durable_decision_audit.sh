#!/usr/bin/env bash
# Bounded TLC audit of the generated DurableDecisionMachine: one durable
# decision's hold and sealed binding, its attested decision, the execution
# permit and the executed fact, expiry, withdrawal and the requester notice
# an ended decision owes. An approved decision expires at its deadline unless
# an execution permit is outstanding: a permit is issued only before the
# deadline, the executed fact settles it at any time, and a release at or
# after the deadline records the expiry.
#
# usage: durable_decision_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's step
# bound and its fourteen invariants, and checks them over the whole bounded
# space (safety) with TLC coverage: every wrapper in covered_actions below
# (one per generated arm, a per-phase arm's phases sharing one, and the
# owner's commit) must generate a successor state, and the space must keep
# at least 90% of the distinct states its first green run found, so a guard
# that silently disables arms fails. It then reruns the same space once per
# scenario witness, with that witness's negation as an extra invariant, and
# requires TLC to report exactly that violation, so no scenario holds
# vacuously:
#   - a human approval executed and its notice delivered
#     (HumanApprovalExecutedAndNoticed);
#   - an execution retried after a release (RetriedAfterRelease);
#   - an approval closed by the owner's permanent refusal (ExecutionRefused);
#   - a review escalated, then approved by the human
#     (EscalatedThenHumanApproved);
#   - a stale digest refused, then an approval (StaleDigestThenApproved);
#   - a decision at the deadline records the expiry
#     (DecideAtDeadlineExpires);
#   - a decision refused after a withdrawal (WithdrawnThenDecideRefused);
#   - a self-approval refused, then the approver's
#     (SelfApprovalThenAnotherApproves);
#   - a replayed record after the commit changes nothing
#     (CommitThenRecordReplayed);
#   - a replayed and a conflicting hold (HoldReplayedAndConflicting);
#   - a permit outstanding across the deadline settled as executed
#     (PermitAcrossDeadlineExecuted);
#   - a release after the deadline records the expiry
#     (ReleaseAfterDeadlineExpires);
#   - an authorization at the deadline refused (AuthorizeAtDeadlineRefused);
#   - a decision observed before the first presentation refused, then an
#     approval (DecidedBeforePresentationRefused);
#   - an approval observed in the same millisecond as the first presentation
#     (ApprovedAtThePresentation).
# With --mutants it seeds one defect into a copy of the generated model per
# mutant and requires TLC to refuse each with its property. TLC stops at its
# first violation and reports only the first violated invariant of a state,
# so each mutant run lists its expected invariant first:
#   - an approval without evidence, without the subject digest, without a
#     presentation, by the requester, or for another approver id
#     (AuditApprovalIsAttested);
#   - a rejection by an ineligible decider (AuditRejectionIsAttested);
#   - an execution of another request, and an execution from Open
#     (AuditExecutedOnlyFromApprovedWithHeldBinding);
#   - an approved decision that expires before its deadline
#     (AuditExpiryOnlyAtTheDeadline);
#   - an expiry while a permit is outstanding
#     (AuditApprovedExpiresOnlyWithoutPermit);
#   - a withdrawal after a decision (AuditWithdrawOnlyOpenByRequesterOrHost);
#   - an expired decision re-opened (AuditTerminalsLatch);
#   - a rejection that owes no notice (AuditNoticeOwedOnceAtTheEnd);
#   - a conflicting hold that overwrites the sealed deadline
#     (AuditSealedFieldsNeverChange);
#   - an authorization at or after the deadline
#     (AuditNoPermitAtOrAfterDeadline);
#   - a release that leaves the permit outstanding (AuditReleaseClearsPermit);
#   - a deadline observation that drops the outstanding permit, orphaning
#     the owner's commit (AuditCommittedIsSettleable);
#   - an approval and a rejection that keep the presentation check and drop
#     the order, and a refusal of a decision observed in the same
#     millisecond as the first presentation
#     (AuditDecisionFollowsPresentation);
#   - a presentation that overwrites the first presentation time with its
#     own, and one to another audience recorded as the approver's
#     (AuditFirstPresentationIsTheEarliest);
#   - a presentation at 0 recorded as the first presentation
#     (first_presentation_is_a_reading, a generated invariant), and a
#     decision accepted on a carried presentation at 0
#     (AuditDecisionFollowsPresentation).
# The executor's obligations are assumed, not checked: the machine records
# that a permit is outstanding, not who holds it or whether a commit landed.
# The audit models one executor per decision (one owner drives the permit),
# that never releases or refuses a permit after its commit (it only records
# the execution), and that settles first after a restart (it reads its own
# commit record before releasing). AuditCommittedIsSettleable holds only
# under them: a release after the commit orphans it (hold, approve,
# authorize, commit, release). DecisionExecutionPermit and the settlement
# calls of DurableDecisionService state the same obligations.
# An anchor this script expects but cannot find fails the run. Each TLC run is
# held to the canonical lane's per-run cap (tlc_run_cap.sh) and fails closed
# as TLC INCOMPLETE.
set -euo pipefail

usage="usage: durable_decision_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 6 )); then
  echo "error: max-steps must be an integer >= 6 (the retried execution and the delivered notice need 6 machine steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/durable-decision-audit.XXXXXX")"
# An interrupted or killed run ends its TLC child too (tlc_run_cap.sh).
trap 'if declare -F tlc_run_cap_reap >/dev/null; then tlc_run_cap_reap; fi; rm -rf "${work_dir}"' EXIT

# The safety run's distinct-state floor is 90% of the whole space its first
# green run found at the lane's bound; a run at a smaller bound is not held
# to it.
default_steps=6
safety_states=36073
min_distinct_states=$(( safety_states * 9 / 10 ))

# Every wrapper must fire. TLC names an arm after the audit's wrapper, so
# each is listed by its wrapper.
covered_actions=(
  AuditHoldHuman
  AuditHoldReview
  AuditHoldRefusedWindow
  AuditHoldRefusedSelfReview
  AuditHoldReplayed
  AuditHoldConflict
  AuditPresentAtDeadline
  AuditPresentRefusedSubjectChanged
  AuditPresentRefusedIncomplete
  AuditPresentToApprover
  AuditPresentRefusedUntimed
  AuditPresentRefusedWrongApproverAudience
  AuditPresentToReviewer
  AuditPresentRefusedWrongReviewerAudience
  AuditPresentRefusedNotOpen
  AuditDecideAtDeadline
  AuditDecideRefusedReviewStage
  AuditDecideRefusedEvidenceRequired
  AuditDecideRefusedSubjectChanged
  AuditDecideRefusedNotEligible
  AuditDecideRefusedSelfApproval
  AuditDecideRefusedPresentationIncomplete
  AuditDecideRefusedDecidedBeforePresentation
  AuditDecideApproved
  AuditDecideRejected
  AuditDecideRefusedNotOpen
  AuditReviewAtDeadline
  AuditReviewRefusedHumanStage
  AuditReviewRefusedWrongReviewer
  AuditReviewRefusedEvidenceRequired
  AuditReviewRefusedSubjectChanged
  AuditReviewRefusedPresentationIncomplete
  AuditReviewAllowed
  AuditReviewDenied
  AuditReviewEscalated
  AuditReviewRefusedNotOpen
  AuditWithdrawAtDeadline
  AuditWithdrawByHost
  AuditWithdrawByRequester
  AuditWithdrawRefusedNotRequester
  AuditWithdrawRefusedNotOpen
  AuditExpireOpenDue
  AuditExpireOpenNotDue
  AuditExpireApprovedDue
  AuditExpireApprovedSettlementOwed
  AuditExpireApprovedNotDue
  AuditExpireSettled
  AuditAuthorizeExecution
  AuditAuthorizeExecutionAtDeadline
  AuditAuthorizeExecutionRefusedSettlementOwed
  AuditAuthorizeExecutionRefusedBinding
  AuditAuthorizeExecutionRefusedNotApproved
  AuditReleaseExecution
  AuditReleaseExecutionAtDeadline
  AuditReleaseExecutionRefusedNotAuthorized
  AuditReleaseExecutionRefusedNotApproved
  AuditRecordExecuted
  AuditRecordExecutedRefusedNotAuthorized
  AuditRecordExecutedRefusedBinding
  AuditRecordExecutedReplayed
  AuditRecordExecutedReplayRefusedBinding
  AuditRecordExecutedRefusedNotApproved
  AuditCloseRefused
  AuditCloseRefusedWithPermit
  AuditCloseRefusedAtDeadline
  AuditCloseRefusedNotApproved
  AuditSettleNoticeDelivered
  AuditSettleNoticeFailed
  AuditSettleNoticeReplayed
  AuditSettleNoticeRefused
  AuditSettleNoticeRefusedNotEnded
  AuditCommit
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
  AuditTerminalsLatch
  AuditApprovalIsAttested
  AuditRejectionIsAttested
  AuditExecutedOnlyFromApprovedWithHeldBinding
  AuditExpiryOnlyAtTheDeadline
  AuditApprovedExpiresOnlyWithoutPermit
  AuditWithdrawOnlyOpenByRequesterOrHost
  AuditSealedFieldsNeverChange
  AuditNoticeOwedOnceAtTheEnd
  AuditNoPermitAtOrAfterDeadline
  AuditReleaseClearsPermit
  AuditDecisionFollowsPresentation
  AuditCommittedIsSettleable
  AuditFirstPresentationIsTheEarliest
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
  tlc_run_capped durable_decision_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" durable_decision_audit.tla || status=$?
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
  echo "error: durable decision audit failed (tlc exit ${safety_status}, bound ${max_steps})" >&2
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
# `<Action line L, col C to line L, col C of module durable_decision_audit>: D:T`,
# with D the new distinct states it found and T the successor states it
# generated; an action that fires only into known states has D = 0, so T
# counts.
coverage="${work_dir}/safety-coverage.txt"
awk '/^The coverage statistics at / { block = ""; next } { block = block $0 "\n" } END { printf "%s", block }' \
  "${safety_log}" > "${coverage}"
for action in "${covered_actions[@]}"; do
  generated="$(awk -v head="<${action} line " '
    index($0, head) == 1 && $0 ~ / of module durable_decision_audit>: [0-9]+:[0-9]+$/ {
      split($NF, counts, ":"); total += counts[2]; found = 1
    }
    END { if (found) print total }' "${coverage}")"
  if [[ -z "${generated}" || "${generated}" == "0" ]]; then
    echo "error: safety run: ${action} never fired (coverage '${generated:-absent}')" >&2
    exit 1
  fi
  echo "covered ${action}: ${generated} successor states"
done

for witness in HumanApprovalExecutedAndNoticed RetriedAfterRelease ExecutionRefused \
  EscalatedThenHumanApproved StaleDigestThenApproved DecideAtDeadlineExpires \
  WithdrawnThenDecideRefused SelfApprovalThenAnotherApproves CommitThenRecordReplayed \
  HoldReplayedAndConflicting PermitAcrossDeadlineExecuted ReleaseAfterDeadlineExpires \
  AuthorizeAtDeadlineRefused DecidedBeforePresentationRefused ApprovedAtThePresentation; do
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
  # Copies the specs and applies the edits file to the copy of the generated
  # model: each edit is three lines (arm, old, new), separated by a line
  # holding only "--"; it replaces exactly one <old> by <new> in the named
  # arm's definition (old and new may span lines, written with "\n"). Then
  # it runs every invariant, the expected one first, and requires TLC to
  # report it violated.
  mutate() {
    local name="$1" violation="$2" edits="$3"
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}/model.tla" "${spec_dir}/durable_decision_audit.tla" "${dir}/"
    MODEL="${dir}/model.tla" EDITS="${edits}" python3 - <<'PY'
import os
path = os.environ["MODEL"]
text = open(path).read()
edits = [block.strip("\n").split("\n") for block in open(os.environ["EDITS"]).read().split("\n--\n")]
assert edits, "no edits"
for edit in edits:
    assert len(edit) == 3, f"an edit is three lines (arm, old, new): {edit}"
    arm, old, new = edit[0], edit[1].replace("\\n", "\n"), edit[2].replace("\\n", "\n")
    heads = text.count("\n" + arm + "(")
    assert heads == 1, f"expected one {arm} definition, found {heads}"
    start = text.index("\n" + arm + "(") + 1
    end = text.index("\n\n", start)
    block = text[start:end]
    count = block.count(old)
    assert count == 1, f"expected one {old!r} in {arm}, found {count}"
    text = text[:start] + block.replace(old, new) + text[end:]
open(path, "w").write(text)
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

  # usage: frame_update <arm> <variable> <value>
  # Prints an edit that replaces the arm's one UNCHANGED frame, read from the
  # generated model so a renamed frame cannot silently weaken the mutant, by
  # an update of <variable> to <value> and the rest of the frame.
  frame_update() {
    MODEL="${spec_dir}/model.tla" ARM="$1" VARIABLE="$2" VALUE="$3" python3 - <<'PY'
import os, re
text = open(os.environ["MODEL"]).read()
arm, variable, value = os.environ["ARM"], os.environ["VARIABLE"], os.environ["VALUE"]
start = text.index("\n" + arm + "(") + 1
end = text.index("\n\n", start)
frames = re.findall(r"UnchangedFrame_[0-9a-f]+", text[start:end])
assert len(frames) == 1, f"expected one frame in {arm}, found {frames}"
match = re.search(r"\n" + frames[0] + r" == UNCHANGED << (.*) >>\n", text)
assert match, f"no definition of {frames[0]}"
names = [name.strip() for name in match.group(1).split(",")]
assert variable in names, f"{variable} is not in {arm}'s frame {names}"
kept = ", ".join(name for name in names if name != variable)
print(arm)
print(f"/\\ {frames[0]}")
print(f"/\\ {variable}' = {value}\\n    /\\ UNCHANGED << {kept} >>")
PY
  }

  edits="${work_dir}/edits"

  # An approval with one attestation guard dropped: no evidence, another
  # subject digest, the requester as decider, another approver id; and no
  # presentation (the guard that the decision was observed at or after a
  # presentation, which is also its presence check).
  for mutant in "no_evidence|(arg_evidence_digest # None)" \
    "stale_subject|(digest = subject_digest)" \
    "self_approval|(decider # requester)" \
    "other_approver_id|(arg_approver_id = approver_id)"; do
    printf 'DecideApproved\n/\\ %s\n/\\ TRUE\n' "${mutant#*|}" > "${edits}"
    mutate "approve_${mutant%%|*}" AuditApprovalIsAttested "${edits}"
  done
  cat > "${edits}" <<'EOF'
DecideApproved
/\ (IF ((first_presented_to_approver_at_ms # None) /\ ((IF "value" \in DOMAIN first_presented_to_approver_at_ms THEN first_presented_to_approver_at_ms["value"] ELSE None) <= observed_at_ms)) THEN TRUE ELSE ((carried_presentation_at_ms # None) /\ ((IF "value" \in DOMAIN carried_presentation_at_ms THEN carried_presentation_at_ms["value"] ELSE None) <= observed_at_ms)))
/\ TRUE
EOF
  mutate approve_no_presentation AuditApprovalIsAttested "${edits}"

  cat > "${edits}" <<'EOF'
DecideRejected
/\ eligible
/\ TRUE
EOF
  mutate reject_ineligible AuditRejectionIsAttested "${edits}"

  cat > "${edits}" <<'EOF'
RecordExecuted
/\ (arg_request_digest = request_digest)
/\ TRUE
EOF
  mutate executed_another_request AuditExecutedOnlyFromApprovedWithHeldBinding "${edits}"

  cat > "${edits}" <<'EOF'
RecordExecutedRefusedNotApprovedOpen
/\ phase' = "Open"
/\ phase' = "Executed"
EOF
  mutate executed_from_open AuditExecutedOnlyFromApprovedWithHeldBinding "${edits}"

  cat > "${edits}" <<'EOF'
ExpireApprovedNotDue
/\ phase' = "Approved"
/\ phase' = "Expired"
EOF
  mutate approved_expires_early AuditExpiryOnlyAtTheDeadline "${edits}"

  cat > "${edits}" <<'EOF'
ExpireApprovedDue
/\ ~(permit_issued)
/\ TRUE
EOF
  mutate expires_with_permit AuditApprovedExpiresOnlyWithoutPermit "${edits}"

  cat > "${edits}" <<'EOF'
WithdrawRefusedNotOpenApproved
/\ phase' = "Approved"
/\ phase' = "Withdrawn"
EOF
  mutate withdraw_after_decision AuditWithdrawOnlyOpenByRequesterOrHost "${edits}"

  cat > "${edits}" <<'EOF'
ExpireSettledExpired
/\ phase' = "Expired"
/\ phase' = "Open"
EOF
  mutate expired_reopens AuditTerminalsLatch "${edits}"

  cat > "${edits}" <<'EOF'
DecideRejected
/\ notice' = owed_if_due(notice)
/\ notice' = notice
EOF
  mutate rejection_owes_nothing AuditNoticeOwedOnceAtTheEnd "${edits}"

  frame_update HoldConflictOpen deadline_at_ms arg_deadline_at_ms > "${edits}"
  mutate conflict_overwrites_deadline AuditSealedFieldsNeverChange "${edits}"

  cat > "${edits}" <<'EOF'
AuthorizeExecution
/\ (now_ms < deadline_at_ms)
/\ TRUE
EOF
  mutate authorize_at_deadline AuditNoPermitAtOrAfterDeadline "${edits}"

  cat > "${edits}" <<'EOF'
ReleaseExecution
/\ permit_issued' = FALSE
/\ permit_issued' = TRUE
EOF
  mutate release_keeps_permit AuditReleaseClearsPermit "${edits}"

  frame_update ExpireApprovedSettlementOwed permit_issued FALSE > "${edits}"
  mutate deadline_drops_permit AuditCommittedIsSettleable "${edits}"

  # A decision that keeps the presentation check and drops the order: each
  # presentation time compares as at or before any observed time.
  for arm in DecideApproved DecideRejected; do
    cat > "${edits}" <<EOF
${arm}
first_presented_to_approver_at_ms["value"] ELSE None) <= observed_at_ms)
first_presented_to_approver_at_ms["value"] ELSE None) >= 0)
--
${arm}
carried_presentation_at_ms["value"] ELSE None) <= observed_at_ms)
carried_presentation_at_ms["value"] ELSE None) >= 0)
EOF
    case "${arm}" in
      DecideApproved) name=approve_before_presentation ;;
      *) name=reject_before_presentation ;;
    esac
    mutate "${name}" AuditDecisionFollowsPresentation "${edits}"
  done

  cat > "${edits}" <<'EOF'
DecideRefusedDecidedBeforePresentation
(observed_at_ms < (IF "value" \in DOMAIN first_presented_to_approver_at_ms THEN
(observed_at_ms <= (IF "value" \in DOMAIN first_presented_to_approver_at_ms THEN
EOF
  mutate refuse_at_the_presentation AuditDecisionFollowsPresentation "${edits}"

  cat > "${edits}" <<'EOF'
PresentToApprover
earliest_presentation(first_presented_to_approver_at_ms, Some(presented_at_ms))
Some(presented_at_ms)
EOF
  mutate first_presentation_overwritten AuditFirstPresentationIsTheEarliest "${edits}"

  # A presentation to any audience recorded as the approver's: the audit
  # reads the audience from the input, not from the arm that fired.
  cat > "${edits}" <<'EOF'
PresentToApprover
/\ (audience = approver_id)
/\ TRUE
EOF
  mutate present_to_another_audience AuditFirstPresentationIsTheEarliest "${edits}"

  # A presentation at 0, no reading, recorded as the first presentation.
  cat > "${edits}" <<'EOF'
PresentToApprover
/\ (presented_at_ms # 0)
/\ TRUE
EOF
  mutate present_untimed first_presentation_is_a_reading "${edits}"

  # A decision accepted on a carried presentation at 0.
  cat > "${edits}" <<'EOF'
DecideApproved
/\ (carried_presentation_at_ms # Some(0))
/\ TRUE
EOF
  mutate decide_carries_untimed AuditDecisionFollowsPresentation "${edits}"
fi
echo "durable decision audit passed at model_step_count <= ${max_steps}"
