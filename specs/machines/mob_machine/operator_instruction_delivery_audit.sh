#!/usr/bin/env bash
# TLC audit of the MobMachine operator-instruction region: the mob's desired
# records, the owed computation and each member's delivery facts, checked on
# the generated model as the shell drives it.
#
# usage: operator_instruction_delivery_audit.sh [--mutants] [extra tlc args...]
#
# Derives the TLC configs from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's
# properties, its generation bounds and its VIEW, which leaves out the two
# counters no guard reads (model_step_count and topology_epoch); each run
# explores its whole reachable space under its generation bound. It first
# checks that the generated model defines every arm the audit drives with the
# parameters it passes, and still reads neither counter anywhere else, so a
# renamed arm or field fails the run instead of a vacuous pass. Then:
# - the write run (AuditWriteSpec) and the seat run (AuditSeatSpec) each over
#   their whole space with TLC coverage: each must finish without an error and
#   fire every action of its covered list (each generated at least one
#   successor state);
# - one run per reachability witness, with that witness's negation as an
#   extra invariant (a state goal) or action property (a step goal),
#   requiring TLC to report exactly that violation;
# - with --mutants, five defects seeded into a copy of the generated model:
#   four refused by the property each targets, checked as the run's only
#   property:
#     profile_check_dropped (the target predicate selects every member for a
#       profile target) by AuditWriteOwesExactlyTheSelected;
#     unborn_seat_delivered (the prepared delivery's born-session guard
#       dropped) by AuditDeliversOnlyToBoundLocalBornMembers;
#     seat_forgets_to_owe (a replayed seat owes nothing) by
#       AuditSeatOwesTheGeneration;
#     unseat_keeps_owed (a replayed retirement keeps the obligation) by
#       AuditUnseatLeavesNoFact;
#   and deliver_only_live (a delivery prepared only for a Live member, as a
#   shell that woke members to deliver would need), which must leave the
#   DormantDeliverable witness unreachable.
# No run uses -continue: a witness run stops at the first state that reaches
# its witness, and a mutant run, whose only property is its target, exactly
# when some reachable state or step of the mutant breaks the target. Each TLC
# run is held to the canonical lane's per-run cap (`TLC_RUN_CAP_SECS`,
# default 900 s, see tlc_run_cap.sh); a run that hits it fails closed as TLC
# INCOMPLETE (exit 3). An anchor this script expects but cannot find fails
# the run.
set -euo pipefail

run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi

# The mob generation bound of each run: the write run reaches three committed
# writes (a write, a retarget and a re-owing write after a record or a
# refusal), the seat run two. A deeper bound may be run by hand through the
# environment; the witnesses need at least these.
max_write_generation="${OPERATOR_INSTRUCTION_DELIVERY_AUDIT_MAX_WRITE_GENERATION:-3}"
max_seat_generation="${OPERATOR_INSTRUCTION_DELIVERY_AUDIT_MAX_SEAT_GENERATION:-2}"
if ! [[ "${max_write_generation}" =~ ^[0-9]+$ ]] || (( max_write_generation < 3 )); then
  echo "error: the write run's generation bound must be an integer >= 3" >&2
  exit 2
fi
if ! [[ "${max_seat_generation}" =~ ^[0-9]+$ ]] || (( max_seat_generation < 2 )); then
  echo "error: the seat run's generation bound must be an integer >= 2" >&2
  exit 2
fi

