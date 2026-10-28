#!/usr/bin/env bash
# TLC audit of MobMachine member residency (Dormant, Waking, Live).
#
# usage: member_residency_audit.sh [--mutants] [extra tlc args...]
#
# Derives the TLC configs from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's model
# values (two identities, three runtime ids, fence tokens 1..3, generations
# 0..1, three sessions, the audit's strings), each run's wake-attempt cap
# (AuditMaxAttempts, held by AuditStateConstraint) and its VIEW, which leaves
# out the two counters no guard, other variable's update or invariant of the
# generated model reads (model_step_count and topology_epoch). The runner
# first checks that the model still reads neither counter anywhere else, and
# fails otherwise. It checks eight small runs over their whole space with TLC
# coverage (see member_residency_audit.tla):
# - seed (AuditSeedSpec): one identity, its seat session unborn or born,
#   through every build seed, every boot verdict, the seal and the host
#   carry-out;
# - wake (AuditWakeSpec): one identity under the default seed through the boot
#   plan and seal, every WakeMember arm for six input shapes, every
#   attempt-fenced resolution (the fresh create of an unborn seat's session
#   among them), the late feedback in Running and Stopped, the seat session
#   replay, the member's own profile check, the runtime-ready observation,
#   three release shapes, the drift and terminal classifications, the peer
#   notice classification, the release verdict, Stop and Resume;
# - retire (AuditRetireSpec): one identity woken, released and retired, the
#   routed and archive-only retirement starts, the routed retire retry, the
#   archives, the replayed starts, a rotation finished in Running and in
#   Stopped, a host-pending seed, the identity seated again on its first
#   seat's session, and the mob's destroy;
# - terminal (AuditTerminalSpec): the Completed and Destroyed phases;
# - resume (AuditResumeSpec): one identity through an explicit resume, its
#   readiness before Resume (while Stopped) included;
# - edge (AuditEdgeSpec): two identities and one wiring edge and its
#   trust realization outcomes (an owed edge repair);
# - seat (AuditSeatSpec): two identities from the spawn ladder's Opened phase
#   through the dormant seat (one over an existing session, one over an
#   unborn one), the embodied spawn commit, the release and the rotation;
# - wake2 (AuditWake2Spec): two identities woken beside each other.
# Every run checks every generated invariant (the residency invariants among
# them), AuditAttemptEndsClosed and AuditWakingClosable (an open attempt is
# never replaced and ends only Live, Dormant or without a phase; every Waking
# member has an enabled closing arm for every kind) and, where the run has
# recovery or seat arms, its RecoverKeepsPhase property (no such arm overwrites
# a phase). The wake run also checks AuditLifecycleNeverWakes (a Lifecycle
# input never wakes, a notice classification never changes residency), that
# exactly one WakeMember arm answers every input shape, that a local wake never
# closes Live without an endpoint while it owes EndpointPending and can always
# close Live when it owes no endpoint cause, that the release verdict is
# Release exactly when the Released arm would honour the member's own
# registration, that a refused binding request applies only to the wake it
# belongs to, that only an unborn seat's session is created afresh, and that a
# release of another epoch never demotes. The seed run checks that the boot
# plan never leaves a member that blocks the seal. The retire run checks that
# no runtime retire is routed for a member with no registration, that a
# settled retirement can always archive and under an admitted destroy nothing
# holds it, that no rotation begins or commits under an admitted destroy, and
# that a resume seat is unborn exactly when no build created its session. The
# resume run checks that the explicit resume can always join a wake in flight
# while its readiness is not in flight and its topology has not started,
# whatever readiness it already settled (AuditResumeJoinable), that it
# holds no member work while its readiness is in flight or once its topology
# started (AuditResumeStagesHoldNoWork), and that it reserves a member it
# rebuilds (Dormant or Live) only before a readiness of its own started
# (AuditResumeReservesBeforeReadiness).
# TLC evaluates the module's wake refusal ASSUME (a present, bound member with
# a phase is never refused as not a member) and its release ASSUMEs (the
# verdict helper, and no member wired to a placed member is releasable) at the
# start of every run.
# Each run must finish without an error, reach at least its floor of distinct
# states and fire every action of its covered list (TLC coverage). It then
# reruns a space once per reachability witness, with that witness's negation
# as an extra invariant (a state goal) or action property (a step goal), and
# requires TLC to report exactly that violation, so the properties are not
# vacuous. With --mutants it then seeds one defect per residency invariant and
# property into a copy of the generated model and requires TLC to refuse each
# with the invariant, property or assumption it breaks. An anchor this script
# expects but cannot find fails the run. Each TLC run is held to the canonical
# lane's per-run cap (tlc_run_cap.sh) and fails closed as TLC INCOMPLETE.
set -euo pipefail

run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
# Each run's distinct-state floor is 90% of the whole space its first green
# run found, so a guard that silently disables arms fails the lane.
seed_states=3078
wake_states=1608
retire_states=5659
terminal_states=238
resume_states=4004
edge_states=1148
seat_states=23452
wake2_states=1425

# Every arm each run must fire. TLC names an arm after the generated
# operator, since every audit wrapper here is a plain application.
seed_covered_actions=(
  RecoverRosterMemberRunning
  RecoverMemberSessionBindingFreshRunning
  RecoverMemberPeerEndpointFreshRunning
  SeedMemberResidencyRunning
  SeedMemberResidencyStopped
  SeedMemberResidencyCompleted
  ResolveMemberHostPreparationRunning
  ResolveMemberHostPreparationStopped
  ResolveMemberHostPreparationCompleted
  PlanBootMaterializationWake
  PlanBootMaterializationVerify
  PlanBootMaterializationStayDormant
  SealBootMaterializationPlan
  WakeMemberAuthorizedRunning
  AbandonMemberWakeDormantRunning
  BeginPlacedCompletionLifecycleQuiesceFreshStop
  StopRunning
  ResumeStopped
  BeginPlacedCompletionLifecycleQuiesceFresh
  CompleteRunning
)

