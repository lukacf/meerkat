#!/usr/bin/env bash
# TLC audit of MobMachine identity entries.
#
# usage: identity_entry_audit.sh [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's model
# values (three identities, two runtime ids, fence tokens 1..2, generations
# 0..1, two sessions, the audit's strings) and its VIEW, which leaves out the
# two counters no guard, other variable's update or invariant of the
# generated model reads (model_step_count and topology_epoch). The runner
# first checks that the model still reads neither counter anywhere else, and
# fails otherwise. It checks one run over its whole space with TLC coverage
# (see identity_entry_audit.tla): a replayed member and a member seated by
# the spawn ladder with or without entry custody, the replay of entry rows,
# every arm of the legacy adoption, the admission probe, the agent lane's
# member admission, the worker reconcile and ensure, the build seed's
# composition authority, the seal, Stop and Resume.
# The run checks every generated invariant (the entry invariants among them:
# every entry is seated and local, no worker reconcile spawns or retires an
# entry, entry custody belongs to an open ladder), that exactly one arm of the
# admission probe, of the agent lane's admission and of the legacy adoption
# answers every input (AuditProbeExactlyOne, AuditAgentLaneExactlyOne,
# AuditAdoptionExactlyOne), that an entry's name probed without custody is
# refused as an entry's (AuditProbeRefusesEntryName), that an identity
# becomes an entry only by a custody commit, an observed adoption or the
# replay of its row (AuditEntryOnlyWithCause), and that an adoption without
# the legacy observation records nothing (AuditAdoptionNeedsObservation).
# The run must finish without an error, reach at least its floor of distinct
# states and fire every action of its covered list (TLC coverage). It then
# reruns the space once per reachability witness, with that witness's
# negation as an extra invariant (a state goal) or action property (a step
# goal), and requires TLC to report exactly that violation, so the properties
# are not vacuous. With --mutants it then seeds four defects into a copy of
# the generated model and requires TLC to refuse each with the invariant or
# property it breaks: a reconcile that retires an entry, an admission probe
# that answers an entry's name without custody as a plain duplicate, an
# adoption without the observation, and the replay of an entry row on an
# unseated identity. An anchor this script expects but cannot find fails the
# run. Each TLC run is held to the canonical lane's per-run cap
# (tlc_run_cap.sh) and fails closed as TLC INCOMPLETE.
set -euo pipefail

run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
# The run's distinct-state floor is 90% of the whole space its first green
# run found, so a guard that silently disables arms fails the lane.
entry_states=8736

# Every arm the run must fire. TLC names an arm after the generated
# operator, since every audit wrapper here is a plain application.
covered_actions=(
  RecoverRosterMemberRunning
  RecoverMemberSessionBindingFreshRunning
  RecoverRosterMemberRetiredRunning
  RecoverRosterMemberRetiredCurrentAlreadyAbsent
  AuthorizeSpawnProfileRunning
  BeginSpawnExecFresh
  CommitSpawnMembershipFresh
  AbortSpawnExecActiveRunning
  RecoverIdentityEntrySeatedRunning
  RecoverIdentityEntrySeatedStopped
  AdoptLegacyIdentityEntryAdopted
  AdoptLegacyIdentityEntryNameHeldByWorker
  AdoptLegacyIdentityEntryNotAdoptable
  AdoptLegacyIdentityEntryStaleObservation
  AdoptLegacyIdentityEntryNotSeated
  AdoptLegacyIdentityEntryAlreadyAnEntry
  AdoptLegacyIdentityEntryNonAuthoritative
  AdoptLegacyIdentityEntryMobNotRunningStopped
  ProbeMemberAdmissionDuplicateRunning
  ProbeMemberAdmissionAdmittedRunning
  ProbeMemberAdmissionIdentityMemberRunning
  ProbeMemberAdmissionDuplicateStopped
  ProbeMemberAdmissionAdmittedStopped
  ProbeMemberAdmissionIdentityMemberStopped
  ResolveAgentLaneMemberAdmissionAdmittedRunning
  ResolveAgentLaneMemberAdmissionIdentityMemberRunning
  ResolveAgentLaneMemberAdmissionAdmittedStopped
  ResolveAgentLaneMemberAdmissionIdentityMemberStopped
  ReconcileRunning
  ReconcileStopped
  EnsureMemberRunningExisting
  EnsureMemberRunningMissing
  SeedMemberResidencyRunning
  SealBootMaterializationPlan
  BeginPlacedCompletionLifecycleQuiesceFreshStop
  StopRunning
  ResumeStopped
)

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/identity-entry-audit.XXXXXX")"
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

