#!/usr/bin/env bash
# TLC audit of MobMachine created identity entries.
#
# usage: created_identity_audit.sh [--mutants] [--part recovery|<run>]
#                                  [extra tlc args...]
#
# Derives each run's TLC config from the generated ci.cfg next to this script
# (its SPECIFICATION, its generated invariants and its domains), as
# identity_entry_audit.sh does for the declared apply, with the VIEW of
# member_residency_audit that leaves out the two counters no guard reads
# (model_step_count and topology_epoch); the runner first checks that the
# model reads neither anywhere else. It checks four runs of
# created_identity_audit.tla, each over its whole space with TLC coverage:
# its recovery part (--part recovery; every part by default; --part <run>
# checks one run):
#
# - scan: member 1 recovered and bound, its Declared entry row replayed or
#   not, member 2 with no member, and the boot scan of a row of either with
#   every presence and revival record, in a running and a stopped mob, until
#   the boot plan's seal. Exactly one arm answers every row while the replay
#   runs and none after the seal (AuditRecoverExactlyOne).
# - reseat: the scan of rows of two identities without a member (without a
#   revival record, but member 2's Present row), a ladder without custody
#   for member 1 and one with each pair of custodies for member 2, seated on
#   member 2's revival record or a fresh session, their aborts, the worker
#   reconcile of both members or of none and the forced destroy: no ladder
#   opens for a reserved name (AuditNoLadderForReservedName), a ladder's
#   custody matches its name's origin (AuditLadderCustodyMatchesOrigin), a
#   seat under created custody binds the revival record
#   (AuditCreatedSeatTakesRecord), and exactly one arm answers every row
#   and every admission probe, ladders and reseated members included.
# - names: the scan of both identities' rows (without a revival record),
#   then the declarations, the seal over every desired set, the legacy
#   adoption, the agent lane and the admission probe, under either
#   composition authority, in a running and a stopped mob: exactly one arm
#   answers each, and the seal never touches a Created entry or an
#   unreadable name's (AuditSealNeverTouchesCreated).
# - placed: member 2 seated on a bound member host by the placed spawn
#   ladder and its activation, then its row scanned: a Present row of a
#   placed member reserves the name.
#
# Every run checks every generated invariant (the created entries' among
# them: unseated_created_entry_is_preparing_or_owed,
# tombstone_is_never_an_entry, entry_custodies_are_disjoint,
# worker_reconcile_spares_entries and destroyed_mob_holds_no_member_lifecycle).
#
# Each run must finish without an error, reach at least its floor of distinct
# states and fire every action of its covered list (TLC coverage): an arm
# that answers exactly the states its guard names (a refusal of a reserved
# name, say) has the coverage check as its witness. The runner also requires
# every witness reached (a step goal's negation violated), and --mutants
# seeds one defect per created entries' invariant and property and per new
# guard a reachable state needs, each of which TLC must refuse with the
# named violation. An anchor this script expects but cannot find fails the
# run. Each TLC run is held to the canonical lane's per-run cap
# (tlc_run_cap.sh) and fails closed as TLC INCOMPLETE.
set -euo pipefail

run_mutants=false
run_part=all
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mutants)
      run_mutants=true
      shift
      ;;
    --part)
      run_part="${2:-}"
      shift 2 || { echo "error: --part needs a part or a run" >&2; exit 2; }
      ;;
    *) break ;;
  esac
done
# usage: part_of <run>: the part that checks <run>.
part_of() {
  case "$1" in
    scan|reseat|names|placed) echo recovery ;;
    *) return 1 ;;
  esac
}
case "${run_part}" in
  all|recovery) ;;
  *)
    if ! part_of "${run_part}" > /dev/null; then
      echo "error: unknown part '${run_part}' (recovery or a run)" >&2
      exit 2
    fi
    ;;
