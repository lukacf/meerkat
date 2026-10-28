#!/usr/bin/env bash
# TLC audit of the MobMachine member lifecycle.
#
# usage: member_lifecycle_audit.sh [--mutants] [extra tlc args...]
#
# Derives the TLC configs from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's finite
# model values (one identity, two runtime ids, two fence tokens, generations
# 0..1, two sessions, the audit's strings) and its VIEW, which leaves out the
# two counters no guard, other variable's update or invariant of the
# generated model reads (model_step_count and topology_epoch). The runner
# first checks that the model still reads neither counter anywhere else, and
# fails otherwise. With the counters left out the reachable space is finite,
# so each run explores all of it, with no step bound. It checks two runs over
# their whole space with TLC coverage:
# - the lifecycle run (AuditSpec) with its four action properties and the two
#   invariants that check the reasons of its not-covered list;
# - the scope run (AuditScopeSpec) with its two RetireAtScope invariants and
#   the two destroy action properties.
# Each run must finish without an error, reach at least its floor of distinct
# states (so a guard that silently disables arms fails the lane instead of
# shrinking the space), and fire every action of its covered list (TLC
# coverage: each generated at least one successor state). It then reruns a
# space once per reachability witness, with that witness's negation as an
# extra invariant (a state goal) or action property (a step goal), and
# requires TLC to report exactly that violation, so the properties are not
# vacuous. With --mutants it then seeds ten defects into a copy of the
# generated model and requires TLC to refuse each: a progress arm without its
# observation_monotonic guard (AuditObservationMonotonic); the same guard
# rendered as a plain `<=` between option records, which TLC cannot evaluate
# (the renderer now writes it through OptionLe); the peer-only retirement
# start replay without its retirement_session_absent guard, which then drops
# a releasing retirement's session while its ingress detach is pending (the
# generated pending_session_ingress_detach_has_session_correlation); the
# stale-incarnation Retired replay with its old guard, which also takes a
# record naming the identity's current runtime with another generation and
# then prunes the Retiring runtime of an identity held for a respawn (the
# generated pending_respawn_topology_has_no_current_runtime); the
# RetireAtScope Current arm without its runtime and fence guards
# (AuditRetireAtScopeCoherent); and the StaleFenceToken arm without its
# mapped guard, which then overlaps NoCurrentIncarnation
# (AuditRetireAtScopeExactlyOne); the roster recovery keeping the
# predecessor's execution-health entries (AuditMintClearsExecutionHealth);
# a runtime destroy observation that ends the mob, one that leaves its
# runtime live, and one that resets the mob's run count
# (AuditRuntimeDestroyedRemovesOnlyItsRuntime).
# An anchor this script
# expects but cannot find fails the run. Each TLC run is held to the
# canonical lane's per-run cap (tlc_run_cap.sh) and fails closed as TLC
# INCOMPLETE.
set -euo pipefail

run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
# Each run's distinct-state floor is 90% of the whole space its first green
# run found, so a guard that silently disables arms fails the lane.
lifecycle_states=136308
min_distinct_states=$(( lifecycle_states * 9 / 10 ))
scope_states=10132
min_scope_distinct_states=$(( scope_states * 9 / 10 ))