# Every region arm the write run's domains can enable; each must fire.
write_covered_actions=(
  SetOperatorInstructionNewRunning
  SetOperatorInstructionAdvanceRunning
  SetOperatorInstructionDuplicateRunning
  SetOperatorInstructionStaleRunning
  SetOperatorInstructionConflictRunning
  RevokeOperatorInstructionUnknownSlotRunning
  RevokeOperatorInstructionStaleRunning
  RevokeOperatorInstructionDuplicateRunning
  RevokeOperatorInstructionConflictRunning
  RevokeOperatorInstructionAdvanceRunning
  PrepareMemberInstructionDeliveryNotOwedRunning
  PrepareMemberInstructionDeliveryDeliverRunning
  RecordMemberInstructionTargetNotOwedRunning
  RecordMemberInstructionTargetStaleSessionRunning
  RecordMemberInstructionTargetSupersededRunning
  RecordMemberInstructionTargetCurrentRunning
  RecordMemberInstructionTargetUnmintedRunning
  RefuseMemberInstructionTargetNotOwedRunning
  RefuseMemberInstructionTargetStaleSessionRunning
  RefuseMemberInstructionTargetSupersededRunning
  RefuseMemberInstructionTargetBlockedRunning
)

# The seat run's deferrals; each must fire.
seat_covered_actions=(
  PrepareMemberInstructionDeliveryUnbornRunning
  PrepareMemberInstructionDeliveryUnboundRunning
)

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/operator-instruction-delivery-audit.XXXXXX")"
# An interrupted or killed run ends its TLC child too (tlc_run_cap.sh).
trap 'if declare -F tlc_run_cap_reap >/dev/null; then tlc_run_cap_reap; fi; rm -rf "${work_dir}"' EXIT

# The premise: the generated model defines each arm the audit drives with the
# parameters, in the order, the audit passes them, and the target predicate
# the profile mutant replaces.
MODEL="${spec_dir}/model.tla" python3 - <<'PY'
import os, re
text = open(os.environ["MODEL"]).read()
write = "slot, revision, digest, body_bytes, target_kind, target_profile, target_member"
revoke = "slot, revision"
prepare = "agent_identity"
record = "agent_identity, session_id, generation"
refuse = "agent_identity, session_id, generation, block"
arms = {
    "RecoverRosterMemberRunning": "agent_identity, agent_runtime_id, fence_token, generation, profile_name, runtime_mode, external_addressable",
    "RecoverMemberSessionBindingFreshRunning": "agent_identity, agent_runtime_id, bridge_session_id, replacing",
    "RecoverMemberSeatSessionUnbornRunning": "agent_identity, agent_runtime_id, unborn",
    "RecoverMemberSeatSessionBornRunning": "agent_identity, agent_runtime_id, unborn",
    "RecoverRosterMemberRetiredRunning": "agent_identity, agent_runtime_id, generation, preserve_machine_topology, preservation_started, session_archived, hosted_incarnation",
    "SetOperatorInstructionNewRunning": write,
    "SetOperatorInstructionAdvanceRunning": write,
    "SetOperatorInstructionDuplicateRunning": write,
    "SetOperatorInstructionStaleRunning": write,
    "SetOperatorInstructionConflictRunning": write,
    "RevokeOperatorInstructionUnknownSlotRunning": revoke,
    "RevokeOperatorInstructionStaleRunning": revoke,
    "RevokeOperatorInstructionDuplicateRunning": revoke,
    "RevokeOperatorInstructionConflictRunning": revoke,
    "RevokeOperatorInstructionAdvanceRunning": revoke,
    "PrepareMemberInstructionDeliveryNotOwedRunning": prepare,
    "PrepareMemberInstructionDeliveryUnbornRunning": prepare,
    "PrepareMemberInstructionDeliveryUnboundRunning": prepare,
    "PrepareMemberInstructionDeliveryDeliverRunning": prepare,
    "RecordMemberInstructionTargetNotOwedRunning": record,
    "RecordMemberInstructionTargetStaleSessionRunning": record,
    "RecordMemberInstructionTargetSupersededRunning": record,
    "RecordMemberInstructionTargetCurrentRunning": record,
    "RecordMemberInstructionTargetUnmintedRunning": record,
    "RefuseMemberInstructionTargetNotOwedRunning": refuse,
    "RefuseMemberInstructionTargetStaleSessionRunning": refuse,
    "RefuseMemberInstructionTargetSupersededRunning": refuse,
    "RefuseMemberInstructionTargetBlockedRunning": refuse,
}
for arm, params in arms.items():
    heads = re.findall(r"(?m)^" + arm + r"\(([^)]*)\) ==$", text)
    assert heads == [params], f"expected {arm}({params}) in model.tla, found {heads}"