wake_covered_actions=(
  RecoverRosterMemberRunning
  RecoverMemberSessionBindingFreshRunning
  RecoverMemberSessionBindingAlreadyCurrentRunning
  RecoverMemberPeerEndpointFreshRunning
  RecoverSpawnedMemberPeerEndpointFreshRunning
  RegisterMemberPeerRunning
  RecoverMemberRestoreFailureRunning
  ResolveRuntimeBindingRefusalRunning
  ResolveRuntimeBindingRefusalStaleRunning
  ResolveRuntimeBindingRefusalStaleStopped
  RecoverMemberRestoreHostRefusedRunning
  PlanBootMaterializationWake
  PlanBootMaterializationStayDormant
  SealBootMaterializationPlan
  WakeMemberAuthorizedRunning
  WakeMemberJoinedRunning
  WakeMemberAlreadyLiveRunning
  WakeMemberRefusedRunning
  WakeMemberRefusedStopped
  ResolveMemberRevivalBoundRunning
  ResolveMemberRevivalSucceededRunningLocal
  ResolveMemberRevivalFailedRunning
  ResolveMemberRevivalHostRefusedRunning
  ResolveMemberRevivalRetryableRunning
  AbandonMemberWakeDormantRunning
  AbandonMemberWakeBrokenRunning
  ResolveRecreatedMemberSessionSucceededRunningLocal
  ResolveMemberRevivalBoundStaleRunning
  ResolveMemberRevivalBoundStaleStopped
  ResolveMemberRevivalSucceededStaleRunning
  ResolveMemberRevivalSucceededStaleStopped
  ResolveMemberRevivalFailedStaleRunning
  ResolveMemberRevivalFailedStaleStopped
  ResolveMemberRevivalHostRefusedStaleRunning
  ResolveMemberRevivalHostRefusedStaleStopped
  ResolveMemberRevivalRetryableStaleRunning
  ResolveMemberRevivalRetryableStaleStopped
  AbandonMemberWakeStaleRunning
  AbandonMemberWakeStaleStopped
  ResolveRecreatedMemberSessionSucceededStaleRunning
  ResolveRecreatedMemberSessionSucceededStaleStopped
  ResolveMemberRevivalSessionCreatedRunning
  ResolveMemberRevivalSessionCreatedStaleRunning
  ResolveMemberRevivalSessionCreatedStaleStopped
  RecoverMemberSeatSessionUnbornRunning
  RecoverMemberSeatSessionBornRunning
  AuthorizeSpawnProfileSeatedRunning
  ObserveRuntimeReady
  ReleaseMemberMaterializationReleasedRunning
  ReleaseMemberMaterializationReleasedStopped
  ReleaseMemberMaterializationStaleRunning
  ReleaseMemberMaterializationStaleStopped
  ClassifyMemberLiveMaterializationRevivable
  ClassifyMemberLiveMaterializationTerminal
  ClassifyMemberLiveMaterializationTerminalDormant
  ClassifyPeerNoticeRunning
  ClassifyPeerNoticeStopped
  ClassifyMemberReleaseRunning
  ClassifyMemberReleaseStopped
  BeginPlacedCompletionLifecycleQuiesceFreshStop
  StopRunning
  ResumeStopped
)

retire_covered_actions=(
  RetireRunningReleasing
  RetireRunningReleasingArchiveOnly
  RetireRunningPreservingBinding
  RetireRunningPreservingBindingArchiveOnly
  RetireStoppedReleasing
  RetireStoppedReleasingArchiveOnly
  RetireStoppedPreservingBinding
  RetireStoppedPreservingBindingArchiveOnly
  RetireMember
  RetireMemberArchiveOnly
  RetryRuntimeRetireRunning
  RetryRuntimeRetireStopped
  ObserveRuntimeRetired
  ObserveRuntimeRetiredStopped
  SessionIngressDetachedForMobDestroyRunning
  SessionIngressDetachedForMobDestroyStopped
  ObserveMemberRetirementArchivedLive
  ObserveMemberRetirementArchivedRetired
  ObserveMemberRetirementArchivedLiveStopped
  ObserveMemberRetirementArchivedRetiredStopped
  RecoverRosterMemberRetirementStartedReleasing
  RecoverRosterMemberRetirementStartedPreservingBinding
  AuthorizeSpawnProfileRunning
  BeginMemberRotationRunning
  CommitMemberRotationLiveRunning
  CommitMemberRotationRetiredRunning
  CommitMemberRotationLiveStopped
  CommitMemberRotationRetiredStopped
  AdmitDestroyCleanup
  AdmitDestroyMemberRetireLiveRunning
  AdmitDestroyMemberRetireArchiveOnlyRunning
  AdmitDestroyMemberRetireAlreadyRetiringSessionRunning
  AdmitDestroyMemberRetireAlreadyRetiringReleasedSessionRunning
  ObserveDestroyMemberRetirementArchivedLiveRunning
  ObserveDestroyMemberRetirementArchivedRetiredRunning
  DestroyMob
)

terminal_covered_actions=(
  RecoverRosterMemberRetiredRunning
  WakeMemberRefusedCompleted
  WakeMemberRefusedDestroyed
  ResolveMemberRevivalBoundStaleCompleted
  ResolveMemberRevivalBoundStaleDestroyed
  ResolveMemberRevivalSucceededStaleCompleted
  ResolveMemberRevivalSucceededStaleDestroyed
  ResolveMemberRevivalFailedStaleCompleted
  ResolveMemberRevivalFailedStaleDestroyed
  ResolveMemberRevivalHostRefusedStaleCompleted
  ResolveMemberRevivalHostRefusedStaleDestroyed
  ResolveMemberRevivalRetryableStaleCompleted
  ResolveMemberRevivalRetryableStaleDestroyed
  AbandonMemberWakeStaleCompleted
  AbandonMemberWakeStaleDestroyed
  ResolveRecreatedMemberSessionSucceededStaleCompleted
  ResolveRecreatedMemberSessionSucceededStaleDestroyed
  ResolveMemberRevivalSessionCreatedStaleCompleted
  ResolveMemberRevivalSessionCreatedStaleDestroyed
  ReleaseMemberMaterializationReleasedCompleted
  ReleaseMemberMaterializationStaleCompleted
  ReleaseMemberMaterializationStaleDestroyed
  ClassifyPeerNoticeCompleted
  ClassifyPeerNoticeDestroyed
  ClassifyMemberReleaseCompleted
  ClassifyMemberReleaseDestroyed
  ResolveRuntimeBindingRefusalStaleCompleted
  ResolveRuntimeBindingRefusalStaleDestroyed
  CompleteRunning
  MarkCompleted
  ShutdownRunning
)

resume_covered_actions=(
  RecoverMemberSessionBindingReplacingRunning
  BeginExplicitResumeStopped
  SettleExplicitResumePreparationStopped
  BeginExplicitResumeReadinessStopped
  SettleExplicitResumeReadinessStopped
  AuthorizeExplicitResumeMemberRunning
  AuthorizeExplicitResumeMemberLiveRunning
  AuthorizeExplicitResumeMemberJoinRunning
  ClassifyExplicitResumeMemberLiveRevivable
  ClassifyExplicitResumeMemberLiveMissing
  SettleExplicitResumeMemberRunning
  SettleExplicitResumeMemberReplacedRunning
  BeginExplicitResumeReadinessRunning
  SettleExplicitResumeReadinessRunning
  BeginExplicitResumeTopologyRunning
  SettleExplicitResumeTopologyRunning
  FinishExplicitResumeRunning
)

