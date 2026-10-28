#!/usr/bin/env bash
# Bounded TLC audit of the entered state of a reviewed effect in the
# generated ApprovalLifecycleMachine: a spent attempt stays registered as
# Used while its admitting dispatch is live or a step admitted under it is
# still in flight; a later step of the entered effect is admitted only
# through AdmitEnteredStep while the dispatch is live, never spending; and
# the attempt is released at the later of the dispatch end and the last
# admitted step's finish.
#
# usage: entered_step_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's two
# attempt ids, its step bound, its six invariants and its action property,
# and checks them over the whole bounded space (safety) with TLC coverage:
# every arm in covered_actions below (each new AdmitEnteredStep,
# FinishEnteredStep and closing arm, and every arm the entered state
# changed) must generate a successor state, and the space must keep at least
# 90% of the distinct states its first green run found, so a guard that
# silently disables arms fails. It then reruns the same space once per
# scenario witness, with that witness's negation as an extra invariant, and
# requires TLC to report exactly that violation, so no scenario holds
# vacuously:
#   - the dispatch cancelled while an admitted step is in flight: the
#     attempt stays Used, a new step is refused, and the step's finish
#     releases it (CancelWhileStepInFlight);
#   - a step refused after the attempt's release (RefusedAfterRelease);
#   - the helper graph admission and two child steps under one consent spend
#     (GraphAndTwoChildrenAfterConsent).
# With --mutants it seeds one defect per invariant and property into a copy
# of the generated model and requires TLC to refuse each with that property.
# TLC stops at its first violation and reports only the first violated
# invariant of a state, so each mutant run lists its expected invariant
# first; each defect breaks no other property first:
#   - a spend that does not open the entered state
#     (entered_state_covers_exactly_the_used_attempts);
#   - a closing finish that leaves no step in flight without releasing
#     (closing_entered_effect_has_a_step_in_flight);
#   - a second spend of an entered review accepted (AuditOneSpendPerAttempt);
#   - a step admitted for an allowed, unspent review (AuditStepOnlyAfterSpend);
#   - a step admitted while the effect closes (AuditNoStepAfterDispatchEnd);
#   - a finish with no step in flight (AuditFinishesNeverExceedAdmissions);
#   - the dispatch end releasing the attempt while a step is in flight
#     (AuditReleaseWaitsForSteps);
#   - the dispatch end leaving the attempt registered with no step in flight,
#     and a dispatch end that leaves the effect open with a step in flight
#     (AuditEnteredStateMatchesHistory);
#   - a step that refunds the consent it entered under
#     (AuditStepsNeverSpendOrSettle).
# An anchor this script expects but cannot find fails the run. Each TLC run is
# held to the canonical lane's per-run cap (tlc_run_cap.sh) and fails closed
# as TLC INCOMPLETE.
set -euo pipefail

usage="usage: entered_step_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 8 )); then
  echo "error: max-steps must be an integer >= 8 (the longest witness is 7 steps and FinishEnteredStepClosing needs 7)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/entered-step-audit.XXXXXX")"
# An interrupted or killed run ends its TLC child too (tlc_run_cap.sh).
trap 'if declare -F tlc_run_cap_reap >/dev/null; then tlc_run_cap_reap; fi; rm -rf "${work_dir}"' EXIT

# The safety run's distinct-state floor is 90% of the whole space its first
# green run found at the lane's bound (16: both attempts entered with two
# steps each, both dispatches ending with steps in flight, every finish); a
# run at a smaller bound is not held to it.
default_steps=16
safety_states=17007
min_distinct_states=$(( safety_states * 9 / 10 ))

# Every arm the entered state added or changed, and the refusals that meet an
# entered attempt (a second spend, a repeated hand-back, a retirement, a
# withdrawal after the consent spend); each must fire. TLC names an arm
# after the audit's wrapper, so each is listed by its wrapper.
covered_actions=(
  AuditConsumeReviewForEntry
  AuditConsumeConsentForEntry
  AuditConsumeReviewRejectedUsed
  AuditConsumeConsentRejectedAlreadyConsumed
  AuditAdmitEnteredStep
  AuditAdmitEnteredStepRejectedMissing
  AuditAdmitEnteredStepRejectedConsentLinked
  AuditAdmitEnteredStepRejectedRetired
  AuditAdmitEnteredStepRejectedNotEntered
  AuditAdmitEnteredStepRejectedClosing
  AuditFinishEnteredStepOpen
  AuditFinishEnteredStepClosing
  AuditFinishEnteredStepReleases
  AuditFinishEnteredStepRejectedMissing
  AuditFinishEnteredStepRejectedNotInFlight
  AuditReleaseReviewUsed
  AuditReleaseReviewUsedClosing
  AuditReleaseReviewRejectedClosing
  AuditReleaseReviewRejectedMissing
  AuditRetireReviewRejectedSettled
  AuditWithdrawConsentRejectedMismatch
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
  AuditOneSpendPerAttempt
  AuditStepOnlyAfterSpend
  AuditNoStepAfterDispatchEnd
  AuditFinishesNeverExceedAdmissions
  AuditReleaseWaitsForSteps
  AuditEnteredStateMatchesHistory
)