# usage: write_cfg <file> <extra invariant lines> <extra property lines>
write_cfg() {
  local cfg="$1" invariants="$2" properties="$3"
  local section_end
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "  AgentIdentityValues = {}" '  AgentIdentityValues = {"agentidentity_1", "agentidentity_2", "agentidentity_3"}' "${cfg}"
  replace_exact_line "  AgentRuntimeIdValues = {}" '  AgentRuntimeIdValues = {"agentruntimeid_1", "agentruntimeid_2"}' "${cfg}"
  replace_exact_line "  FenceTokenValues = {1}" "  FenceTokenValues = {1, 2}" "${cfg}"
  replace_exact_line "  GenerationValues = {1}" "  GenerationValues = {0, 1}" "${cfg}"
  replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1", "sessionid_2"}' "${cfg}"
  replace_exact_line "  StringValues = {}" '  StringValues = {"profile_1", "model_1", "digest_1"}' "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditStepIndex = ${step_index}
  AuditEpochIndex = ${epoch_index}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "INVARIANTS" "PROPERTIES
  AuditEntryOnlyWithCause
  AuditAdoptionNeedsObservation${properties}
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
  replace_exact_line "CONSTRAINTS" "  AuditProbeExactlyOne
  AuditProbeRefusesEntryName
  AuditAgentLaneExactlyOne
  AuditAdoptionExactlyOne${invariants}
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
  tlc_run_capped identity_entry_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" identity_entry_audit.tla || status=$?
  return "${status}"
}

tlc_args=("$@")