edge_covered_actions=(
  RecoverRosterWiringRunning
  WireMembersRunning
  ObserveMemberRetirementArchivedLivePreservingTopology
  ResolveWiringEdgeRealizationRealizedRunning
  ResolveWiringEdgeRealizationOwedRunning
  ResolveWiringEdgeRealizationStaleRunning
)

seat_covered_actions=(
  BeginSpawnExecFresh
  CommitDormantSeatRunning
  CommitSpawnMembershipFresh
)

wake2_covered_actions=(
  ClassifyPeerNoticeRunning
  WakeMemberAuthorizedRunning
  WakeMemberRefusedRunning
  RetireRunningPreservingBindingArchiveOnly
)

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/member-residency-audit.XXXXXX")"
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

# The VIEW leaves out model_step_count and topology_epoch, so the generated
# model must read them nowhere but in their own updates, Init, the UNCHANGED
# frames and the generated profiles' state constraints (which the audit does
# not use). Prints their positions in the `vars` tuple.
view_counter_positions() {
  MODEL="${spec_dir}/model.tla" python3 - <<'PY'
import os, re, sys
lines = open(os.environ["MODEL"]).read().split("\n")
allowed = {
    "model_step_count": [
        r"VARIABLES ",
        r"vars == << ",
        r"    /\\ model_step_count = 0$",
        r"    /\\ model_step_count' = model_step_count \+ 1$",
        r"CiStateConstraint == ",
        r"DeepStateConstraint == ",
    ],
    "topology_epoch": [
        r"VARIABLES ",
        r"vars == << ",
        r"    /\\ topology_epoch = 0$",
        r"    /\\ topology_epoch' = ",
        r"UnchangedFrame_[0-9a-f]+ == UNCHANGED << ",
    ],
}
for name, patterns in allowed.items():
    word = re.compile(r"\b" + name + r"\b")
    for number, line in enumerate(lines, 1):
        if word.search(line) and not any(re.match(p, line) for p in patterns):
            sys.exit(f"error: the audit's VIEW leaves out {name}, but model.tla line {number} reads it: {line[:160]}")
tuples = [line for line in lines if line.startswith("vars == << ") and line.endswith(" >>")]
if len(tuples) != 1:
    sys.exit("error: expected one `vars == << ... >>` tuple in model.tla")
names = [name.strip() for name in tuples[0][len("vars == << "):-len(" >>")].split(",")]
for name in allowed:
    if names.count(name) != 1:
        sys.exit(f"error: expected {name} once in the vars tuple of model.tla")
print(names.index("model_step_count") + 1, names.index("topology_epoch") + 1)
PY
}
view_positions="$(view_counter_positions)"
step_index="${view_positions% *}"
epoch_index="${view_positions#* }"

# usage: write_cfg <run> <file> <extra invariant lines> <extra property lines>
# where <run> is seed, wake, retire, terminal, resume, edge, seat or wake2.
write_cfg() {
  local run="$1" cfg="$2" invariants="$3" properties="$4"
  local spec run_properties run_invariants max_attempts section_end
  case "${run}" in
    seed)
      spec="AuditSeedSpec"
      max_attempts=1
      run_properties="
  AuditSeedRecoverKeepsPhase
  AuditAttemptEndsClosed
  AuditPlanLeavesSealable"
      run_invariants="
  AuditWakingClosable"
      ;;
    wake)
      spec="AuditWakeSpec"
      max_attempts=2
      run_properties="
  AuditRecoverKeepsPhase
  AuditAttemptEndsClosed
  AuditLifecycleNeverWakes
  AuditWakeNeverSkipsWaking
  AuditSucceededLeavesEndpoint
  AuditReleaseEpochFence
  AuditBindingRefusalFenced
  AuditFreshCreateOnlyUnborn"
      run_invariants="
  AuditWakingClosable
  AuditWakeExactlyOne
  AuditWakeClosesLiveUnlessEndpointOwed
  AuditReleaseVerdictMatchesArm"
      ;;
    retire)
      spec="AuditRetireSpec"
      max_attempts=1
      run_properties="
  AuditAttemptEndsClosed
  AuditRetireRoutesOnlyRegistered
  AuditDestroyFencesRotation
  AuditReseatUnbornIffNeverBorn"
      run_invariants="
  AuditWakingClosable
  AuditRetirementArchivable"
      ;;
    terminal)
      spec="AuditTerminalSpec"
      max_attempts=1
      run_properties="
  AuditTerminalRecoverKeepsPhase
  AuditAttemptEndsClosed"
      run_invariants="
  AuditWakingClosable"
      ;;
    resume)
      spec="AuditResumeSpec"
      max_attempts=2
      run_properties="
  AuditResumeRecoverKeepsPhase
  AuditAttemptEndsClosed
  AuditResumeReservesBeforeReadiness"
      run_invariants="
  AuditWakingClosable
  AuditResumeJoinable
  AuditResumeStagesHoldNoWork"
      ;;
    edge)
      spec="AuditEdgeSpec"
      max_attempts=2
      run_properties="
  AuditEdgeRecoverKeepsPhase
  AuditAttemptEndsClosed"
      run_invariants="
  AuditWakingClosable"
      ;;
    seat)
      spec="AuditSeatSpec"
      max_attempts=2
      run_properties="
  AuditSeatRecoverKeepsPhase
  AuditAttemptEndsClosed
  AuditSeatLeavesDormant"
      run_invariants="
  AuditWakingClosable"
      ;;
    wake2)
      spec="AuditWake2Spec"
      max_attempts=2
      run_properties="
  AuditAttemptEndsClosed"
      run_invariants="
  AuditWakingClosable"
      ;;
    *)
      echo "error: unknown audit run '${run}'" >&2
      exit 1
      ;;
  esac
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "  AgentIdentityValues = {}" '  AgentIdentityValues = {"agentidentity_1", "agentidentity_2"}' "${cfg}"
  replace_exact_line "  AgentRuntimeIdValues = {}" '  AgentRuntimeIdValues = {"agentruntimeid_1", "agentruntimeid_2", "agentruntimeid_3"}' "${cfg}"
  replace_exact_line "  FenceTokenValues = {1}" "  FenceTokenValues = {1, 2, 3}" "${cfg}"
  replace_exact_line "  GenerationValues = {1}" "  GenerationValues = {0, 1}" "${cfg}"
  replace_exact_line "  MobIdValues = {}" '  MobIdValues = {"mobid_1"}' "${cfg}"
  replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1", "sessionid_2", "sessionid_3"}' "${cfg}"
  replace_exact_line "  StringValues = {}" '  StringValues = {"profile_1", "model_1", "digest_1", "reason_1", "code_1", "objective_1"}' "${cfg}"
  replace_exact_line "  ResumeAttemptIdValues = {}" '  ResumeAttemptIdValues = {"resumeattemptid_1"}' "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditStepIndex = ${step_index}
  AuditEpochIndex = ${epoch_index}
  AuditMaxAttempts = ${max_attempts}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION ${spec}" "${cfg}"
  replace_exact_line "INVARIANTS" "PROPERTIES${run_properties}${properties}