esac
# usage: in_part <run>: whether the selected part checks <run>.
in_part() {
  [[ "${run_part}" == "all" || "${run_part}" == "$1" || "${run_part}" == "$(part_of "$1")" ]]
}
# Each run's distinct-state floor is 90% of the whole space its first green
# run found, so a guard that silently disables arms fails the lane.
scan_states=1512
reseat_states=3245
names_states=1560
placed_states=150

# Every arm each run must fire. TLC names an arm after the generated
# operator, since every audit wrapper here is a plain application.
scan_covered_actions=(
  RecoverRosterMemberRunning
  RecoverMemberSessionBindingFreshRunning
  RecoverIdentityEntrySeatedRunning
  RecoverCreatedIdentityEntrySeatedRunning
  RecoverCreatedIdentityEntrySeatedStopped
  RecoverCreatedIdentityEntryRetiredRunning
  RecoverCreatedIdentityEntryRetiredStopped
  RecoverCreatedIdentityEntryPendingRunning
  RecoverCreatedIdentityEntryPendingStopped
  RecoverCreatedIdentityEntryTombstoneRunning
  RecoverCreatedIdentityEntryTombstoneStopped
  RecoverCreatedIdentityEntryUnreadableRunning
  RecoverCreatedIdentityEntryUnreadableStopped
  BeginPlacedCompletionLifecycleQuiesceFreshStop
  StopRunning
  ResumeStopped
  SealBootMaterializationPlan
)

reseat_covered_actions=(
  RecoverCreatedIdentityEntrySeatedRunning
  RecoverCreatedIdentityEntryRetiredRunning
  RecoverCreatedIdentityEntryPendingRunning
  RecoverCreatedIdentityEntryTombstoneRunning
  RecoverCreatedIdentityEntryUnreadableRunning
  SeedMemberResidencyRunning
  AuthorizeSpawnProfileRunning
  BeginSpawnExecFresh
  AbortSpawnExecActiveRunning
  CommitDormantSeatRunning
  ReconcileRunning
  BeginPlacedCompletionLifecycleQuiesceFresh
  DestroyFromAny
)

names_covered_actions=(
  RecoverRosterMemberRunning
  RecoverMemberSessionBindingFreshRunning
  RecoverIdentityEntrySeatedRunning
  RecoverCreatedIdentityEntrySeatedRunning
  RecoverCreatedIdentityEntrySeatedStopped
  RecoverCreatedIdentityEntryRetiredRunning
  RecoverCreatedIdentityEntryRetiredStopped
  RecoverCreatedIdentityEntryPendingRunning
  RecoverCreatedIdentityEntryPendingStopped
  RecoverCreatedIdentityEntryTombstoneRunning
  RecoverCreatedIdentityEntryTombstoneStopped
  RecoverCreatedIdentityEntryUnreadableRunning
  RecoverCreatedIdentityEntryUnreadableStopped
  SeedMemberResidencyRunning
  BeginPlacedCompletionLifecycleQuiesceFreshStop
  StopRunning
  ResumeStopped
  DeclareIdentityEntryKept
  DeclareIdentityEntrySeatOwed
  DeclareIdentityEntryRefusedNameHeldByWorker
  DeclareIdentityEntryRefusedNonAuthoritative
  DeclareIdentityEntryRefusedNameOwnedByCreated
  DeclareIdentityEntryRefusedNameTombstoned
  DeclareIdentityEntryRefusedNameUnreadable
  DeclareIdentityEntryMobNotRunningStopped
  SealDeclaredRosterSealed
  SealDeclaredRosterNonAuthoritative
  SealDeclaredRosterMobNotRunningStopped
  AdoptLegacyIdentityEntryAdopted
  AdoptLegacyIdentityEntryNameHeldByWorker
  AdoptLegacyIdentityEntryNotAdoptable
  AdoptLegacyIdentityEntryNotSeated
  AdoptLegacyIdentityEntryAlreadyAnEntry
  AdoptLegacyIdentityEntryOtherCustody
  AdoptLegacyIdentityEntryNonAuthoritative
  AdoptLegacyIdentityEntryMobNotRunningStopped
  ResolveAgentLaneMemberAdmissionAdmittedRunning
  ResolveAgentLaneMemberAdmissionAdmittedStopped
  ResolveAgentLaneMemberAdmissionIdentityMemberRunning
  ResolveAgentLaneMemberAdmissionIdentityMemberStopped
  ResolveAgentLaneMemberAdmissionUnreadableNameRunning
  ResolveAgentLaneMemberAdmissionUnreadableNameStopped
  ProbeMemberAdmissionDuplicateRunning
  ProbeMemberAdmissionDuplicateStopped
  ProbeMemberAdmissionAdmittedRunning
  ProbeMemberAdmissionAdmittedStopped
  ProbeMemberAdmissionIdentityMemberRunning
  ProbeMemberAdmissionIdentityMemberStopped
  ProbeMemberAdmissionIdentityMemberOfAnotherOriginRunning
  ProbeMemberAdmissionIdentityMemberOfAnotherOriginStopped
  ProbeMemberAdmissionIdentityNameReservedRunning
  ProbeMemberAdmissionIdentityNameReservedStopped
)