# Every lifecycle-run arm the audit's domains can enable; each must fire.
# AuditStartRun is StartRunRunning held to one run at a time: TLC names a
# guarded arm after the audit's wrapper, so its coverage line is of this
# audit's module.
covered_actions=(
  RecoverRosterMemberRunning
  RecoverRosterMemberResetRunning
  RecoverMemberSessionBindingFreshRunning
  RecoverMemberSessionBindingReplacingRunning
  RecoverMemberSessionBindingAlreadyCurrentRunning
  ObserveMemberProgressChangedOpenRunning
  ObserveMemberProgressChangedIdleRunning
  ObserveMemberProgressUnchangedIdleRunning
  ObserveMemberProgressUnchangedOpenHealthyRunning
  ObserveMemberProgressUnchangedOpenDegradedRunning
  ObserveMemberProgressUnchangedOpenWedgedRunning
  ObserveMemberProgressStaleRunning
  RetireRunningReleasingArchiveOnly
  RetireRunningPreservingBindingArchiveOnly
  RetireRunningNoBinding
  RetireStoppedReleasingArchiveOnly
  RetireStoppedPreservingBindingArchiveOnly
  RetireStoppedNoBinding
  RetireAbsentRunning
  RetireAbsentStopped
  RetireMemberArchiveOnly
  RetireMemberPeerOnly
  SessionIngressDetachedForMobDestroyRunning
  SessionIngressDetachedForMobDestroyStopped
  RecoverRosterMemberRetiringRunning
  RecoverRosterMemberRetiringReplay
  RecoverRosterMemberRetiredRunning
  RecoverRosterMemberRetiredCurrentAlreadyAbsent
  RecoverRosterMemberRetiredStaleGeneration
  RecoverRosterMemberRetiredStaleIncarnation
  RecoverRosterMemberRetirementStartedReleasing
  RecoverRosterMemberRetirementStartedReleasingAlreadyApplied
  RecoverRosterMemberRetirementStartedPreservingBinding
  RecoverRosterMemberRetirementStartedPeerOnly
  RecoverRosterMemberAddressabilityRunning
  ObserveRuntimeRetired
  ObserveRuntimeRetiredStopped
  ObserveRuntimeDestroyed
  ObserveRuntimeDestroyedStopped
  ObserveMemberRetirementArchivedLive
  ObserveMemberRetirementArchivedRetired
  ObserveMemberRetirementArchivedLiveStopped
  ObserveMemberRetirementArchivedRetiredStopped
  ObserveMemberRetirementArchivedAlreadyCleared
  ObserveMemberRetirementArchivedStaleRuntimeAlreadyCleared
  ObserveMemberRetirementArchivedAlreadyClearedStopped
  ObserveMemberRetirementArchivedStaleRuntimeAlreadyClearedStopped
  ObserveMemberRetirementArchivedLivePreservingTopology
  ObserveMemberRetirementArchivedRetiredPreservingTopology
  BeginPlacedCompletionLifecycleQuiesceFreshStop
  StopRunning
  ResumeStopped
  AuditStartRun
  FinishRunRunning
)

# Every RetireAtScope verdict arm, the completion and destruction arms that
# reach the not-running verdict, the runtime destroy observation of a
# completed mob, and the destroy admission with a runtime destroy
# observation after it; each must fire in the scope run.
scope_covered_actions=(
  BeginPlacedCompletionLifecycleQuiesceFresh
  MarkCompleted
  ObserveRuntimeDestroyedCompleted
  AdmitDestroyCleanup
  ObserveRuntimeDestroyed
  DestroyMob
  RetireAtScopeCurrentRunning
  RetireAtScopeCurrentStopped
  RetireAtScopeNoCurrentIncarnationRunning
  RetireAtScopeNoCurrentIncarnationStopped
  RetireAtScopeStaleFenceTokenRunning
  RetireAtScopeStaleFenceTokenStopped
  RetireAtScopeStaleSessionBindingRunning
  RetireAtScopeStaleSessionBindingStopped
  RetireAtScopeMobNotRunningCompleted
  RetireAtScopeMobNotRunningDestroyed
)

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/member-lifecycle-audit.XXXXXX")"
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
# where <run> is lifecycle (AuditSpec and its action properties) or scope
# (AuditScopeSpec, its two invariants and the two destroy action properties).
write_cfg() {
  local run="$1" cfg="$2" invariants="$3" properties="$4"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "  AgentIdentityValues = {}" '  AgentIdentityValues = {"agentidentity_1"}' "${cfg}"
  replace_exact_line "  AgentRuntimeIdValues = {}" '  AgentRuntimeIdValues = {"agentruntimeid_1", "agentruntimeid_2"}' "${cfg}"
  replace_exact_line "  FenceTokenValues = {1}" "  FenceTokenValues = {1, 2}" "${cfg}"
  replace_exact_line "  GenerationValues = {1}" "  GenerationValues = {0, 1}" "${cfg}"
  replace_exact_line "  MobIdValues = {}" '  MobIdValues = {"mobid_1"}' "${cfg}"
  replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1", "sessionid_2"}' "${cfg}"
  replace_exact_line "  StringValues = {}" '  StringValues = {"profile_1", "token_1", "token_2"}' "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditStepIndex = ${step_index}
  AuditEpochIndex = ${epoch_index}" "${cfg}"
  case "${run}" in
    lifecycle)
      replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
      replace_exact_line "INVARIANTS" "PROPERTIES
  AuditObservationMonotonic
  AuditMintClearsExecutionHealth
  AuditMintClearsFailureRecords
  AuditRuntimeDestroyedRemovesOnlyItsRuntime
  AuditDestroyedOnlyThroughAnAdmittedDestroy${properties}