INVARIANTS" "${cfg}"
  # The audit's invariants follow the generated ones: TLC reports the first
  # invariant a state breaks in the config's order, so a mutant seeded
  # against a generated invariant is refused by that invariant even where
  # its state also breaks an audit invariant.
  section_end="$(awk '/^INVARIANTS$/ { inside = 1; next } inside && /^[A-Z]/ { print; exit }' "${cfg}")"
  if [[ "${section_end}" != "CONSTRAINTS" ]]; then
    echo "error: expected the CONSTRAINTS section right after INVARIANTS in ${ci_cfg}, found '${section_end}'" >&2
    exit 1
  fi
  replace_exact_line "CONSTRAINTS" "${run_invariants#$'\n'}${invariants}
VIEW AuditView
CONSTRAINTS" "${cfg}"
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
  tlc_run_capped member_residency_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" member_residency_audit.tla || status=$?
  return "${status}"
}

# usage: check_space <run> <states of its first green run> <covered action...>
# runs <run> over its whole space with coverage and checks the outcome, the
# distinct-state floor (90% of the first green run) and the covered actions.
check_space() {
  local run="$1" whole_space="$2"
  shift 2
  local floor=$(( whole_space * 9 / 10 ))
  local cfg="${work_dir}/${run}.cfg"
  write_cfg "${run}" "${cfg}" "" ""
  local status=0
  run_tlc "${run}" "${cfg}" -coverage 1 ${tlc_args[@]+"${tlc_args[@]}"} || status=$?
  local log="${work_dir}/${run}.log"
  cat "${log}"
  if [[ "${status}" != "0" ]] \
    || ! grep -q "Model checking completed. No error has been found." "${log}"; then
    echo "error: member residency audit ${run} run failed (tlc exit ${status})" >&2
    exit 1
  fi

  local distinct
  distinct="$(sed -nE 's/^[0-9]+ states generated, ([0-9]+) distinct states found, 0 states left on queue\.$/\1/p' \
    "${log}" | tail -1)"
  if [[ -z "${distinct}" ]]; then
    echo "error: the ${run} run reported no final distinct-state count" >&2
    exit 1
  fi
  if (( distinct < floor )); then
    echo "error: ${run} run: ${distinct} distinct states, below the floor ${floor} (90% of ${whole_space})" >&2
    exit 1
  fi
  echo "${run} distinct states: ${distinct} (floor ${floor})"

  # TLC prints a coverage block every minute and one at the end; read the
  # last. An action's line is
  # `<Action line L, col C to line L, col C of module model>: D:T`, with D the
  # new distinct states it found and T the successor states it generated; an
  # action that fires only into known states has D = 0, so T counts. An arm
  # applied with a state-level argument (the stale feedback for the last
  # minted attempt) stays its own action, and TLC adds the application's
  # location before the `>`: `... of module model (L C L C)>: D:T`.
  local coverage="${work_dir}/${run}-coverage.txt"
  awk '/^The coverage statistics at / { block = ""; next } { block = block $0 "\n" } END { printf "%s", block }' \
    "${log}" > "${coverage}"
  local action generated
  for action in "$@"; do
    generated="$(awk -v head="<${action} line " '
      index($0, head) == 1 && $0 ~ / of module (model|member_residency_audit)( \([0-9]+ [0-9]+ [0-9]+ [0-9]+\))?>: [0-9]+:[0-9]+$/ {
        split($NF, counts, ":"); total += counts[2]; found = 1
      }
      END { if (found) print total }' "${coverage}")"
    if [[ -z "${generated}" || "${generated}" == "0" ]]; then
      echo "error: ${run} run: ${action} never fired (coverage '${generated:-absent}')" >&2
      exit 1
    fi
    echo "covered ${action}: ${generated} successor states"
  done
}

tlc_args=("$@")
check_space seed "${seed_states}" "${seed_covered_actions[@]}"
check_space wake "${wake_states}" "${wake_covered_actions[@]}"
check_space retire "${retire_states}" "${retire_covered_actions[@]}"
check_space terminal "${terminal_states}" "${terminal_covered_actions[@]}"
check_space resume "${resume_states}" "${resume_covered_actions[@]}"
check_space edge "${edge_states}" "${edge_covered_actions[@]}"
check_space seat "${seat_states}" "${seat_covered_actions[@]}"
check_space wake2 "${wake2_states}" "${wake2_covered_actions[@]}"

# Each witness is <run>:<kind>:<name>: a state goal's negation joins the
# invariants, a step goal's negation ([][~step]_vars) joins the properties.
for witness in \
  "seed:Invariant:SealedDormant" "seed:Invariant:SealedEndpointPending" \
  "wake:Action property:WokeLive" "wake:Action property:Released" \
  "wake:Action property:StopCloses" "wake:Invariant:StaleInStopped" \
  "wake:Invariant:Drift" "wake:Invariant:BrokenWake" \
  "wake:Action property:CommsLessWokeLive" \
  "retire:Action property:RetireCloses" "retire:Action property:ArchiveOnlyArchived" \
  "retire:Action property:DestroyBeatsRotation" "retire:Action property:DestroyBeatsHostPending" \
  "retire:Action property:RotatedStopped" \
  "resume:Action property:ResumeJoins" "resume:Action property:ResumeReservesLive" \
  "resume:Action property:ResumeFinishedLive" "resume:Action property:ResumeJoinsAfterReadiness" \
  "edge:Invariant:HeldEdge" "edge:Invariant:EdgeRepairOwed" \
  "wake:Action property:UnbornCreated" "retire:Action property:UnbornArchived" \
  "retire:Action property:ReseatCreated" "retire:Invariant:RotationGrant" \
  "seat:Invariant:SeatUnborn" "seat:Action property:RotatedUnborn" \
  "seat:Invariant:BothSeated" "seat:Invariant:SeatEndpointPending" \
  "seat:Action property:Rotated" "seat:Action property:ReleasedSpawn" \
  "wake2:Action property:CrossNotice" "wake2:Action property:RetireBesideWaking"; do
  run="${witness%%:*}"
  rest="${witness#*:}"
  kind="${rest%%:*}"
  name="${rest#*:}"
  cfg="${work_dir}/witness-${name}.cfg"
  if [[ "${kind}" == "Invariant" ]]; then
    write_cfg "${run}" "${cfg}" "
  NotAuditWitness${name}" ""
  else
    write_cfg "${run}" "${cfg}" "" "
  NotAuditWitness${name}"
  fi
  status=0
  run_tlc "witness-${name}" "${cfg}" "$@" || status=$?
  log="${work_dir}/witness-${name}.log"
  if ! grep -q "Error: ${kind} NotAuditWitness${name} is violated" "${log}"; then
    cat "${log}"
    echo "error: witness ${name} is not reachable (tlc exit ${status})" >&2
    exit 1
  fi
  depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
  echo "witness ${name} reached in ${depth} states"