placed_covered_actions=(
  BindOwnerBridgeSessionRunning
  BeginHostBindFreshUnplaced
  CommitHostBind
  AuthorizeSpawnProfileRunning
  BeginSpawnExecRemote
  CommitSpawnMembershipRemote
  CommitSpawnActivationFinalRunning
  RecoverCreatedIdentityEntryRetiredRunning
  RecoverCreatedIdentityEntryPendingRunning
  RecoverCreatedIdentityEntryTombstoneRunning
  RecoverCreatedIdentityEntryUnreadableRunning
)

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/created-identity-audit.XXXXXX")"
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
#                  [without-totality]
write_cfg() {
  local run="$1" cfg="$2" invariants="$3" properties="$4" totality="${5:-}"
  local spec run_properties run_invariants section_end
  cp "${ci_cfg}" "${cfg}"
  case "${run}" in
    scan)
      spec="ScanSpec"
      run_properties=""
      run_invariants="
  AuditRecoverExactlyOne"
      ;;
    reseat)
      spec="ReseatSpec"
      run_properties="
  AuditNoLadderForReservedName
  AuditLadderCustodyMatchesOrigin
  AuditCreatedSeatTakesRecord"
      run_invariants="
  AuditReseatRecoverExactlyOne
  AuditReseatProbeExactlyOne"
      ;;
    names)
      spec="NamesSpec"
      run_properties="
  AuditSealNeverTouchesCreated"
      run_invariants="
  AuditNamesDeclareExactlyOne
  AuditSealExactlyOne
  AuditNamesAdoptionExactlyOne
  AuditNamesLaneExactlyOne
  AuditNamesProbeExactlyOne"
      ;;
    placed)
      spec="PlacedScanSpec"
      run_properties=""
      run_invariants="
  AuditRecoverExactlyOne"
      ;;
    *)
      echo "error: unknown audit run '${run}'" >&2
      exit 1
      ;;
  esac
  case "${totality}" in
    "") ;;
    without-totality) run_invariants="" ;;
    *)
      echo "error: unknown write_cfg mode '${totality}'" >&2
      exit 1
      ;;
  esac
  # The declared apply's runs' domains (identity_entry_audit.sh).
  replace_exact_line "  AgentIdentityValues = {}" '  AgentIdentityValues = {"agentidentity_1", "agentidentity_2"}' "${cfg}"
  replace_exact_line "  AgentRuntimeIdValues = {}" '  AgentRuntimeIdValues = {"agentruntimeid_1", "agentruntimeid_2", "agentruntimeid_3"}' "${cfg}"
  replace_exact_line "  FenceTokenValues = {1}" "  FenceTokenValues = {1, 2, 3}" "${cfg}"
  replace_exact_line "  GenerationValues = {1}" "  GenerationValues = {0, 1}" "${cfg}"
  replace_exact_line "  MobIdValues = {}" '  MobIdValues = {"mobid_1"}' "${cfg}"
  replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1", "sessionid_2", "sessionid_3"}' "${cfg}"
  replace_exact_line "  StringValues = {}" '  StringValues = {"profile_1", "profile_2", "model_1", "digest_1", "reason_1", "code_1", "objective_1"}' "${cfg}"
  replace_exact_line "  ResumeAttemptIdValues = {}" '  ResumeAttemptIdValues = {"resumeattemptid_1"}' "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditStepIndex = ${step_index}
  AuditEpochIndex = ${epoch_index}
  AuditMaxAttempts = 1" "${cfg}"
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