helper = re.findall(
    r"(?m)^operator_instruction_target_applies\(([^)]*)\) == ", text
)
assert helper == ["kind, profile, member, member_profile, agent_identity"], (
    f"expected the operator_instruction_target_applies helper in model.tla, found {helper}"
)
variables = [line for line in text.split("\n") if line.startswith("VARIABLES ")]
assert len(variables) == 1, "expected one VARIABLES line in model.tla"
for name in (
    "operator_instruction_revision",
    "operator_instruction_effect",
    "operator_instruction_target_kind",
    "operator_instruction_target_profile",
    "operator_instruction_target_member",
    "operator_instruction_generation",
    "member_instruction_owed",
    "member_instruction_recorded",
    "member_instruction_blocked",
):
    assert re.search(r"\b" + name + r"\b", variables[0]), f"model.tla declares no variable {name}"
print("premise holds: the generated model defines every operator-instruction arm the audit drives")
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

audit_properties="  AuditWriteOwesExactlyTheSelected
  AuditSeatOwesTheGeneration
  AuditUnseatLeavesNoFact
  AuditRegionWakesNothing"
audit_invariants="  AuditRecordedWithinGeneration
  AuditOwedIsSeatedAndUnblocked
  AuditDeliversOnlyToBoundLocalBornMembers"

# usage: write_cfg <run> <file> <extra invariant lines> <extra property lines>
# where <run> is write (AuditWriteSpec under AuditWriteConstraint) or seat
# (AuditSeatSpec under AuditSeatConstraint).
write_cfg() {
  local run="$1" cfg="$2" invariants="$3" properties="$4"
  local spec constraint
  case "${run}" in
    write) spec="AuditWriteSpec"; constraint="AuditWriteConstraint" ;;
    seat) spec="AuditSeatSpec"; constraint="AuditSeatConstraint" ;;
    *)
      echo "error: unknown audit run '${run}'" >&2
      exit 1
      ;;
  esac
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION ${spec}" "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditStepIndex = ${step_index}
  AuditEpochIndex = ${epoch_index}
  AuditMaxWriteGeneration = ${max_write_generation}
  AuditMaxSeatGeneration = ${max_seat_generation}" "${cfg}"
  replace_exact_line "INVARIANTS" "PROPERTIES
${audit_properties}${properties}
INVARIANTS
${audit_invariants}${invariants}" "${cfg}"
  replace_exact_line "CONSTRAINTS" "VIEW AuditView
CONSTRAINTS" "${cfg}"
  replace_exact_line "  CiStateConstraint" "  ${constraint}" "${cfg}"
  printf 'CHECK_DEADLOCK FALSE\n' >> "${cfg}"
}

# usage: write_target_cfg <run> <file> <kind> <target>; the run's config with
# <target> as its only invariant (kind Invariant) or only property (kind
# "Action property"), every other invariant and property left out.
write_target_cfg() {
  local run="$1" cfg="$2" kind="$3" target="$4"
  write_cfg "${run}" "${cfg}" "" ""
  CFG="${cfg}" KIND="${kind}" TARGET="${target}" python3 - <<'PY'
import os, re
path, kind, target = os.environ["CFG"], os.environ["KIND"], os.environ["TARGET"]
lines = open(path).read().split("\n")
kept = []
skipping = False
for line in lines:
    if line in ("PROPERTIES", "INVARIANTS"):
        skipping = True
        continue
    if skipping and line.startswith("  "):
        continue
    skipping = False
    kept.append(line)
assert "VIEW AuditView" in kept, f"no VIEW in {path}"
section = {"Invariant": "INVARIANTS", "Action property": "PROPERTIES"}[kind]
at = kept.index("VIEW AuditView")
kept[at:at] = [section, "  " + target]
for keyword in ("SPECIFICATION", "CONSTANTS", "CONSTRAINTS"):
    assert any(re.fullmatch(keyword + r"( .*)?", line) for line in kept), f"{keyword} missing from {path}"
open(path, "w").write("\n".join(kept))
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
  tlc_run_capped operator_instruction_delivery_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" ${extra_tlc_args[@]+"${extra_tlc_args[@]}"} \
    "${dir}/operator_instruction_delivery_audit.tla" || tlc_status=$?
}