done

if [[ "${run_mutants}" == "true" ]]; then
  # Shared edit helpers for the mutants: each mutant's python edit runs after
  # this prelude, rewrites the copied model.tla in place and must find each
  # anchor it names.
  read -r -d '' mutant_prelude <<'PY' || true
import os, re
path = os.environ["MODEL"]
text = open(path).read()

def _span(arm):
    found = list(re.finditer(r"^" + re.escape(arm) + r"(\([^)]*\))? ==\n.*?\n\n", text, re.S | re.M))
    assert len(found) == 1, f"expected one definition of {arm}, found {len(found)}"
    return found[0].start(), found[0].end()

def _replace_body(arm, edit):
    global text
    start, end = _span(arm)
    text = text[:start] + edit(text[start:end]) + text[end:]

def keep_var(arm, var):
    """The arm leaves `var` as it was instead of updating it."""
    def edit(body):
        lines = body.split("\n")
        hits = [i for i, line in enumerate(lines) if line.startswith("    /\\ " + var + "' = ")]
        assert len(hits) == 1, f"expected one update of {var} in {arm}, found {len(hits)}"
        lines[hits[0]] = "    /\\ " + var + "' = " + var
        return "\n".join(lines)
    _replace_body(arm, edit)

def drop_guard(arm, needle):
    """Delete the one guard line of the arm that contains `needle`."""
    def edit(body):
        lines = body.split("\n")
        hits = [i for i, line in enumerate(lines)
                if line.startswith("    /\\ ") and "'" not in line and needle in line]
        assert len(hits) == 1, f"expected one guard with {needle!r} in {arm}, found {len(hits)}"
        del lines[hits[0]]
        return "\n".join(lines)
    _replace_body(arm, edit)

def add_guard(arm, guard):
    """Add a guard right after the arm's phase guard."""
    def edit(body):
        lines = body.split("\n")
        hits = [i for i, line in enumerate(lines) if line.startswith("    /\\ phase = ")]
        assert len(hits) == 1, f"expected one phase guard in {arm}, found {len(hits)}"
        lines.insert(hits[0] + 1, "    /\\ " + guard)
        return "\n".join(lines)
    _replace_body(arm, edit)

def rewrite_frame(arm, updates):
    """Replace the arm's UNCHANGED frame with explicit updates of `updates`
    (var -> right-hand side) and an UNCHANGED of every other frame variable."""
    def edit(body):
        frames = re.findall(r"^    /\\ (UnchangedFrame_[0-9a-f]+)$", body, re.M)
        assert len(frames) == 1, f"expected one UNCHANGED frame in {arm}"
        frame = frames[0]
        definition = re.findall(r"^" + frame + r" == UNCHANGED << (.*) >>$", text, re.M)
        assert len(definition) == 1, "expected one definition of " + frame
        names = [name.strip() for name in definition[0].split(",")]
        for var in updates:
            assert var in names, f"{arm} no longer leaves {var} unchanged"
        kept = ", ".join(name for name in names if name not in updates)
        explicit = "\n".join("    /\\ " + var + "' = " + rhs for var, rhs in updates.items())
        return body.replace("    /\\ " + frame, explicit + "\n    /\\ UNCHANGED << " + kept + " >>")
    _replace_body(arm, edit)

def replace_in_update(arm, var, old, new):
    """Replace the one occurrence of `old` in the arm's update of `var`."""
    def edit(body):
        lines = body.split("\n")
        hits = [i for i, line in enumerate(lines) if line.startswith("    /\\ " + var + "' = ")]
        assert len(hits) == 1, f"expected one update of {var} in {arm}, found {len(hits)}"
        assert lines[hits[0]].count(old) == 1, f"expected one {old!r} in the {var} update of {arm}"
        lines[hits[0]] = lines[hits[0]].replace(old, new)
        return "\n".join(lines)
    _replace_body(arm, edit)

def replace_in_operator(operator, old, new):
    """Replace the one occurrence of `old` in a one-line operator definition."""
    global text
    found = list(re.finditer(r"^" + re.escape(operator) + r"\(.*$", text, re.M))
    assert len(found) == 1, f"expected one definition line of {operator}, found {len(found)}"
    line = found[0].group(0)
    assert line.count(old) == 1, f"expected one {old!r} in {operator}, found {line.count(old)}"
    text = text[:found[0].start()] + line.replace(old, new) + text[found[0].end():]

def save():
    open(path, "w").write(text)