# usage: run_tlc <name> <cfg> [extra tlc args...]; checks the audit module in
# the current directory and leaves the log at ${work_dir}/<name>.log
run_tlc() {
  local name="$1" cfg="$2"
  shift 2
  local status=0
  # Witness and mutant runs end in a violation by design: -noGenerateSpecTE
  # keeps TLC from writing trace-exploration files into the spec directory.
  tlc_run_capped created_identity_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" created_identity_audit.tla || status=$?
  return "${status}"
}

tlc_args=("$@")

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
    echo "error: created identity audit ${run} run failed (tlc exit ${status})" >&2
    exit 1
  fi
  local distinct
  distinct="$(sed -nE 's/^[0-9]+ states generated, ([0-9]+) distinct states found, 0 states left on queue\.$/\1/p' \
    "${log}" | tail -1)"
  if [[ -z "${distinct}" ]]; then
    echo "error: the created identity audit ${run} run reported no final distinct-state count" >&2
    exit 1
  fi
  if (( distinct < floor )); then
    echo "error: created identity audit ${run} run: ${distinct} distinct states, below the floor ${floor} (90% of ${whole_space})" >&2
    exit 1
  fi
  echo "created identity audit ${run} run: ${distinct} distinct states (floor ${floor})"

  # TLC prints a coverage block every minute and one at the end; read the
  # last. An action's line is
  # `<Action line L, col C to line L, col C of module model>: D:T`, with D the
  # new distinct states it found and T the successor states it generated; an
  # action that fires only into known states has D = 0, so T counts.
  local coverage="${work_dir}/${run}-coverage.txt"
  awk '/^The coverage statistics at / { block = ""; next } { block = block $0 "\n" } END { printf "%s", block }' \
    "${log}" > "${coverage}"
  local action generated
  for action in "$@"; do
    generated="$(awk -v head="<${action} line " '
      index($0, head) == 1 && $0 ~ / of module (model|created_identity_audit|identity_entry_apply_audit|member_residency_audit)( \([0-9]+ [0-9]+ [0-9]+ [0-9]+\))?>: [0-9]+:[0-9]+$/ {
        split($NF, counts, ":"); total += counts[2]; found = 1
      }
      END { if (found) print total }' "${coverage}")"
    if [[ -z "${generated}" || "${generated}" == "0" ]]; then
      echo "error: created identity audit ${run} run: ${action} never fired (coverage '${generated:-absent}')" >&2
      exit 1
    fi
    echo "covered ${action}: ${generated} successor states"
  done
}

for run in scan reseat names placed; do
  if in_part "${run}"; then
    floor_var="${run}_states"
    arms_var="${run}_covered_actions[@]"
    check_space "${run}" "${!floor_var}" "${!arms_var}"
  fi
done