# The whole space, with coverage: the outcome, the distinct-state floor (90%
# of the first green run) and the covered actions.
safety_cfg="${work_dir}/entry.cfg"
write_cfg "${safety_cfg}" "" ""
safety_status=0
run_tlc entry "${safety_cfg}" -coverage 1 ${tlc_args[@]+"${tlc_args[@]}"} || safety_status=$?
safety_log="${work_dir}/entry.log"
cat "${safety_log}"
if [[ "${safety_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${safety_log}"; then
  echo "error: identity entry audit failed (tlc exit ${safety_status})" >&2
  exit 1
fi
distinct="$(sed -nE 's/^[0-9]+ states generated, ([0-9]+) distinct states found, 0 states left on queue\.$/\1/p' \
  "${safety_log}" | tail -1)"
if [[ -z "${distinct}" ]]; then
  echo "error: the identity entry run reported no final distinct-state count" >&2
  exit 1
fi
floor=$(( entry_states * 9 / 10 ))
if (( distinct < floor )); then
  echo "error: identity entry run: ${distinct} distinct states, below the floor ${floor} (90% of ${entry_states})" >&2
  exit 1
fi
echo "identity entry distinct states: ${distinct} (floor ${floor})"

# TLC prints a coverage block every minute and one at the end; read the
# last. An action's line is
# `<Action line L, col C to line L, col C of module model>: D:T`, with D the
# new distinct states it found and T the successor states it generated; an
# action that fires only into known states has D = 0, so T counts.
coverage="${work_dir}/entry-coverage.txt"
awk '/^The coverage statistics at / { block = ""; next } { block = block $0 "\n" } END { printf "%s", block }' \
  "${safety_log}" > "${coverage}"
for action in "${covered_actions[@]}"; do
  generated="$(awk -v head="<${action} line " '
    index($0, head) == 1 && $0 ~ / of module (model|identity_entry_audit)( \([0-9]+ [0-9]+ [0-9]+ [0-9]+\))?>: [0-9]+:[0-9]+$/ {
      split($NF, counts, ":"); total += counts[2]; found = 1
    }
    END { if (found) print total }' "${coverage}")"
  if [[ -z "${generated}" || "${generated}" == "0" ]]; then
    echo "error: identity entry run: ${action} never fired (coverage '${generated:-absent}')" >&2
    exit 1
  fi
  echo "covered ${action}: ${generated} successor states"
done

# Each witness is <kind>:<name>: a state goal's negation joins the
# invariants, a step goal's negation ([][~step]_vars) joins the properties.
for witness in \
  "Action property:CustodyCommit" "Action property:CustodyAborted" \
  "Action property:Replayed" "Action property:ReplayedStopped" \
  "Action property:Adopted" "Action property:ReconcileSparesEntry" \
  "Action property:ReconcileSpawnsWorkerOnly" "Action property:Unseated" \
  "Invariant:ProbeRefused" "Invariant:LaneRefusedStopped" "Invariant:TwoEntries"; do
  kind="${witness%%:*}"
  name="${witness#*:}"
  cfg="${work_dir}/witness-${name}.cfg"
  if [[ "${kind}" == "Invariant" ]]; then
    write_cfg "${cfg}" "
  NotAuditWitness${name}" ""
  else
    write_cfg "${cfg}" "" "
  NotAuditWitness${name}"
  fi
  status=0
  run_tlc "witness-${name}" "${cfg}" ${tlc_args[@]+"${tlc_args[@]}"} || status=$?
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
  # usage: mutate <name> <expected violation regex>; reads a python edit on
  # stdin that rewrites model.tla in place and must find each anchor it names.
  mutate() {
    local name="$1" violation="$2"
    shift 2
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}"/*.tla "${dir}/"
    MODEL="${dir}/model.tla" python3 -
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${cfg}" "" ""
    local status=0
    (cd "${dir}" && run_tlc "mutant-${name}" "${cfg}" ${tlc_args[@]+"${tlc_args[@]}"}) || status=$?
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -qE "Error: (${violation}) is violated" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by ${violation} (tlc exit ${status})" >&2
      exit 1
    fi
    echo "mutant ${name} refused by $(grep -oE "Error: (${violation}) is violated" "${log}" | head -1)"
  }

  # A reconcile that retires an entry: the retire helper ranges over every
  # bound identity again.
  mutate reconcile_retires_entry "Invariant worker_reconcile_spares_entries" <<'PY'
import os
path = os.environ["MODEL"]
text = open(path).read()
old = " /\\ member \\notin DOMAIN arg_identity_entry_origins }"
start = text.index("mob_machine_members_to_retire(")
end = text.index("\n\n", start) if "\n\n" in text[start:] else len(text)
head = text[start:end]
assert head.count(old) == 1, "expected the entry filter once in mob_machine_members_to_retire"
text = text[:start] + head.replace(old, " }") + text[end:]
open(path, "w").write(text)
PY

  # The name reservation: the admission probe answers an entry's name
  # without custody as a plain duplicate, as before identity entries. (The
  # spawn ladder cannot open on a seated entry's name: its
  # current_binding_absent guard already refuses the bound identity.)
  mutate probe_answers_entry_as_duplicate "Invariant AuditProbeExactlyOne" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^ProbeMemberAdmissionDuplicateRunning\(.*?\n\n", text, re.S | re.M)
assert block, "ProbeMemberAdmissionDuplicateRunning not found"
lines = block.group(0).split("\n")
guard = [i for i, line in enumerate(lines)
         if "identity_entry_origins" in line and "entry_custody" in line]
assert len(guard) == 1, "expected one entry-custody guard in ProbeMemberAdmissionDuplicateRunning"
del lines[guard[0]]
text = text[: block.start()] + "\n".join(lines) + text[block.end():]
open(path, "w").write(text)
PY

  # An adoption without the shell's legacy observation.
  mutate adoption_without_observation \
    "Invariant AuditAdoptionExactlyOne|Action property AuditAdoptionNeedsObservation|Action property AuditEntryOnlyWithCause" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^AdoptLegacyIdentityEntryAdopted\(.*?\n\n", text, re.S | re.M)
assert block, "AdoptLegacyIdentityEntryAdopted not found"
lines = block.group(0).split("\n")
guard = [i for i, line in enumerate(lines) if "(legacy_roster_member = TRUE)" in line]
assert len(guard) == 1, "expected one observation guard in AdoptLegacyIdentityEntryAdopted"
del lines[guard[0]]
text = text[: block.start()] + "\n".join(lines) + text[block.end():]
open(path, "w").write(text)
PY

  # The replay of an entry row on an identity that is not seated.
  mutate replay_on_unseated_identity "Invariant entry_is_seated_or_has_an_unseated_cause" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^RecoverIdentityEntrySeatedRunning\(.*?\n\n", text, re.S | re.M)
assert block, "RecoverIdentityEntrySeatedRunning not found"
lines = block.group(0).split("\n")
guard = [i for i, line in enumerate(lines)
         if "identity_to_runtime" in line and "= Some(agent_runtime_id)" in line]
assert len(guard) == 1, "expected one seated-incarnation guard in RecoverIdentityEntrySeatedRunning"
del lines[guard[0]]
text = text[: block.start()] + "\n".join(lines) + text[block.end():]
open(path, "w").write(text)
PY
fi
echo "identity entry audit passed"