PY

  # usage: mutate <run> <name> <expected refusal regex> <python edit> [extra tlc args...]
  mutate() {
    local run="$1" name="$2" refusal="$3" edit="$4"
    shift 4
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}"/*.tla "${dir}/"
    printf '%s\n%s\nsave()\n' "${mutant_prelude}" "${edit}" | MODEL="${dir}/model.tla" python3 -
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${run}" "${cfg}" "" ""
    local status=0
    (cd "${dir}" && run_tlc "mutant-${name}" "${cfg}" "$@") || status=$?
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -qE "${refusal}" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by '${refusal}' (tlc exit ${status})" >&2
      exit 1
    fi
    echo "mutant ${name} refused by $(grep -oE "${refusal}" "${log}" | head -1)"
  }

  # An unseating replay that keeps the residency phase.
  mutate terminal replay_unseat_keeps_phase \
    "Error: Invariant materialization_domain_is_session_bound_membership is violated" \
    'keep_var("RecoverRosterMemberRetiredRunning", "member_materialization")' "$@"
  # The one seating arm binds the member without seating it Dormant.
  mutate wake bind_seats_no_phase \
    "Error: Invariant local_session_bound_member_is_live_or_wakeable is violated" \
    'keep_var("RecoverMemberSessionBindingFreshRunning", "member_materialization")' "$@"
  # A terminal classification records Broken and keeps the phase.
  mutate wake broken_keeps_phase \
    "Error: Invariant broken_member_has_no_phase is violated" \
    'keep_var("ClassifyMemberLiveMaterializationTerminal", "member_materialization")' "$@"
  # The seal without its endpoint guard.
  mutate seed seal_without_endpoint_guard \
    "Error: Invariant dormant_member_has_durable_endpoint is violated" \
    'drop_guard("SealBootMaterializationPlan", "member_boot_obligation_owed")' "$@"
  # The seal's exception: registering an endpoint keeps EndpointPending owed.
  mutate seed endpoint_keeps_pending \
    "Error: Invariant endpoint_pending_member_has_no_endpoint is violated" \
    'keep_var("RecoverMemberPeerEndpointFreshRunning", "member_boot_obligation_owed")' "$@"
  # An abandoned wake keeps its binding request.
  mutate wake abandon_keeps_binding_request \
    "Error: Invariant dormant_member_requests_no_runtime_binding is violated" \
    'keep_var("AbandonMemberWakeDormantRunning", "member_startup_binding_requested")' "$@"
  # The runtime-ready observation records no epoch.
  mutate wake ready_records_no_epoch \
    "Error: Invariant live_member_records_its_epoch is violated" \
    'keep_var("ObserveRuntimeReady", "member_materialization_epoch")' "$@"
  # An authorized wake records no attempt.
  mutate wake wake_records_no_attempt \
    "Error: Invariant waking_is_one_attempt is violated" \
    'keep_var("WakeMemberAuthorizedRunning", "member_wake_attempts")' "$@"
  # A retirement start keeps the member's open wake (a Waking member
  # retires through the archive-only twin).
  mutate retire retire_start_keeps_wake \
    "Error: Invariant retiring_incarnation_never_waking is violated" \
    'for var in ("member_materialization", "member_wake_attempts", "member_wake_bound", "member_materialization_epoch", "member_boot_obligation_owed"):
    keep_var("RetireRunningPreservingBindingArchiveOnly", var)' "$@"
  # The build seed turns the composition non-authoritative over an open
  # attempt.
  mutate seed seed_over_open_attempt \
    "Error: Invariant candidate_composition_never_wakes is violated" \
    'drop_guard("SeedMemberResidencyRunning", "member_wake_attempts")' "$@"
  # An authorized wake mints no attempt id.
  mutate wake wake_mints_nothing \
    "Error: Invariant waking_has_an_open_attempt is violated" \
    'keep_var("WakeMemberAuthorizedRunning", "member_wake_attempt_sequence")' "$@"
  # A wake that closes Live keeps its attempt.
  mutate wake live_keeps_attempt \
    "Error: Invariant attempt_and_epoch_follow_the_phase is violated" \
    'keep_var("ResolveMemberRevivalSucceededRunningLocal", "member_wake_attempts")' "$@"
  # A Stop that leaves its Waking members waking.
  mutate wake stop_keeps_wakes \
    "Error: Invariant only_a_running_mob_wakes is violated" \
    'for var in ("member_materialization", "member_wake_attempts", "member_wake_bound", "member_materialization_epoch", "member_startup_binding_requested", "member_startup_runtime_ready", "member_startup_ready"):
    keep_var("StopRunning", var)' "$@"
  # The WakeMember action property: an authorized wake moves Dormant straight
  # to Live (with no attempt and no owed obligation left, so only the
  # property can refuse it).
  mutate wake wake_skips_waking \
    "Error: Action property AuditWakeNeverSkipsWaking is violated" \
    'replace_in_update("WakeMemberAuthorizedRunning", "member_materialization", "\"Waking\"", "\"Live\"")
keep_var("WakeMemberAuthorizedRunning", "member_wake_attempts")
keep_var("WakeMemberAuthorizedRunning", "member_wake_attempt_sequence")
rewrite_frame("WakeMemberAuthorizedRunning", {
    "member_boot_obligation_owed": "MapRemove(member_boot_obligation_owed, agent_identity)",
})' "$@"
  # A recovery arm that writes Dormant over a Live member (with its epoch
  # and startup ladder cleared and, after the seal, its endpoint present, so
  # only the property can refuse it).
  mutate wake recover_writes_dormant \
    "Error: Action property AuditRecoverKeepsPhase is violated" \
    'mutable = "(agent_identity \\in DOMAIN member_materialization /\\ member_materialization[agent_identity] = \"Live\" /\\ (boot_plan_sealed = FALSE \\/ agent_identity \\in DOMAIN member_peer_endpoints))"