# usage: check_space <run> <covered action...>; runs <run> over its whole
# space with coverage and checks the outcome and the covered actions.
check_space() {
  local run="$1"
  shift
  local cfg="${work_dir}/${run}.cfg"
  write_cfg "${run}" "${cfg}" "" ""
  run_tlc "${run}" "${cfg}" "${spec_dir}" -coverage 1
  local log="${work_dir}/${run}.log"
  cat "${log}"
  if [[ "${tlc_status}" != "0" ]] \
    || ! grep -q "Model checking completed. No error has been found." "${log}"; then
    echo "error: operator-instruction delivery audit ${run} run failed (tlc exit ${tlc_status})" >&2
    exit 1
  fi
  local distinct
  distinct="$(sed -nE 's/^[0-9]+ states generated, ([0-9]+) distinct states found, 0 states left on queue\.$/\1/p' \
    "${log}" | tail -1)"
  if [[ -z "${distinct}" ]]; then
    echo "error: the ${run} run reported no final distinct-state count" >&2
    exit 1
  fi
  echo "${run} distinct states: ${distinct}"

  # TLC prints a coverage block every minute and one at the end; read the
  # last. An action's line is
  # `<Action line L, col C to line L, col C of module model>: D:T`, with D the
  # new distinct states it found and T the successor states it generated; an
  # arm that changes nothing the VIEW keeps (a verdict) finds no new state,
  # so T counts.
  local coverage="${work_dir}/${run}-coverage.txt"
  awk '/^The coverage statistics at / { block = ""; next } { block = block $0 "\n" } END { printf "%s", block }' \
    "${log}" > "${coverage}"
  local action generated
  for action in "$@"; do
    generated="$(awk -v head="<${action} line " '
      index($0, head) == 1 && $0 ~ / of module (model|operator_instruction_delivery_audit)( \([0-9 ]+\))?>: [0-9]+:[0-9]+$/ {
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

check_space write "${write_covered_actions[@]}"
check_space seat "${seat_covered_actions[@]}"

# Each witness is <run>:<kind>:<name>: a state goal's negation joins the
# invariants, a step goal's negation ([][~step]_vars) joins the properties.
# The whole-space runs showed every property holds, so a witness run stops
# at the first state or step that reaches its witness.
for witness in \
  "write:Action property:ProfileWriteOwesOne" \
  "write:Action property:RetargetOwesLeaver" \
  "write:Action property:RecordedReowed" \
  "write:Action property:BlockedThenUnblocked" \
  "write:Action property:RecordCloses" \
  "write:Invariant:DormantDeliverable" \
  "seat:Action property:SeatAfterWriteOwed" \
  "seat:Action property:UnseatDropsFacts" \
  "seat:Invariant:UnbornOwedDeferred" \
  "seat:Invariant:UnboundOwedDeferred"; do
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
  run_tlc "witness-${name}" "${cfg}" "${spec_dir}"
  log="${work_dir}/witness-${name}.log"
  if ! grep -q "Error: ${kind} NotAuditWitness${name} is violated" "${log}"; then
    cat "${log}"
    echo "error: witness ${name} is not reachable (tlc exit ${tlc_status})" >&2
    exit 1
  fi
  depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
  echo "witness ${name} reached in ${depth} states"
done

if [[ "${run_mutants}" == "true" ]]; then
  # usage: seed_mutant <name> <arm> <anchor> <new line> [<anchor> <new line>...]
  # Copies the specs into the mutant's directory and replaces the one line of
  # <arm> (an action or a helper operator of the generated model) that
  # contains each <anchor> with its <new line>.
  seed_mutant() {
    local name="$1" arm="$2"
    shift 2
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}/model.tla" "${spec_dir}/operator_instruction_delivery_audit.tla" "${dir}/"
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
  }

  # usage: mutate <run> <name> <kind> <target> <arm> <anchor> <new line>...
  # Seeds the mutant, runs <run> with <target> as its only invariant or
  # property, and requires TLC to report it violated.
  mutate() {
    local run="$1" name="$2" kind="$3" target="$4"
    shift 4
    seed_mutant "${name}" "$@"
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_target_cfg "${run}" "${cfg}" "${kind}" "${target}"
    run_tlc "mutant-${name}" "${cfg}" "${work_dir}/mutant-${name}"
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -q "Error: ${kind} ${target} is violated" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by ${target} (tlc exit ${tlc_status})" >&2
      exit 1
    fi
    local depth
    depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
    echo "mutant ${name} refused by ${target} in ${depth} states"
  }

  # usage: mutate_unreachable <run> <name> <witness> <arm> <anchor> <new line>...
  # Seeds the mutant and requires the state goal <witness> to be unreachable
  # in it: TLC finishes without an error with the witness's negation as an
  # extra invariant.
  mutate_unreachable() {
    local run="$1" name="$2" witness="$3"
    shift 3
    seed_mutant "${name}" "$@"
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${run}" "${cfg}" "
  NotAuditWitness${witness}" ""
    run_tlc "mutant-${name}" "${cfg}" "${work_dir}/mutant-${name}"
    local log="${work_dir}/mutant-${name}.log"
    if [[ "${tlc_status}" != "0" ]] \
      || ! grep -q "Model checking completed. No error has been found." "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} still reaches witness ${witness} (tlc exit ${tlc_status})" >&2
      exit 1
    fi
    echo "mutant ${name} makes witness ${witness} unreachable"
  }

  # The target predicate selects every member for a profile target.
  mutate write profile_check_dropped "Action property" AuditWriteOwesExactlyTheSelected \
    operator_instruction_target_applies 'member_profile = Some(profile)' \
    'operator_instruction_target_applies(kind, profile, member, member_profile, agent_identity) == ((kind = "Mob") \/ (kind = "Profile") \/ ((kind = "Member") /\ (member = Some(agent_identity))))'
  # A delivery is prepared for an unborn seat: the born-session guard dropped.
  mutate seat unborn_seat_delivered Invariant AuditDeliversOnlyToBoundLocalBornMembers \
    PrepareMemberInstructionDeliveryDeliverRunning 'member_seat_session_unborn' \
    '    /\ TRUE'
  # A replayed seat owes the member nothing.
  mutate seat seat_forgets_to_owe "Action property" AuditSeatOwesTheGeneration \
    RecoverRosterMemberRunning "member_instruction_owed' =" \
    "    /\\ member_instruction_owed' = MapRemove(member_instruction_owed, agent_identity)"
  # A replayed retirement keeps the member's obligation.
  mutate seat unseat_keeps_owed "Action property" AuditUnseatLeavesNoFact \
    RecoverRosterMemberRetiredRunning "member_instruction_owed' =" \
    "    /\\ member_instruction_owed' = member_instruction_owed"
  # A delivery is prepared only for a Live member, as a shell that woke a
  # member to deliver to it would need: no Dormant member's delivery remains.
  mutate_unreachable write deliver_only_live DormantDeliverable \
    PrepareMemberInstructionDeliveryDeliverRunning 'member_session_bindings' \
    '    /\ ((agent_identity \in DOMAIN member_session_bindings) /\ (agent_identity \in DOMAIN member_materialization) /\ (member_materialization[agent_identity] = "Live"))'
fi
echo "operator-instruction delivery audit passed"