# Each witness is <run>:<kind>:<name>: a state goal's negation joins the
# invariants, a step goal's negation ([][~step]_vars) joins the properties.
# A witness run leaves out its run's ENABLED totality invariants: it explores
# a part of the run's space, whose whole-space run checks them in every
# state.
for witness in \
  "scan:Action property:ScanSeated" "scan:Action property:ScanRetired" \
  "scan:Action property:ScanPending" "scan:Action property:ScanTombstone" \
  "scan:Action property:ScanUnreadable" "scan:Action property:ScanContradictedDeclared" \
  "scan:Action property:ScanContradictedMember" "scan:Action property:ScanEntryTombstoned" \
  "reseat:Action property:ReseatOnRecord" "reseat:Action property:CreatedCustodyAborted" \
  "reseat:Action property:ScanContradictedLadder" "reseat:Action property:ReconcileSparesReserved" \
  "reseat:Action property:DestroyClearsReserved" \
  "names:Action property:SealKeepsCreated" "names:Action property:SealSkipsUnreadable" \
  "names:Action property:ProbeOtherOrigin" \
  "placed:Action property:ScanContradictedPlaced"; do
  run="${witness%%:*}"
  if ! in_part "${run}"; then
    continue
  fi
  rest="${witness#*:}"
  kind="${rest%%:*}"
  name="${rest#*:}"
  cfg="${work_dir}/witness-${name}.cfg"
  if [[ "${kind}" == "Invariant" ]]; then
    write_cfg "${run}" "${cfg}" "
  NotAuditWitness${name}" "" without-totality
  else
    write_cfg "${run}" "${cfg}" "" "
  NotAuditWitness${name}" without-totality
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
  # Edit helpers the mutants share: each mutant's python edit runs after
  # them, rewrites the copied model.tla in place and must find each anchor
  # it names.
  read -r -d '' mutant_prelude <<'PY' || true
import os, re
path = os.environ["MODEL"]

def _arm(text, arm):
    found = list(re.finditer(r"^" + re.escape(arm) + r"(\([^)]*\))? ==\n.*?\n\n", text, re.S | re.M))
    assert len(found) == 1, f"expected one definition of {arm}, found {len(found)}"
    return found[0]

def keep_var(arm, var):
    """The arm leaves `var` as it was instead of updating it."""
    text = open(path).read()
    found = _arm(text, arm)
    lines = found.group(0).split("\n")
    hits = [i for i, line in enumerate(lines) if line.startswith("    /\\ " + var + "' = ")]
    assert len(hits) == 1, f"expected one update of {var} in {arm}, found {len(hits)}"
    lines[hits[0]] = "    /\\ " + var + "' = " + var
    open(path, "w").write(text[:found.start()] + "\n".join(lines) + text[found.end():])

def replace_update(arm, var, rhs):
    """The arm updates `var`, which it updates already, to `rhs`."""
    text = open(path).read()
    found = _arm(text, arm)
    lines = found.group(0).split("\n")
    hits = [i for i, line in enumerate(lines) if line.startswith("    /\\ " + var + "' = ")]
    assert len(hits) == 1, f"expected one update of {var} in {arm}, found {len(hits)}"
    lines[hits[0]] = "    /\\ " + var + "' = " + rhs
    open(path, "w").write(text[:found.start()] + "\n".join(lines) + text[found.end():])

def drop_guard(arm, needle):
    """The arm loses the one guard line, before its first update, that
    contains `needle`."""
    text = open(path).read()
    found = _arm(text, arm)
    lines = found.group(0).split("\n")
    first_update = next(i for i, line in enumerate(lines) if "' = " in line)
    hits = [i for i, line in enumerate(lines[:first_update]) if line.startswith("    /\\ ") and needle in line]
    assert len(hits) == 1, f"expected one guard with {needle!r} in {arm}, found {len(hits)}"
    del lines[hits[0]]
    open(path, "w").write(text[:found.start()] + "\n".join(lines) + text[found.end():])