rewrite_frame("RecoverMemberSessionBindingAlreadyCurrentRunning", {
    "member_materialization": "IF " + mutable + " THEN MapSet(member_materialization, agent_identity, \"Dormant\") ELSE member_materialization",
    "member_materialization_epoch": "IF " + mutable + " THEN MapRemove(member_materialization_epoch, agent_identity) ELSE member_materialization_epoch",
    "member_startup_binding_requested": "IF " + mutable + " THEN member_startup_binding_requested \\ {agent_runtime_id} ELSE member_startup_binding_requested",
    "member_startup_runtime_ready": "IF " + mutable + " THEN member_startup_runtime_ready \\ {agent_runtime_id} ELSE member_startup_runtime_ready",
    "member_startup_ready": "IF " + mutable + " THEN member_startup_ready \\ {agent_runtime_id} ELSE member_startup_ready",
})' "$@"
  # A joined wake replaces the open attempt with a fresh one.
  mutate wake join_replaces_attempt \
    "Error: Action property AuditAttemptEndsClosed is violated" \
    'rewrite_frame("WakeMemberJoinedRunning", {
    "member_wake_attempt_sequence": "member_wake_attempt_sequence + 1",
    "member_wake_attempts": "MapSet(member_wake_attempts, agent_identity, member_wake_attempt_sequence + 1)",
})' "$@"
  # The wake refusal helper ignores the Lifecycle input class.
  mutate wake lifecycle_wakes \
    "Error: Action property AuditLifecycleNeverWakes is violated" \
    'replace_in_operator("member_wake_refusal", "(input_class = \"Lifecycle\")", "FALSE")' "$@"
  # The shell's abandon is refused before the Bound step (the ENABLED form).
  mutate wake abandon_needs_bound \
    "Error: Invariant AuditWakingClosable is violated" \
    'add_guard("AbandonMemberWakeDormantRunning", "(agent_identity \\in member_wake_bound)")' "$@"
  # WakeMember totality: Joined without its Waking guard overlaps the others.
  mutate wake joined_overlaps \
    "Error: Invariant AuditWakeExactlyOne is violated" \
    'drop_guard("WakeMemberJoinedRunning", "= Some(\"Waking\"))")' "$@"
  # The refusal helper refuses every member as not a member. The helper's
  # `phase` parameter renders as arg_phase (it would shadow the machine's
  # phase variable).
  mutate wake helper_refuses_members \
    "Assumption .* is false" \
    'replace_in_operator("member_wake_refusal", "(arg_phase = None)", "TRUE")' "$@"
  # An archive that keeps the archived identity's wiring edges.
  mutate edge archive_keeps_edges \
    "Error: Invariant wiring_edges_connect_seated_members is violated" \
    'keep_var("ObserveMemberRetirementArchivedLive", "wiring_edges")' "$@"
  # A preserved-topology archive that keeps the archived end's owed edge
  # repair: the repair then names an unseated member.
  mutate edge preserved_archive_keeps_repair \
    "Error: Invariant owed_edge_repairs_join_seated_members is violated" \
    'keep_var("ObserveMemberRetirementArchivedLivePreservingTopology", "owed_edge_repairs")' "$@"
  # An archive that keeps the archived member's unborn seat session fact.
  mutate retire archive_keeps_unborn \
    "Error: Invariant seat_session_unborn_is_seated is violated" \
    'keep_var("ObserveMemberRetirementArchivedLive", "member_seat_session_unborn")' "$@"
  # A restore that binds an unborn seat, whose session was never created.
  mutate wake bound_without_born_guard \
    "Error: Invariant seat_session_unborn_is_never_bound is violated" \
    'drop_guard("ResolveMemberRevivalBoundRunning", "member_seat_session_unborn")' "$@"
  # The fresh create for a member whose session was created before.
  mutate wake fresh_create_without_unborn \
    "Error: Action property AuditFreshCreateOnlyUnborn is violated" \
    'drop_guard("ResolveMemberRevivalSessionCreatedRunning", "member_seat_session_unborn")' "$@"
  # An unborn seat that records no never-created session.
  mutate seat seat_records_no_session \
    "Error: Invariant unborn_seat_session_is_recorded is violated" \
    'keep_var("CommitDormantSeatRunning", "unborn_seat_sessions")' "$@"
  # A seat naming a session no build created, seated born (its absence would
  # read as lost).
  mutate retire reseat_never_born_seats_born \
    "Error: Action property AuditReseatUnbornIffNeverBorn is violated" \
    'drop_guard("CommitDormantSeatRunning", "(session_unborn = TRUE)")' "$@"
  # A resume seat of a lost session seated unborn (created afresh, empty).
  mutate retire lost_session_seats_unborn \
    "Error: Action property AuditReseatUnbornIffNeverBorn is violated" \
    'drop_guard("CommitDormantSeatRunning", "(fresh_launch = TRUE)")' "$@"
  # A spawn-profile grant recorded for a seated member outside a rotation
  # (it would outlive the check and stand in for a later rotation's role).
  mutate retire grant_to_a_seated_member \
    "Error: Invariant spawn_profile_authority_is_a_pending_grant is violated" \
    'drop_guard("AuthorizeSpawnProfileRunning", "member_rotation_session_ids")' "$@"
  # A replayed seat that keeps a grant recorded before it.
  mutate retire replay_seat_keeps_grant \
    "Error: Invariant spawn_profile_authority_is_a_pending_grant is violated" \
    'keep_var("RecoverRosterMemberRunning", "spawn_profile_authority_profile_names")' "$@"
  # The rotation intent recorded for a member that is not Retiring.
  mutate seat rotation_without_retirement \
    "Error: Invariant rotation_intent_is_a_retiring_member is violated" \
    'drop_guard("BeginMemberRotationRunning", "= Some(\"Retiring\"))")' "$@"
  # A dormant seat that requests a runtime binding.
  mutate seat seat_requests_binding \
    "Error: (Invariant dormant_member_requests_no_runtime_binding|Action property AuditSeatLeavesDormant) is violated" \
    'replace_in_update("CommitDormantSeatRunning", "member_startup_binding_requested", "member_startup_binding_requested \\ {agent_runtime_id}", "member_startup_binding_requested \\cup {agent_runtime_id}")' "$@"
  # A dormant seat without its peer-id guard takes another member's peer id.
  mutate seat seat_duplicate_peer_id \
    "Error: Invariant (member_peer_id_ownership_is_global_across_generations|live_member_peer_ids_are_unique) is violated" \
    'drop_guard("CommitDormantSeatRunning", "mob_machine_member_peer_id_available_for_identity")' "$@"
  # A local wake closes Live without a registered endpoint.
  mutate wake succeeded_without_endpoint \
    "Error: Action property AuditSucceededLeavesEndpoint is violated" \
    'drop_guard("ResolveMemberRevivalSucceededRunningLocal", "member_peer_endpoints")' "$@"
  # The epoch fence: a release demotes whatever its epoch. The epoch
  # conjunct of the Released guard and of both its folds (the demotion and
  # the epoch clear) becomes TRUE, so the demoted state is a valid one and
  # only the property can refuse it.
  mutate wake release_ignores_epoch \
    "Error: Action property AuditReleaseEpochFence is violated" \
    'epoch = r"\(\(IF \(member_id \\in DOMAIN (member_materialization_epoch|acc)\) THEN Some\(\(IF member_id \\in DOMAIN \1 THEN \1\[member_id\] ELSE None\)\) ELSE None\) = Some\((outer_)?runtime_epoch_id\)\)"
def edit(body):
    found = re.findall(epoch, body)
    assert len(found) == 1, f"expected one epoch conjunct in the Released guard, found {len(found)}"
    return re.sub(epoch, "TRUE", body)
_replace_body("ReleaseMemberMaterializationReleasedRunning", edit)
folds = re.findall(r"^(ReleaseMemberMaterializationReleasedRunning_ForEach\d+_member_materialization(?:_epoch)?)\(", text, re.M)
assert len(folds) == 2, f"expected the two folds of the Released arm, found {folds}"
for fold in folds:
    found = list(re.finditer(r"^" + re.escape(fold) + r"\(.*$", text, re.M))
    assert len(found) == 1, f"expected one definition line of {fold}, found {len(found)}"
    line = found[0].group(0)
    assert len(re.findall(epoch, line)) == 1, f"expected one epoch conjunct in {fold}"
    text = text[:found[0].start()] + re.sub(epoch, "TRUE", line) + text[found[0].end():]' "$@"
  # A local wake that needs an endpoint whatever it owes: a member owing no
  # endpoint cause is held Waking for one.
  mutate wake succeeded_needs_endpoint \
    "Error: Invariant AuditWakeClosesLiveUnlessEndpointOwed is violated" \
    'add_guard("ResolveMemberRevivalSucceededRunningLocal", "(agent_identity \\in DOMAIN member_peer_endpoints)")' "$@"
  # The release verdict misses a Waking member (its wake in flight would be
  # answered as nothing materialized).
  mutate wake release_verdict_misses_waking \
    "Assumption .* is false" \
    'replace_in_operator("member_release_verdict", "(residency = Some(\"Waking\"))", "FALSE")' "$@"
  # The routed binding refusal without its attempt fence: a late refusal of
  # an earlier attempt's request ends a newer wake.
  mutate wake binding_refusal_unfenced \
    "Error: Action property AuditBindingRefusalFenced is violated" \
    'drop_guard("ResolveRuntimeBindingRefusalRunning", "member_wake_attempts")' "$@"
  # The fence for a wake's request only: a spawn-era refusal (no attempt)
  # still ends a wake opened after the spawn.
  mutate wake spawn_refusal_unfenced \
    "Error: Action property AuditBindingRefusalFenced is violated" \
    'drop_guard("ResolveRuntimeBindingRefusalRunning", "member_wake_attempts")
