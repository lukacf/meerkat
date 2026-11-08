#!/usr/bin/env bash
# Bounded TLC audit of a client delegation admitted under its channel's exact
# binding whose worker starts only after that channel closed (Turbo S S107):
# its transcript reconciliation, worker start, consequential-effect authority,
# busy-source requeue and the steer of a continuation held for it resolve
# through the operation's own retired channel, never through a foreign
# channel or a stale binding; a worker started after the close settles
# through the revoked-worker reconciliation; a blocked hand-back on the
# unbound channel settles Cancelled; restart recovery, which passes the
# completed turn behind a recorded classification, settles it as recorded;
# and a channel whose delegation intake the coordinator closed admits no
# delegation.
#
# usage: live_delegation_admitted_start_after_close_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's finite
# model values, its five safety invariants, its step bound and its
# deterministic-prefix constraint. It runs TLC once with every generated
# invariant (safety), then once per goal with that goal as an extra
# invariant, and requires TLC to report each goal violated: the bound
# provably reaches every post-close arm, so the invariants are not vacuously
# true. With --mutants it then seeds fourteen defects into a copy of the
# generated model and requires the audit to refuse each: eight restore the
# bound-channel-only guards of one family of arms (reconciliation confirmed;
# reconciliation as a conflict or missing; worker start; consequential
# effect; busy-source requeue; steer authorization; steer delivery; steer
# reconciliation), and that family's goal must become unreachable; one drops
# the close from the worker start's post-close disjunct, which
# AuditStaleBindingNeverServes must refuse; one counts a worker started after
# the close against the closed channel, which
# AuditClosedChannelHoldsNoWorkerCount must refuse; two leave a blocked
# hand-back Blocked on the unbound channel (fresh, and from a terminal
# recorded while bound), which AuditRevokedBlockedIsCancelled must refuse;
# one lets the restart reconciliation refuse the completed turn behind a
# recorded classification, which must make every restart goal unreachable;
# and one drops the closed intake from both admission arms, which
# AuditNoAdmissionAfterIntakeClose must refuse.
# Nothing is hand-copied, so a DSL change regenerates ci.cfg and the audit
# follows it; an anchor this script expects but cannot find fails the run
# instead of silently narrowing the check.
set -euo pipefail

usage="usage: live_delegation_admitted_start_after_close_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 18 )); then
  echo "error: max-steps must be an integer >= 18 (the deepest goal, a steer reconciled after the close, needs 18)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/live-delegation-admitted-start-after-close-audit.XXXXXX")"
# An interrupted or killed run ends its TLC child too (tlc_run_cap.sh).
trap 'if declare -F tlc_run_cap_reap >/dev/null; then tlc_run_cap_reap; fi; rm -rf "${work_dir}"' EXIT

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

# usage: write_cfg <file> <extra invariant lines>
write_cfg() {
  local cfg="$1" extra="$2"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1"}' "${cfg}"
  replace_exact_line "  AgentRuntimeIdValues = {}" '  AgentRuntimeIdValues = {"runtime_1"}' "${cfg}"
  replace_exact_line "  SessionLlmIdentityValues = {}" '  SessionLlmIdentityValues = {"identity_1"}' "${cfg}"
  replace_exact_line "  OperationIdValues = {}" '  OperationIdValues = {"operation_1"}' "${cfg}"
  replace_exact_line "  StringValues = {}" '  StringValues = {"", "channel_a", "channel_b", "profile_1", "pending_a", "owner_1", "ready_1", "activation_a", "lease_1", "digest_s", "append_boot", "append_2", "digest_1", "commit_1", "interaction_1", "turn_1", "worker_1", "authority_1", "continuation_1", "digest_c"}' "${cfg}"
  replace_exact_line "INVARIANTS" "INVARIANTS
  AuditForeignChannelNeverServes
  AuditStaleBindingNeverServes
  AuditClosedChannelHoldsNoWorkerCount
  AuditRevokedBlockedIsCancelled
  AuditNoAdmissionAfterIntakeClose${extra}" "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}" "${cfg}"
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
# Each TLC run is held to the canonical lane's per-run cap (fails closed as
# TLC INCOMPLETE, exit 3).
# shellcheck source=tlc_run_cap.sh
source "${spec_dir}/tlc_run_cap.sh"
cd "${spec_dir}"
extra_tlc_args=("$@")