def replace_in_arm(arm, old, new, count=1):
    """The `count` occurrences of `old` in the arm's definition become
    `new`."""
    text = open(path).read()
    found = _arm(text, arm)
    body = found.group(0)
    assert body.count(old) == count, f"expected {count} of {old[:70]!r} in {arm}, found {body.count(old)}"
    open(path, "w").write(text[:found.start()] + body.replace(old, new) + text[found.end():])

def replace_text(old, new):
    """The one occurrence of `old` in the model becomes `new`."""
    text = open(path).read()
    assert text.count(old) == 1, f"expected one {old[:70]!r}, found {text.count(old)}"
    open(path, "w").write(text.replace(old, new))
PY

  # usage: mutate <run> <name> <expected violation regex>; reads a python
  # edit on stdin, run after the shared helpers, that rewrites model.tla in
  # place and must find each anchor it names. A mutant runs without its
  # run's ENABLED totality invariants unless it expects one of them (they
  # cost most of a deep run's time); a mutant that expects a generated
  # invariant or an action property then meets it even where the totality
  # would refuse an earlier state first (a second arm answering a row).
  mutate() {
    local run="$1" name="$2" violation="$3"
    if ! in_part "${run}"; then
      cat > /dev/null
      return 0
    fi
    local totality=""
    if [[ "${violation}" != *ExactlyOne* ]]; then
      totality=without-totality
    fi
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}"/*.tla "${dir}/"
    { printf '%s\n' "${mutant_prelude}"; cat; } | MODEL="${dir}/model.tla" python3 -
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${run}" "${cfg}" "" "" "${totality}"
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

  # The scan answers a row after the boot plan's seal.
  mutate scan scan_after_seal "Invariant AuditRecoverExactlyOne" <<'PY'
drop_guard("RecoverCreatedIdentityEntrySeatedRunning", "(boot_plan_sealed = FALSE)")
PY

  # A Present row whose identity has no member records the entry hosted on
  # its first use instead of owed its seat.
  mutate scan retired_records_hosted "Invariant unseated_created_entry_is_preparing_or_owed" <<'PY'
replace_update("RecoverCreatedIdentityEntryRetiredRunning", "identity_entry_unseated",
               'MapSet(identity_entry_unseated, agent_identity, "HostedOnFirstUse")')
PY

  # A removed row keeps the Created entry its earlier row recorded.
  mutate scan tombstone_keeps_entry "Invariant tombstone_is_never_an_entry" <<'PY'
keep_var("RecoverCreatedIdentityEntryTombstoneRunning", "identity_entry_origins")
keep_var("RecoverCreatedIdentityEntryTombstoneRunning", "identity_entry_unseated")
keep_var("RecoverCreatedIdentityEntryTombstoneRunning", "identity_entry_archived_sessions")
PY

  # A Present row of a seated member records it over its Declared entry
  # (its no_declared_entry guard dropped).
  mutate scan seated_over_declared "Invariant AuditRecoverExactlyOne" <<'PY'
drop_guard("RecoverCreatedIdentityEntrySeatedRunning", 'Some("Declared"))')
PY

  # A Present row of a seated member records it over an unreadable name
  # (its name_readable guard dropped).
  mutate scan seated_over_unreadable "Invariant AuditRecoverExactlyOne" <<'PY'
drop_guard("RecoverCreatedIdentityEntrySeatedRunning", "identity_entry_unreadable) = FALSE)")
PY

  # A row that says no member while the replay seated one is not reserved.
  mutate scan member_contradiction_ignored "Invariant AuditRecoverExactlyOne" <<'PY'
replace_in_arm("RecoverCreatedIdentityEntryUnreadableRunning",
               ' \\/ ((presence # "Present") /\\ ((agent_identity \\in DOMAIN identity_to_runtime) = TRUE))', "")
PY

  # A Present row of an identity without a member records an entry under a
  # removed entry's name (its name_not_tombstoned guard dropped).
  mutate scan retired_ignores_tombstone "Invariant tombstone_is_never_an_entry" <<'PY'