add_guard("ResolveRuntimeBindingRefusalRunning", "((attempt = None) \\/ ((IF (agent_identity \\in DOMAIN member_wake_attempts) THEN Some(member_wake_attempts[agent_identity]) ELSE None) = attempt))")' "$@"
  # The release verdict drifts from the Released arm on the registration
  # facts: it answers Release for a Live, unpinned member whose registration
  # release would be Stale. It keeps the pin, so the release ASSUMEs (which
  # TLC evaluates before any invariant) still hold and the property refuses it.
  mutate wake release_verdict_ignores_registration \
    "Error: Invariant AuditReleaseVerdictMatchesArm is violated" \
    'replace_in_operator("member_release_verdict", "(releasable = TRUE)", "((releasable = TRUE) \\/ (wired_to_placed = FALSE))")' "$@"
  # The release predicate without its placed-peer pin: a member wired to a
  # placed member is released (no run places a member; the ASSUME over the
  # helper's whole domain refuses it).
  mutate wake release_ignores_placed_pin \
    "Assumption .* is false" \
    'replace_in_operator("member_registration_releasable", "(wired_to_placed = FALSE)", "TRUE")' "$@"
  # The boot plan leaves an endpoint-less member Dormant owing nothing, so
  # the seal would refuse the whole start.
  mutate seed stay_dormant_owes_nothing \
    "Error: Action property AuditPlanLeavesSealable is violated" \
    'keep_var("PlanBootMaterializationStayDormant", "member_boot_obligation_owed")' "$@"
  # The routed retirement start without its Dormant guard routes a
  # runtime retire for a member with no registration.
  mutate retire archive_only_routes \
    "Error: Action property AuditRetireRoutesOnlyRegistered is violated" \
    'drop_guard("RetireRunningReleasing", "# Some(\"Dormant\")")' "$@"
  # An archive terminal keeps the runtime's archive-only mark.
  mutate retire archive_keeps_archive_only \
    "Error: Invariant archive_only_retirement_is_retiring is violated" \
    'keep_var("ObserveMemberRetirementArchivedLive", "runtime_retire_archive_only")' "$@"
  # The plain archive terminals wait for the rotation even under an
  # admitted destroy.
  mutate retire destroy_waits_on_rotation \
    "Error: Invariant AuditRetirementArchivable is violated" \
    'for arm in ("ObserveMemberRetirementArchivedLive", "ObserveMemberRetirementArchivedRetired", "ObserveMemberRetirementArchivedLiveStopped", "ObserveMemberRetirementArchivedRetiredStopped"):
    drop_guard(arm, "member_rotation_session_ids")
    add_guard(arm, "((agent_identity \\in DOMAIN member_rotation_session_ids) = FALSE)")' "$@"
  # The rotation commits under an admitted destroy.
  mutate retire commit_rotation_after_destroy \
    "Error: Action property AuditDestroyFencesRotation is violated" \
    'drop_guard("CommitMemberRotationLiveRunning", "destroy_admitted")' "$@"
  # The destroy keeps the pending host preparations.
  mutate retire destroy_keeps_host_pending \
    "Error: Invariant destroyed_mob_holds_no_member_lifecycle is violated" \
    'keep_var("DestroyMob", "member_host_preparation_pending")' "$@"
  # The join refused once the resume settled a readiness: the lift of a
  # stopped mob, whose readiness ran before its Resume, could not join the
  # wakes its member plan opened.
  mutate resume join_refused_after_readiness \
    "Error: Invariant AuditResumeJoinable is violated" \
    'add_guard("AuthorizeExplicitResumeMemberJoinRunning", "explicit_resume_readiness_settled = FALSE")' "$@"
  # A join admitted while the topology is pending (the topology guard keeps
  # only its settled conjunct): member work the topology stage never waits
  # for.
  mutate resume join_while_topology_pending \
    "Error: Invariant AuditResumeStagesHoldNoWork is violated" \
    'drop_guard("AuthorizeExplicitResumeMemberJoinRunning", "explicit_resume_topology_pending")
add_guard("AuthorizeExplicitResumeMemberJoinRunning", "(explicit_resume_topology_settled = FALSE)")' "$@"
  # A join admitted once the topology settled, before the finish (the
  # topology guard keeps only its pending conjunct).
  mutate resume join_after_topology_settled \
    "Error: Invariant AuditResumeStagesHoldNoWork is violated" \
    'drop_guard("AuthorizeExplicitResumeMemberJoinRunning", "explicit_resume_topology_pending")
add_guard("AuthorizeExplicitResumeMemberJoinRunning", "(explicit_resume_topology_pending = FALSE)")' "$@"
  # A join admitted while a readiness is in flight.
  mutate resume join_during_readiness \
    "Error: Invariant AuditResumeStagesHoldNoWork is violated" \
    'drop_guard("AuthorizeExplicitResumeMemberJoinRunning", "explicit_resume_readiness_pending")' "$@"
  # The Dormant and Live reservation arms relaxed as the Join was (their
  # readiness guard keeps only its pending conjunct): a member the resume
  # rebuilds reserved after the readiness that should have settled it.
  mutate resume reserve_dormant_after_readiness \
    "Error: Action property AuditResumeReservesBeforeReadiness is violated" \
    'drop_guard("AuthorizeExplicitResumeMemberRunning", "explicit_resume_readiness_settled")
add_guard("AuthorizeExplicitResumeMemberRunning", "(explicit_resume_readiness_pending = FALSE)")' "$@"
  mutate resume reserve_live_after_readiness \
    "Error: Action property AuditResumeReservesBeforeReadiness is violated" \
    'drop_guard("AuthorizeExplicitResumeMemberLiveRunning", "explicit_resume_readiness_settled")
add_guard("AuthorizeExplicitResumeMemberLiveRunning", "(explicit_resume_readiness_pending = FALSE)")' "$@"
fi
echo "member residency audit passed over the whole reachable space of its domains"