# usage: write_cfg <file> <first invariant or ""> <extra invariant lines>
# The first invariant (an audit or generated one) is listed before every
# other: TLC reports only the first violated invariant of a state.
write_cfg() {
  local cfg="$1" first="$2" extra="$3"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line '  SetOfStringValues = {{}, {"alpha"}}' \
    '  SetOfStringValues = {{}, {"alpha"}, {"beta"}, {"alpha", "beta"}}' "${cfg}"
  replace_exact_line '  StringValues = {"alpha"}' '  StringValues = {"alpha", "beta"}' "${cfg}"
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
  replace_exact_line "INVARIANTS" "PROPERTIES
  AuditStepsNeverSpendOrSettle
INVARIANTS
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
  tlc_run_capped entered_step_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" entered_step_audit.tla || status=$?
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
  echo "error: entered-step audit failed (tlc exit ${safety_status}, bound ${max_steps})" >&2
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
# `<Action line L, col C to line L, col C of module entered_step_audit>: D:T`,
# with D the new distinct states it found and T the successor states it
# generated; an action that fires only into known states has D = 0, so T
# counts.
coverage="${work_dir}/safety-coverage.txt"
awk '/^The coverage statistics at / { block = ""; next } { block = block $0 "\n" } END { printf "%s", block }' \
  "${safety_log}" > "${coverage}"
for action in "${covered_actions[@]}"; do
  generated="$(awk -v head="<${action} line " '
    index($0, head) == 1 && $0 ~ / of module entered_step_audit>: [0-9]+:[0-9]+$/ {
      split($NF, counts, ":"); total += counts[2]; found = 1
    }
    END { if (found) print total }' "${coverage}")"
  if [[ -z "${generated}" || "${generated}" == "0" ]]; then
    echo "error: safety run: ${action} never fired (coverage '${generated:-absent}')" >&2
    exit 1
  fi
  echo "covered ${action}: ${generated} successor states"
done

for witness in CancelWhileStepInFlight RefusedAfterRelease GraphAndTwoChildrenAfterConsent; do
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
  # usage: mutate <name> <expected violation> <first invariant or ""> <edits file>
  # Copies the specs and applies the edits file to the copy of the generated
  # model: each edit is three lines (arm, old, new), separated by a line
  # holding only "--"; it replaces exactly one <old> by <new> in the named
  # arm's definition (old and new may span lines, written with "\n"). Then
  # it runs every property, the expected invariant first, and requires TLC
  # to report the expected violation.
  mutate() {
    local name="$1" violation="$2" first="$3" edits="$4"
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}/model.tla" "${spec_dir}/entered_step_audit.tla" "${dir}/"
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
    write_cfg "${cfg}" "${first}" ""
    local status=0
    (cd "${dir}" && run_tlc "mutant-${name}" "${cfg}" "${tlc_args[@]+"${tlc_args[@]}"}") || status=$?
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -q "Error: ${violation} is violated" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by ${violation} (tlc exit ${status})" >&2
      exit 1
    fi
    local depth
    depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
    echo "mutant ${name} refused by ${violation} in ${depth} states"
  }
  tlc_args=("$@")

  # The frame AdmitEnteredStep keeps, read from the generated model so a
  # renamed frame cannot silently weaken the consent-refund mutant; the
  # mutant keeps every variable of it except the consent's spend flag.
  read -r step_frame step_frame_rest < <(MODEL="${spec_dir}/model.tla" python3 - <<'PY'
import os, re
text = open(os.environ["MODEL"]).read()
start = text.index("\nAdmitEnteredStep(") + 1
end = text.index("\n\n", start)
frames = re.findall(r"UnchangedFrame_[0-9a-f]+", text[start:end])
assert len(frames) == 1, f"expected one frame in AdmitEnteredStep, found {frames}"
match = re.search(r"\n" + frames[0] + r" == UNCHANGED << (.*) >>\n", text)
assert match, f"no definition of {frames[0]}"
names = [name.strip() for name in match.group(1).split(",")]
assert "approval_consent_consumed" in names, names
kept = ",".join(name for name in names if name != "approval_consent_consumed")
print(frames[0], kept)
PY
)
  if [[ -z "${step_frame}" || -z "${step_frame_rest}" ]]; then
    echo "error: could not read AdmitEnteredStep's frame" >&2
    exit 1
  fi
  step_frame_rest="${step_frame_rest//,/, }"

  edits="${work_dir}/edits"
  steps_update_prefix="/\\ review_entered_steps' = MapSet(review_entered_steps, review_id, ((IF \"value\""

  cat > "${edits}" <<'EOF'
ConsumeReviewForEntry
    /\ review_entered_phase' = MapSet(review_entered_phase, review_id, "Open")\n    /\ review_entered_steps' = MapSet(review_entered_steps, review_id, 0)
    /\ UNCHANGED << review_entered_phase, review_entered_steps >>