drop_guard("RecoverCreatedIdentityEntryRetiredRunning", "identity_entry_tombstones) = FALSE)")
PY

  # The journal replay recovers a member under a removed entry's name (the
  # roster replay's name_not_tombstoned guard dropped).
  mutate scan replay_ignores_tombstone "Invariant tombstone_is_never_an_entry" <<'PY'
drop_guard("RecoverRosterMemberRunning", "identity_entry_tombstones) = FALSE)")
PY

  # A spawn ladder opens for a reserved name (its name_not_reserved guard
  # dropped).
  mutate reseat ladder_takes_reserved_name "Action property AuditNoLadderForReservedName" <<'PY'
drop_guard("BeginSpawnExecFresh", "identity_entry_tombstones) = FALSE)")
PY

  # Created custody takes a Created entry not owed its seat.
  mutate reseat created_custody_any_created "Action property AuditLadderCustodyMatchesOrigin" <<'PY'
replace_in_arm("BeginSpawnExecFresh",
               ' /\\ ((IF (agent_identity \\in DOMAIN identity_entry_unseated) THEN Some((IF agent_identity \\in DOMAIN identity_entry_unseated THEN identity_entry_unseated[agent_identity] ELSE "None")) ELSE None) = Some("SeatOwed"))',
               "")
PY

  # Entry custody takes a Created entry's name.
  mutate reseat entry_custody_takes_created "Action property AuditLadderCustodyMatchesOrigin" <<'PY'
replace_in_arm("BeginSpawnExecFresh",
               '((entry_custody = TRUE) /\\ (created_custody = FALSE) /\\ ((IF (agent_identity \\in DOMAIN identity_entry_origins) THEN Some((IF agent_identity \\in DOMAIN identity_entry_origins THEN identity_entry_origins[agent_identity] ELSE "None")) ELSE None) = Some("Declared")))',
               "((entry_custody = TRUE) /\\ (created_custody = FALSE))")
PY

  # An abort keeps the created custody of the ladder it ends.
  mutate reseat abort_keeps_created_custody "Invariant entry_custodies_are_disjoint" <<'PY'
keep_var("AbortSpawnExecActiveRunning", "spawn_exec_created_custody")
PY

  # A removed row drops the Created entry a created ladder acts for (its
  # no_spawn_ladder guard dropped).
  mutate reseat tombstone_ignores_ladder "Invariant entry_custodies_are_disjoint" <<'PY'
drop_guard("RecoverCreatedIdentityEntryTombstoneRunning", "DOMAIN spawn_exec_phase) = FALSE)")
PY

  # A seat under created custody binds another session than the revival
  # record (its created_seat_takes_its_record guard dropped).
  mutate reseat created_seat_any_session "Action property AuditCreatedSeatTakesRecord" <<'PY'
drop_guard("CommitDormantSeatRunning", "spawn_exec_created_custody) = FALSE)")
PY

  # The worker reconcile spawns a tombstoned name.
  mutate reseat reconcile_spawns_tombstone "Invariant worker_reconcile_spares_entries" <<'PY'
replace_text(" /\\ member \\notin arg_identity_entry_tombstones", "")
PY

  # The worker reconcile retires an unreadable name's member.
  mutate reseat reconcile_retires_unreadable "Invariant worker_reconcile_spares_entries" <<'PY'
replace_text(" /\\ member \\notin arg_identity_entry_unreadable }\n    ELSE {}", " }\n    ELSE {}")
PY

  # The admission probe admits a reserved name (its name_not_reserved guard
  # dropped).
  mutate reseat probe_admits_reserved "Invariant AuditReseatProbeExactlyOne" <<'PY'
drop_guard("ProbeMemberAdmissionAdmittedRunning", "identity_entry_tombstones) = FALSE)")
PY

  # The forced destroy keeps the reserved names.
  mutate reseat destroy_keeps_tombstones "Invariant destroyed_mob_holds_no_member_lifecycle" <<'PY'