INVARIANTS
  AuditPlacementAbsent
  AuditRetiringRuntimeMapped${invariants}" "${cfg}"
      ;;
    scope)
      replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditScopeSpec" "${cfg}"
      replace_exact_line "INVARIANTS" "PROPERTIES
  AuditRuntimeDestroyedRemovesOnlyItsRuntime
  AuditDestroyedOnlyThroughAnAdmittedDestroy
INVARIANTS
  AuditRetireAtScopeCoherent
  AuditRetireAtScopeExactlyOne${invariants}" "${cfg}"
      if [[ -n "${properties}" ]]; then
        echo "error: the scope run takes no action properties" >&2
        exit 1
      fi
      ;;
    *)
      echo "error: unknown audit run '${run}'" >&2
      exit 1
      ;;
  esac
  replace_exact_line "CONSTRAINTS" "VIEW AuditView" "${cfg}"
  replace_exact_line "  CiStateConstraint" "" "${cfg}"
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
  # Witness runs end in a violation by design: -noGenerateSpecTE keeps TLC
  # from writing trace-exploration files into the spec directory.
  tlc_run_capped member_lifecycle_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" member_lifecycle_audit.tla || status=$?
  return "${status}"
}

# usage: check_space <run> <floor> <states of its first green run> <covered action...>
# runs <run> over its whole space with coverage and checks the outcome, the
# distinct-state floor and the covered actions.
check_space() {
  local run="$1" floor="$2" whole_space="$3"
  shift 3
  local cfg="${work_dir}/${run}.cfg"
  write_cfg "${run}" "${cfg}" "" ""
  local status=0
  run_tlc "${run}" "${cfg}" -coverage 1 ${tlc_args[@]+"${tlc_args[@]}"} || status=$?
  local log="${work_dir}/${run}.log"
  cat "${log}"
  if [[ "${status}" != "0" ]] \
    || ! grep -q "Model checking completed. No error has been found." "${log}"; then
    echo "error: member lifecycle audit ${run} run failed (tlc exit ${status})" >&2
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
  # `<Action line L, col C to line L, col C of module model>: D:T` (of module
  # member_lifecycle_audit for an audit wrapper), with D the new distinct
  # states it found and T the successor states it generated; an action that
  # fires only into known states has D = 0, so T counts.
  local coverage="${work_dir}/${run}-coverage.txt"
  awk '/^The coverage statistics at / { block = ""; next } { block = block $0 "\n" } END { printf "%s", block }' \
    "${log}" > "${coverage}"
  local action generated
  for action in "$@"; do
    generated="$(awk -v head="<${action} line " '
      index($0, head) == 1 && $0 ~ / of module (model|member_lifecycle_audit)>: [0-9]+:[0-9]+$/ {
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
check_space lifecycle "${min_distinct_states}" "${lifecycle_states}" "${covered_actions[@]}"
check_space scope "${min_scope_distinct_states}" "${scope_states}" "${scope_covered_actions[@]}"

# Each witness is <run>:<kind>:<name>: a state goal's negation joins the
# invariants, a step goal's negation ([][~step]_vars) joins the properties.
for witness in "lifecycle:Invariant:Bound" "lifecycle:Action property:ObservationAdvances" \
  "lifecycle:Invariant:Retiring" "lifecycle:Invariant:Archived" \
  "lifecycle:Action property:Successor" "lifecycle:Action property:StaleIncarnation" \
  "scope:Invariant:RetiringScopeCurrent" \
  "scope:Invariant:CurrentRuntimeGone"; do
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
  # usage: mutate <run> <name> <expected refusal regex>; reads a python edit
  # on stdin that rewrites model.tla in place and must find each anchor it
  # names.
  mutate() {
    local run="$1" name="$2" refusal="$3"
    shift 3
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}"/*.tla "${dir}/"
    MODEL="${dir}/model.tla" python3 -
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

  mutate lifecycle observation_unguarded "Error: Action property AuditObservationMonotonic is violated" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^ObserveMemberProgressChangedOpenRunning\(.*?\n\n", text, re.S | re.M)
assert block, "ObserveMemberProgressChangedOpenRunning not found"
body = block.group(0)
lines = body.split("\n")
guard = [i for i, line in enumerate(lines)
         if line.startswith("    /\\ OptionLe(") and "member_last_observed_at_ms" in line]
assert len(guard) == 1, "expected one observation_monotonic guard in ObserveMemberProgressChangedOpenRunning"
del lines[guard[0]]
text = text[: block.start()] + "\n".join(lines) + text[block.end():]
open(path, "w").write(text)
PY

  mutate lifecycle plain_option_ordering "The first argument of <= should be an integer" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^ObserveMemberProgressChangedOpenRunning\(.*?\n\n", text, re.S | re.M)
assert block, "ObserveMemberProgressChangedOpenRunning not found"
body = block.group(0)
match = re.search(r"^    /\\ OptionLe\((.*), Some\(observed_at_ms\)\)$", body, re.M)
assert match, "expected the observation_monotonic guard rendered through OptionLe"
plain = "    /\\ (" + match.group(1) + " <= Some(observed_at_ms))"
body = body[: match.start()] + plain + body[match.end():]
text = text[: block.start()] + body + text[block.end():]
open(path, "w").write(text)
PY

  mutate lifecycle peer_only_replay_unguarded "Error: Invariant pending_session_ingress_detach_has_session_correlation is violated" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^RecoverRosterMemberRetirementStartedPeerOnly\(.*?\n\n", text, re.S | re.M)
assert block, "RecoverRosterMemberRetirementStartedPeerOnly not found"
lines = block.group(0).split("\n")
guard = [i for i, line in enumerate(lines)
         if line == "    /\\ ((agent_runtime_id \\in DOMAIN runtime_retire_pending_sessions) = FALSE)"]
assert len(guard) == 1, "expected one retirement_session_absent guard in RecoverRosterMemberRetirementStartedPeerOnly"
del lines[guard[0]]
text = text[: block.start()] + "\n".join(lines) + text[block.end():]
open(path, "w").write(text)
PY

  mutate lifecycle stale_incarnation_old_guard "Error: Invariant (pending_respawn_topology_has_no_current_runtime|local_session_bound_member_is_live_or_wakeable) is violated" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^RecoverRosterMemberRetiredStaleIncarnation\(.*?\n\n", text, re.S | re.M)
assert block, "RecoverRosterMemberRetiredStaleIncarnation not found"
lines = block.group(0).split("\n")
unmapped = [i for i, line in enumerate(lines)
            if line == "    /\\ ((agent_identity \\in DOMAIN identity_to_runtime) = FALSE)"]
assert len(unmapped) == 1, "expected one identity_unmapped guard in RecoverRosterMemberRetiredStaleIncarnation"
advanced = [i for i, line in enumerate(lines)
            if line.startswith("    /\\ (") and line.endswith(" # Some(generation))")]
assert len(advanced) == 1, "expected one generation_advanced guard in RecoverRosterMemberRetiredStaleIncarnation"
# The guard before the partition: the recorded generation differs, or the
# identity maps to another runtime.
generation_differs = lines[advanced[0]][len("    /\\ "):]
old_guard = [
    "    /\\ \\/ " + generation_differs,
    "       \\/ /\\ (agent_identity \\in DOMAIN identity_to_runtime)",
    "          /\\ identity_to_runtime[agent_identity] # agent_runtime_id",
]
first = min(unmapped[0], advanced[0])
for index in sorted([unmapped[0], advanced[0]], reverse=True):
    del lines[index]
lines[first:first] = old_guard
text = text[: block.start()] + "\n".join(lines) + text[block.end():]
open(path, "w").write(text)
PY

  mutate scope retire_at_scope_unguarded "Error: Invariant AuditRetireAtScopeCoherent is violated" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^RetireAtScopeCurrentRunning\(.*?\n\n", text, re.S | re.M)
assert block, "RetireAtScopeCurrentRunning not found"
lines = block.group(0).split("\n")
guards = [i for i, line in enumerate(lines)
          if line.startswith("    /\\ ")
          and ("= Some(agent_runtime_id))" in line or "= Some(fence_token))" in line)]
assert len(guards) == 2, "expected the runtime and fence guards in RetireAtScopeCurrentRunning"
for index in reversed(guards):
    del lines[index]
text = text[: block.start()] + "\n".join(lines) + text[block.end():]
open(path, "w").write(text)
PY

  mutate scope verdict_overlap "Error: Invariant AuditRetireAtScopeExactlyOne is violated" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^RetireAtScopeStaleFenceTokenRunning\(.*?\n\n", text, re.S | re.M)
assert block, "RetireAtScopeStaleFenceTokenRunning not found"
lines = block.group(0).split("\n")
mapped = [i for i, line in enumerate(lines)
          if line == "    /\\ (agent_identity \\in DOMAIN identity_to_runtime)"]
assert len(mapped) == 1, "expected one mapped guard in RetireAtScopeStaleFenceTokenRunning"
del lines[mapped[0]]
text = text[: block.start()] + "\n".join(lines) + text[block.end():]
open(path, "w").write(text)
PY

  mutate lifecycle mint_keeps_execution_health "Error: Action property AuditMintClearsExecutionHealth is violated" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^RecoverRosterMemberRunning\(.*?\n\n", text, re.S | re.M)
assert block, "RecoverRosterMemberRunning not found"
body = block.group(0)
for field in ("member_run_open", "member_in_flight_work", "member_progress_tokens",
              "member_last_observed_at_ms", "member_last_progress_at_ms",
              "member_last_progress_event", "member_health_class"):
    clear = "    /\\ " + field + "' = MapRemove(" + field + ", agent_identity)"
    assert body.count(clear) == 1, "expected one clear of " + field + " in RecoverRosterMemberRunning"
    body = body.replace(clear, "    /\\ " + field + "' = " + field)
text = text[: block.start()] + body + text[block.end():]
open(path, "w").write(text)
PY

  # A runtime destroy observation that ends the mob, and one that leaves its
  # runtime live and fenced.
  mutate lifecycle destroyed_runtime_ends_the_mob "Error: Action property AuditRuntimeDestroyedRemovesOnlyItsRuntime is violated" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^ObserveRuntimeDestroyed\(.*?\n\n", text, re.S | re.M)
assert block, "ObserveRuntimeDestroyed not found"
body = block.group(0)
old = "    /\\ phase' = \"Running\""
assert body.count(old) == 1, "expected one phase update in ObserveRuntimeDestroyed"
body = body.replace(old, "    /\\ phase' = \"Destroyed\"")
text = text[: block.start()] + body + text[block.end():]
open(path, "w").write(text)
PY

  mutate lifecycle destroyed_runtime_left_live "Error: Action property AuditRuntimeDestroyedRemovesOnlyItsRuntime is violated" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^ObserveRuntimeDestroyed\(.*?\n\n", text, re.S | re.M)
assert block, "ObserveRuntimeDestroyed not found"
body = block.group(0)
for field, removal in (
    ("live_runtime_ids", "(live_runtime_ids \\ {agent_runtime_id})"),
    ("runtime_fence_tokens", "MapRemove(runtime_fence_tokens, agent_runtime_id)"),
):
    old = "    /\\ " + field + "' = " + removal
    assert body.count(old) == 1, f"expected one {field} removal in ObserveRuntimeDestroyed"
    body = body.replace(old, "    /\\ " + field + "' = " + field)
text = text[: block.start()] + body + text[block.end():]
open(path, "w").write(text)
PY

  mutate lifecycle destroyed_runtime_resets_the_runs "Error: Action property AuditRuntimeDestroyedRemovesOnlyItsRuntime is violated" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^ObserveRuntimeDestroyed\(.*?\n\n", text, re.S | re.M)
assert block, "ObserveRuntimeDestroyed not found"
body = block.group(0)
frames = re.findall(r"^    /\\ (UnchangedFrame_[0-9a-f]+)$", body, re.M)
assert len(frames) == 1, "expected one UNCHANGED frame in ObserveRuntimeDestroyed"
frame = frames[0]
definition = re.findall(r"^" + frame + r" == UNCHANGED << (.*) >>$", text, re.M)
assert len(definition) == 1, "expected one definition of " + frame
names = [name.strip() for name in definition[0].split(",")]
assert "active_run_count" in names, "the arm no longer leaves active_run_count unchanged"
kept = ", ".join(name for name in names if name != "active_run_count")
body = body.replace(
    "    /\\ " + frame,
    "    /\\ active_run_count' = 0\n    /\\ UNCHANGED << " + kept + " >>",
)
text = text[: block.start()] + body + text[block.end():]
open(path, "w").write(text)
PY
fi
echo "member lifecycle audit passed over the whole reachable space of its domains"