# run_tlc <name> <cfg>: runs in the current directory and leaves the log at
# ${work_dir}/<name>.log
run_tlc() {
  local name="$1" cfg="$2"
  local log="${work_dir}/${name}.log"
  tlc_status=0
  # Goal runs end in a violation by design: -noGenerateSpecTE keeps TLC from
  # writing trace-explorer specs next to the model.
  tlc_run_capped live_delegation_admitted_start_after_close_audit "${name}" "${log}" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE -config "${cfg}" "${extra_tlc_args[@]}" \
    live_delegation_admitted_start_after_close_audit.tla || tlc_status=$?
  grep -E 'states generated|distinct states|is violated|Error:' "${log}" | sed "s/^/[${name}] /" || true
}

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" ""
run_tlc safety "${safety_cfg}"
if [[ "${tlc_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  cat "${work_dir}/safety.log" >&2
  echo "error: live delegation admitted start after close audit safety run failed (tlc exit ${tlc_status}, bound ${max_steps})" >&2
  exit 1
fi

goals=(
  AuditNeverConfirmedAfterClose
  AuditNeverUnconfirmedAfterClose
  AuditNeverAuthorizedAfterClose
  AuditNeverConsequentialAfterClose
  AuditNeverRequeuedAfterClose
  AuditNeverSettledAfterClose
  AuditNeverSteerSettledAfterClose
  AuditNeverFreshBlockedSettled
  AuditNeverCustodyBlockedSettled
  AuditNeverIntakeClosedWhileBound
  AuditNeverAuthorizedAfterIntakeClose
  AuditNeverRestartBlockedSettled
  AuditNeverRestartFailedSettled
  AuditNeverRestartCancelledSettled
)
# Each goal must be reachable within the bound: TLC has to report it violated.
for goal in "${goals[@]}"; do
  goal_cfg="${work_dir}/${goal}.cfg"
  write_cfg "${goal_cfg}" "
  ${goal}"
  run_tlc "${goal}" "${goal_cfg}"
  if ! grep -q "Invariant ${goal} is violated" "${work_dir}/${goal}.log"; then
    cat "${work_dir}/${goal}.log" >&2
    echo "error: live delegation admitted start after close audit goal ${goal} is not reached within bound ${max_steps}" >&2
    exit 1
  fi
done

if [[ "${run_mutants}" == "true" ]]; then
  # usage: mutate <name> unreachable|violated <invariant> <edit> [arms];
  # <edit> is a python file (or `-` for stdin) that rewrites model.tla in
  # place and must find each anchor it names; [arms] reaches it as ARMS.
  # `unreachable`: the mutant must make <invariant> (a goal) hold over the
  # whole bounded space; `violated`: TLC must report <invariant> violated.
  mutate() {
    local name="$1" expect="$2" invariant="$3" edit="$4" arms="${5:-}"
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}"/*.tla "${dir}/"
    ARMS="${arms}" MODEL="${dir}/model.tla" python3 -I "${edit}"
    local cfg="${work_dir}/mutant-${name}.cfg"
    if [[ "${expect}" == "unreachable" ]]; then
      write_cfg "${cfg}" "
  ${invariant}"
    else
      write_cfg "${cfg}" ""
    fi
    (cd "${dir}" && run_tlc "mutant-${name}" "${cfg}")
    local log="${work_dir}/mutant-${name}.log"
    if [[ "${expect}" == "unreachable" ]]; then
      if ! grep -q "Model checking completed. No error has been found." "${log}"; then
        cat "${log}" >&2
        echo "error: mutant ${name} still reaches ${invariant}'s goal" >&2
        exit 1
      fi
      echo "mutant ${name} refused: ${invariant}'s goal is unreachable"
    else
      if ! grep -q "Invariant ${invariant} is violated" "${log}"; then
        cat "${log}" >&2
        echo "error: mutant ${name} was not refused by ${invariant}" >&2
        exit 1
      fi
      echo "mutant ${name} refused by ${invariant}"
    fi
  }

  # Restore the bound-channel-only binding guards in every per-phase arm of
  # the transitions named in ARMS (three guards each).
  bound_only_edit="${work_dir}/bound_only_guards.py"
  cat > "${bound_only_edit}" <<'PY'
import os, re
path = os.environ["MODEL"]
arms = os.environ["ARMS"].split()
assert arms, "no transitions named"
text = open(path).read()
guard = re.compile(
    r"^    /\\ \(IF (\(\(IF \(channel_id \\in DOMAIN live_execution_(?:runtime_id|fence|generation)_by_channel\)"
    r".*?\) = Some\((?:runtime_id|fence_token|generation)\)\)) THEN TRUE ELSE \(.*\)\)$"
)
for arm in arms:
    for phase in ("Idle", "Attached", "Running"):
        name = f"{arm}{phase}("
        start = text.find("\n" + name)
        assert start >= 0, f"{name} not found"
        end = text.find("\n\n", start + 1)
        lines = text[start:end].split("\n")
        hits = 0
        for index, line in enumerate(lines):
            match = guard.match(line)
            if match:
                lines[index] = "    /\\ " + match.group(1)
                hits += 1
        assert hits == 3, f"expected three post-close binding guards in {name}, found {hits}"
        text = text[:start] + "\n".join(lines) + text[end:]
open(path, "w").write(text)
PY

  mutate reconcile_confirmed_bound_only unreachable AuditNeverConfirmedAfterClose \
    "${bound_only_edit}" "ReconcileLiveDelegationTranscriptConfirmed"
  mutate reconcile_unconfirmed_bound_only unreachable AuditNeverUnconfirmedAfterClose \
    "${bound_only_edit}" "ReconcileLiveDelegationTranscriptMaterialConflict ReconcileLiveDelegationTranscriptMissing"
  mutate worker_start_bound_only unreachable AuditNeverAuthorizedAfterClose \
    "${bound_only_edit}" "AuthorizeLiveDelegationWorkerStart"
  mutate consequential_bound_only unreachable AuditNeverConsequentialAfterClose \
    "${bound_only_edit}" "AuthorizeLiveConsequentialEffect"
  mutate requeue_bound_only unreachable AuditNeverRequeuedAfterClose \
    "${bound_only_edit}" "RequeueUnstartedLiveDelegation"
  mutate steer_authorization_bound_only unreachable AuditNeverSteerSettledAfterClose \
    "${bound_only_edit}" "AuthorizeLiveDelegationSteer"
  mutate steer_delivery_bound_only unreachable AuditNeverSteerSettledAfterClose \
    "${bound_only_edit}" "ResolveLiveDelegationSteerDelivery"
  mutate steer_reconciliation_bound_only unreachable AuditNeverSteerSettledAfterClose \
    "${bound_only_edit}" "ReconcileLiveDelegationSteerConfirmed ReconcileLiveDelegationSteerMaterialConflict ReconcileLiveDelegationSteerMissing"

  # The worker start's post-close disjunct forgets that the channel must be
  # unbound: a still-bound channel then takes a stale fence.
  mutate worker_start_ignores_the_close violated AuditStaleBindingNeverServes - <<'PY'
import os
path = os.environ["MODEL"]
text = open(path).read()
unbound = (
    "~((channel_id \\in DOMAIN live_execution_runtime_id_by_channel)) /\\ "
    "~((channel_id \\in DOMAIN live_execution_fence_by_channel)) /\\ "
    "~((channel_id \\in DOMAIN live_execution_generation_by_channel)) /\\ "
)
for phase in ("Idle", "Attached", "Running"):
    name = f"AuthorizeLiveDelegationWorkerStart{phase}("
    start = text.find("\n" + name)
    assert start >= 0, f"{name} not found"
    end = text.find("\n\n", start + 1)
    block = text[start:end]
    count = block.count(unbound)
    assert count == 3, f"expected three post-close disjuncts in {name}, found {count}"
    text = text[:start] + block.replace(unbound, "") + text[end:]
open(path, "w").write(text)
PY

  # A worker started after the close takes a slot of the closed channel.
  mutate worker_start_counts_the_closed_channel violated AuditClosedChannelHoldsNoWorkerCount - <<'PY'
import os
path = os.environ["MODEL"]
text = open(path).read()
counted = (
    "    /\\ live_delegation_active_worker_count_by_channel' = IF (channel_id \\in DOMAIN "
    "live_delegation_active_worker_count_by_channel) THEN MapIncrement("
    "live_delegation_active_worker_count_by_channel, channel_id, 1) ELSE MapSet("
    "live_delegation_active_worker_count_by_channel, channel_id, 1)"
)
for phase in ("Idle", "Attached", "Running"):
    name = f"AuthorizeLiveDelegationWorkerStart{phase}("
    start = text.find("\n" + name)
    assert start >= 0, f"{name} not found"
    end = text.find("\n\n", start + 1)
    lines = text[start:end].split("\n")
    hits = [i for i, line in enumerate(lines)
            if line.startswith("    /\\ live_delegation_active_worker_count_by_channel' = ")]
    assert len(hits) == 1, f"expected one worker count update in {name}, found {len(hits)}"
    assert lines[hits[0]] != counted, f"{name} already counts every start"
    lines[hits[0]] = counted
    text = text[:start] + "\n".join(lines) + text[end:]
open(path, "w").write(text)
PY

  # A blocked hand-back reconciled on the unbound channel stays Blocked, as a
  # bound terminal record leaves it, so nothing ever settles it. ARMS carries
  # the revoked-worker arm, its blocked settle and the defect, split by `|`;
  # every per-phase variant of the arm must hold the settle exactly once.
  blocked_stays_edit="${work_dir}/blocked_stays_blocked.py"
  cat > "${blocked_stays_edit}" <<'PY'
import os
path = os.environ["MODEL"]
arm, settle, keep = os.environ["ARMS"].split("|")
text = open(path).read()
for phase in ("Idle", "Attached", "Running", "Retired", "Stopped"):
    name = f"{arm}{phase}("
    start = text.find("\n" + name)
    assert start >= 0, f"{name} not found"
    end = text.find("\n\n", start + 1)
    block = text[start:end]
    count = block.count(settle)
    assert count == 1, f"expected one blocked settle in {name}, found {count}"
    text = text[:start] + block.replace(settle, keep) + text[end:]
open(path, "w").write(text)
PY
  mutate fresh_blocked_stays_blocked violated AuditRevokedBlockedIsCancelled \
    "${blocked_stays_edit}" \
    'ReconcileRevokedLiveDelegationWorkerAfterRestartFresh|(IF (terminal = "Blocked") THEN "Cancelled"|(IF (terminal = "Blocked") THEN "Blocked"'
  mutate custody_blocked_stays_blocked violated AuditRevokedBlockedIsCancelled \
    "${blocked_stays_edit}" \
    'ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustody|MapSet(live_delegation_schedule_state_by_operation, operation_id, "Cancelled")|MapSet(live_delegation_schedule_state_by_operation, operation_id, "Blocked")'

  # Restart recovery passes the Mob's completed turn; the terminal-custody
  # arm refuses it against a recorded classification of that turn, as it did
  # before, so no restart goal is reachable.
  mutate restart_refuses_the_completed_turn_behind_a_classification unreachable AuditNeverRestartSettled - <<'PY'
import os
path = os.environ["MODEL"]
text = open(path).read()
agreement = (
    "((terminal = \"Completed\") /\\ (IF ((IF (operation_id \\in DOMAIN "
    "live_delegation_worker_terminal_by_operation)"
)
refused = (
    "(FALSE /\\ (IF ((IF (operation_id \\in DOMAIN "
    "live_delegation_worker_terminal_by_operation)"
)
for phase in ("Idle", "Attached", "Running", "Retired", "Stopped"):
    name = f"ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustody{phase}("
    start = text.find("\n" + name)
    assert start >= 0, f"{name} not found"
    end = text.find("\n\n", start + 1)
    block = text[start:end]
    count = block.count(agreement)
    assert count == 1, f"expected one completed-turn agreement in {name}, found {count}"
    text = text[:start] + block.replace(agreement, refused) + text[end:]
open(path, "w").write(text)
PY

  # Both admission arms forget the closed intake.
  mutate admission_ignores_the_closed_intake violated AuditNoAdmissionAfterIntakeClose - <<'PY'
import os
path = os.environ["MODEL"]
text = open(path).read()
guard = "\n    /\\ ~((channel_id \\in live_delegation_intake_closed_channels))"
for arm in ("AdmitLiveDelegation", "AdmitLiveInteractionDelegation"):
    for phase in ("Idle", "Attached", "Running"):
        name = f"{arm}{phase}("
        start = text.find("\n" + name)
        assert start >= 0, f"{name} not found"
        end = text.find("\n\n", start + 1)
        block = text[start:end]
        count = block.count(guard)
        assert count == 1, f"expected one intake guard in {name}, found {count}"
        text = text[:start] + block.replace(guard, "") + text[end:]
open(path, "w").write(text)
PY
  echo "live delegation admitted start after close audit refused 14 mutants"
fi
echo "live delegation admitted start after close audit passed at model_step_count <= ${max_steps} (14 goals reached)"