keep_var("DestroyFromAny", "identity_entry_tombstones")
PY

  # A declaration over a reserved name (its name_not_reserved guard
  # dropped).
  mutate names declare_over_reserved "Invariant AuditNamesDeclareExactlyOne" <<'PY'
drop_guard("DeclareIdentityEntrySeatOwed", "identity_entry_tombstones) = FALSE)")
PY

  # The tombstone refusal answers an unreadable name too (its name_readable
  # guard dropped).
  mutate names tombstone_refusal_over_unreadable "Invariant AuditNamesDeclareExactlyOne" <<'PY'
drop_guard("DeclareIdentityEntryRefusedNameTombstoned", "identity_entry_unreadable) = FALSE)")
PY

  # The declared apply takes a Created entry's name (the no entry or a
  # Declared entry guard dropped from every arm that carries it).
  mutate names declare_over_created "Invariant AuditNamesDeclareExactlyOne" <<'PY'
for verdict in ("Kept", "SeatOwed", "HostedOnFirstUse", "AwaitHeldCreation", "RuntimeModeRestamp",
                "RoleMigrate", "RoleRotate", "RoleMigrationHeld", "LifecycleInFlight",
                "RefusedNameHeldByWorker", "RefusedHostAdopted", "RefusedNonAuthoritative"):
    drop_guard("DeclareIdentityEntry" + verdict, "identity_entry_origins) = FALSE) \\/")
PY

  # The seal acts on Created entries as on Declared ones.
  mutate names seal_takes_created "Action property AuditSealNeverTouchesCreated" <<'PY'
text = open(path).read()
found = _arm(text, "SealDeclaredRosterSealed")
body = found.group(0)
old = '(identity_entry_origins[id] = "Declared")'
assert body.count(old) >= 2, "expected the seal's Declared filters"
open(path, "w").write(text[:found.start()] + body.replace(old, '(identity_entry_origins[id] \\in {"Declared", "Created"})') + text[found.end():])
PY

  # The seal acts on an unreadable name's Declared entry.
  mutate names seal_takes_unreadable "Action property AuditSealNeverTouchesCreated" <<'PY'
text = open(path).read()
found = _arm(text, "SealDeclaredRosterSealed")
body = found.group(0)
old = " /\\ ~(id \\in identity_entry_unreadable))"
assert body.count(old) >= 2, "expected the seal's readable filters"
open(path, "w").write(text[:found.start()] + body.replace(old, ")") + text[found.end():])
PY

  # The legacy adoption adopts a reserved name (its name_not_reserved guard
  # dropped).
  mutate names adopt_reserved "Invariant AuditNamesAdoptionExactlyOne" <<'PY'
drop_guard("AdoptLegacyIdentityEntryAdopted", "identity_entry_tombstones) = FALSE)")
PY

  # A Created entry answers the adoption as already an entry (its
  # declared_entry guard dropped).
  mutate names already_entry_over_created "Invariant AuditNamesAdoptionExactlyOne" <<'PY'
drop_guard("AdoptLegacyIdentityEntryAlreadyAnEntry", 'Some("Declared"))')
PY

  # The agent lane admits an unreadable name (its name_readable guard
  # dropped).
  mutate names lane_admits_unreadable "Invariant AuditNamesLaneExactlyOne" <<'PY'
drop_guard("ResolveAgentLaneMemberAdmissionAdmittedRunning", "identity_entry_unreadable) = FALSE)")
PY

  # A Present row of a placed member records it a Created entry (the
  # placement half of its member_seated_locally guard dropped).
  mutate placed seated_takes_placed "Invariant AuditRecoverExactlyOne" <<'PY'
replace_in_arm("RecoverCreatedIdentityEntrySeatedRunning",
               " /\\ ((agent_identity \\in DOMAIN member_placement) = FALSE)", "")
PY
fi
echo "created identity audit passed"