EOF
  mutate spend_opens_no_entered_state "Invariant entered_state_covers_exactly_the_used_attempts" \
    entered_state_covers_exactly_the_used_attempts "${edits}"

  cat > "${edits}" <<'EOF'
FinishEnteredStepClosing
 /\ ((review_entered_steps)[review_id] # 1) /\ ((review_entered_phase)[review_id] = "Closing")
 /\ ((review_entered_phase)[review_id] = "Closing")
EOF
  mutate closing_finish_leaves_none "Invariant closing_entered_effect_has_a_step_in_flight" \
    closing_entered_effect_has_a_step_in_flight "${edits}"

  cat > "${edits}" <<'EOF'
ConsumeReviewForEntry
((review_statuses)[review_id] = "Allowed")
((review_statuses)[review_id] \in {"Allowed", "Used"})
EOF
  mutate second_spend_accepted "Invariant AuditOneSpendPerAttempt" AuditOneSpendPerAttempt "${edits}"

  # The mutant step also counts an attempt with no entered state (the
  # generated update reads the existing count).
  steps_update="$(MODEL="${spec_dir}/model.tla" PREFIX="${steps_update_prefix}" python3 - <<'PY'
import os
text = open(os.environ["MODEL"]).read()
start = text.index("\nAdmitEnteredStep(") + 1
end = text.index("\n\n", start)
lines = [line.strip() for line in text[start:end].split("\n") if line.strip().startswith(os.environ["PREFIX"])]
assert len(lines) == 1, f"expected one steps update in AdmitEnteredStep, found {lines}"
print(lines[0])
PY
)"
  cat > "${edits}" <<EOF
AdmitEnteredStep
 /\\ ((review_statuses)[review_id] = "Used") /\\ ((review_entered_phase)[review_id] = "Open")))
 /\\ (((review_statuses)[review_id] = "Allowed") \\/ (((review_statuses)[review_id] = "Used") /\\ ((review_entered_phase)[review_id] = "Open")))))
--
AdmitEnteredStep
${steps_update}
/\\ review_entered_steps' = MapIncrement(review_entered_steps, review_id, 1)
EOF
  mutate step_admits_unspent "Invariant AuditStepOnlyAfterSpend" AuditStepOnlyAfterSpend "${edits}"

  cat > "${edits}" <<'EOF'
AdmitEnteredStep
((review_entered_phase)[review_id] = "Open")))
((review_entered_phase)[review_id] \in {"Open", "Closing"})))
EOF
  mutate step_admitted_while_closing "Invariant AuditNoStepAfterDispatchEnd" \
    AuditNoStepAfterDispatchEnd "${edits}"

  cat > "${edits}" <<'EOF'
ReleaseReviewUsedClosing
/\ review_entered_phase' = MapSet(review_entered_phase, review_id, "Closing")
/\ review_entered_phase' = review_entered_phase
EOF
  mutate end_leaves_effect_open "Invariant AuditEnteredStateMatchesHistory" \
    AuditEnteredStateMatchesHistory "${edits}"

  cat > "${edits}" <<'EOF'
FinishEnteredStepOpen
 /\ ((review_entered_steps)[review_id] # 0) /\ ((review_entered_phase)[review_id] = "Open")))
 /\ ((review_entered_phase)[review_id] = "Open")))
EOF
  mutate finish_without_step "Invariant AuditFinishesNeverExceedAdmissions" \
    AuditFinishesNeverExceedAdmissions "${edits}"

  cat > "${edits}" <<'EOF'
ReleaseReviewUsed
 /\ ((review_entered_steps)[review_id] = 0)))
))
EOF
  mutate release_with_step_in_flight "Invariant AuditReleaseWaitsForSteps" \
    AuditReleaseWaitsForSteps "${edits}"

  cat > "${edits}" <<'EOF'
ReleaseReviewUsed
/\ review_ids' = (review_ids \ {review_id})\n    /\ review_statuses' = MapRemove(review_statuses, review_id)\n    /\ review_retirements' = MapRemove(review_retirements, review_id)\n    /\ review_entered_phase' = MapRemove(review_entered_phase, review_id)\n    /\ review_entered_steps' = MapRemove(review_entered_steps, review_id)
/\ UNCHANGED << review_ids, review_statuses, review_retirements, review_entered_phase, review_entered_steps >>
EOF
  mutate release_keeps_attempt "Invariant AuditEnteredStateMatchesHistory" \
    AuditEnteredStateMatchesHistory "${edits}"

  cat > "${edits}" <<EOF
AdmitEnteredStep
/\\ ${step_frame}
/\\ approval_consent_consumed' = [a \\in DOMAIN approval_consent_consumed |-> FALSE]\\n    /\\ UNCHANGED << ${step_frame_rest} >>
EOF
  mutate step_refunds_consent "Action property AuditStepsNeverSpendOrSettle" "" "${edits}"
fi
echo "entered-step audit passed at model_step_count <= ${max_steps}"
