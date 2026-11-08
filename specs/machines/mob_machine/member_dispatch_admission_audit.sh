#!/usr/bin/env bash
# TLC audit of the generated MobMachine member dispatch admission verdict and
# record: the four ResolveMemberDispatchAdmission* arms partition the
# observations (the lineage MobMachine records for the member's current
# incarnation, a build's observed lineage and the host's answer), each takes
# the verdict the shell mirrors, a lineage that runs under a tool dispatch
# admission is never built without one, a record of an earlier incarnation
# never matches, an Unknown lineage (a fork-derived member MobMachine holds
# no record for) is built without one only when the host was asked and
# derived none, and RecordMemberDispatchAdmissionLineage records only the
# identity's current incarnation of a local member.
#
# usage: member_dispatch_admission_audit.sh [--mutants] [extra tlc args...]
#
# Derives the TLC configs from the generated ci.cfg next to this script
# (every generated constant and invariant, with the identity and runtime id
# constants given the audit member's values) and adds the audit's
# invariants. The audit checks the initial state alone, with ENABLED over
# every observation pair in every state the audit member can be in (the
# variables the arms read substituted through a parameterized INSTANCE);
# that stands for every reachable state of every phase only while the arms
# read no state but phase and those variables, and while each arm's copies
# in the other phases have the guards of its Running copy, so this script
# checks that premise on the generated model first and fails if it stops
# holding. Then:
# - the safety run checks the invariants on the initial state;
# - the step run takes one classification step over every pair with TLC
#   coverage, and requires every arm of its covered list to fire;
# - the witness run requires its reachability witness reported violated;
# - with --mutants, one defect seeded into each verdict arm of a copy of the
#   generated model, each refused by the invariant it targets (every
#   invariant is checked with -continue, and AuditReportGaps names the pairs
#   the defect leaves with no arm or several):
#     ask_host_after_answer (AskHost's not-yet-asked guard dropped) by
#       AuditHostAskedOnlyForGatedOrUnknownLineage;
#     inherit_without_admission (Inherit taken whenever the host was asked)
#       by AuditInheritNeedsDerived;
#     refused_never_reached (Refused's guard never holds) by
#       AuditVerdictTable;
#     none_required_for_any_lineage (NoneRequired's lineage guard dropped)
#       by AuditGatedLineageNeverBuiltUngated;
#     unknown_built_without_asking (NoneRequired taken for an Unknown
#       lineage whatever the host answered) by
#       AuditUnknownLineageUngatedOnlyWhenTheHostDerivedNone;
#     unknown_refused (Refused taken for an Unknown lineage the host derived
#       none for) by AuditRefusedOnlyForGatedNoneDerived;
#     record_matches_any_incarnation (the verdict's record match drops the
#       incarnation) by AuditStaleRecordNeverMatches;
#     recorded_lineage_needs_none (NoneRequired's record guard dropped) by
#       AuditGatedLineageNeverBuiltUngated;
#     record_any_incarnation (the record's incarnation guard dropped) and
#       record_placed_member (its locality guard dropped) by
#       AuditRecordOnlyTheCurrentLocalIncarnation.
# Each TLC run is held to the canonical lane's per-run cap (tlc_run_cap.sh)
# and fails closed as TLC INCOMPLETE. An anchor this script expects but
# cannot find fails the run.
set -euo pipefail

run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi

# The step run's arms; each must fire. Their copies in the other phases are
# held to the same guards by the premise below.
step_covered_actions=(
  ResolveMemberDispatchAdmissionAskHostRunning
  ResolveMemberDispatchAdmissionInheritRunning
  ResolveMemberDispatchAdmissionRefusedRunning
  ResolveMemberDispatchAdmissionNoneRequiredRunning
)

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/member-dispatch-admission-audit.XXXXXX")"
# An interrupted or killed run ends its TLC child too (tlc_run_cap.sh).
trap 'if declare -F tlc_run_cap_reap >/dev/null; then tlc_run_cap_reap; fi; rm -rf "${work_dir}"' EXIT

# The premise: each verdict arm keeps its phase and changes only the step
# count, each record arm keeps its phase and changes only the record and the
# step count; their guards read no state variable but phase,
# identity_to_runtime, member_dispatch_admission_lineage and (the record
# arms) member_placement, the variables the audit substitutes; each arm's
# copies in the other phases have its Running copy's guards but for the
# phase.
MODEL="${spec_dir}/model.tla" python3 - <<'PY'
import os, re
text = open(os.environ["MODEL"]).read()
declared = re.search(r"^VARIABLES (.*)$", text, re.M)
assert declared, "VARIABLES line not found in model.tla"
variables = {name.strip() for name in declared.group(1).split(",")}
arms = [
    (
        "ResolveMemberDispatchAdmission" + verdict,
        ("Running", "Stopped", "Completed", "Destroyed"),
        "agent_identity, lineage, host_answer",
        [],
        {"phase", "identity_to_runtime", "member_dispatch_admission_lineage"},
    )
    for verdict in ("AskHost", "Inherit", "Refused", "NoneRequired")
] + [
    (
        "RecordMemberDispatchAdmissionLineage",
        ("Running", "Stopped"),
        "agent_identity, agent_runtime_id",
        ["member_dispatch_admission_lineage"],
        {"phase", "identity_to_runtime", "member_placement"},
    )
]
for name, phases, params, changed, readable in arms:
    guards_by_phase = {}
    for phase in phases:
        arm = name + phase
        heads = re.findall(r"(?m)^" + arm + r"\(([^)]*)\) ==$", text)
        assert heads == [params], f"expected one {arm}({params}), found {heads}"
        start = text.index("\n" + arm + "(") + 1
        end = text.index("\n\n", start)
        lines = text[start:end].split("\n")[1:]
        primed = sorted(line for line in lines if "'" in line)
        expected_primed = [
            "    /\\ phase' = \"" + phase + "\"",
            "    /\\ model_step_count' = model_step_count + 1",
        ]
        updates = [line for line in primed if line not in expected_primed]
        assert all(line in primed for line in expected_primed), f"{arm} no longer keeps its phase and counts its step: {primed}"
        updated = sorted({re.match(r"    /\\ ([a-z_]+)' = ", line).group(1) for line in updates if re.match(r"    /\\ ([a-z_]+)' = ", line)})
        assert updated == sorted(changed) and len(updates) == len(changed), f"{arm} changes {updates}, expected only {changed}"
        frames = [line for line in lines if re.fullmatch(r"    /\\ UnchangedFrame_[0-9a-f]+", line)]
        assert len(frames) == 1, f"{arm} has no single UNCHANGED frame"
        phase_guard = "    /\\ phase = \"" + phase + "\""
        guards = [line for line in lines if "'" not in line and line not in frames]
        assert phase_guard in guards, f"{arm} is no longer guarded on phase {phase}"
        others = [line for line in guards if line != phase_guard]
        assert others, f"{arm} has no guard but its phase"
        for line in others:
            bare = re.sub(r'"[^"]*"', '""', line)
            read = set(re.findall(r"[A-Za-z_][A-Za-z0-9_]*", bare)) & variables
            assert read <= readable, f"{arm} now reads state {sorted(read - readable)}: the audited states no longer stand for every state"
        guards_by_phase[phase] = others
    for phase in phases[1:]:
        assert guards_by_phase[phase] == guards_by_phase["Running"], (
            f"{name}{phase} guards differ from its Running copy"
        )
print("premise holds: the dispatch admission arms read no state but phase and the substituted variables, alike in every phase")
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

audit_invariants="  AuditVerdictIsTotalAndDisjoint
  AuditVerdictTable
  AuditGatedLineageNeverBuiltUngated
  AuditStaleRecordNeverMatches
  AuditInheritNeedsDerived
  AuditRefusedOnlyForGatedNoneDerived
  AuditHostAskedOnlyForGatedOrUnknownLineage
  AuditUnknownLineageUngatedOnlyWhenTheHostDerivedNone
  AuditRecordOnlyTheCurrentLocalIncarnation"

# usage: write_cfg <file> <spec> <extra invariant lines>; the step run
# (AuditStepSpec) takes the audit's one-step constraint. The identity and
# runtime id constants take the audit member's values: the generated ci.cfg
# leaves them empty.
write_cfg() {
  local cfg="$1" spec="$2" extra="$3"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION ${spec}" "${cfg}"
  replace_exact_line "INVARIANTS" "INVARIANTS
${audit_invariants}${extra}" "${cfg}"
  replace_exact_line "  AgentIdentityValues = {}" "  AgentIdentityValues = {\"audit-member\"}" "${cfg}"
  replace_exact_line "  AgentRuntimeIdValues = {}" \
    "  AgentRuntimeIdValues = {\"audit-runtime-1\", \"audit-runtime-2\"}" "${cfg}"
  if [[ "${spec}" == "AuditStepSpec" ]]; then
    replace_exact_line "  CiStateConstraint" "  AuditStepConstraint" "${cfg}"
  fi
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
  tlc_run_capped member_dispatch_admission_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" ${extra_tlc_args[@]+"${extra_tlc_args[@]}"} \
    "${dir}/member_dispatch_admission_audit.tla" || tlc_status=$?
}

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" AuditSpec ""
run_tlc safety "${safety_cfg}" "${spec_dir}"
cat "${work_dir}/safety.log"
if [[ "${tlc_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: member dispatch admission audit failed (tlc exit ${tlc_status})" >&2
  exit 1
fi

step_cfg="${work_dir}/step.cfg"
write_cfg "${step_cfg}" AuditStepSpec ""
run_tlc step "${step_cfg}" "${spec_dir}" -coverage 1
step_log="${work_dir}/step.log"
cat "${step_log}"
if [[ "${tlc_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${step_log}"; then
  echo "error: member dispatch admission step run failed (tlc exit ${tlc_status})" >&2
  exit 1
fi
# TLC prints a coverage block every minute and one at the end; read the
# last. An action's line is
# `<Action line L, col C to line L, col C of module model>: D:T`, with D the
# new distinct states it found and T the successor states it generated; a
# verdict arm changes only the step count, so T counts.
coverage="${work_dir}/step-coverage.txt"
awk '/^The coverage statistics at / { block = ""; next } { block = block $0 "\n" } END { printf "%s", block }' \
  "${step_log}" > "${coverage}"
for action in "${step_covered_actions[@]}"; do
  generated="$(awk -v head="<${action} line " '
    index($0, head) == 1 && $0 ~ / of module (model|member_dispatch_admission_audit)( \([0-9 ]+\))?>: [0-9]+:[0-9]+$/ {
      split($NF, counts, ":"); total += counts[2]; found = 1
    }
    END { if (found) print total }' "${coverage}")"
  if [[ -z "${generated}" || "${generated}" == "0" ]]; then
    echo "error: step run: ${action} never fired (coverage '${generated:-absent}')" >&2
    exit 1
  fi
  echo "covered ${action}: ${generated} successor states"
done

witness_cfg="${work_dir}/witness-Running.cfg"
write_cfg "${witness_cfg}" AuditSpec "
  NotAuditWitnessRunning"
run_tlc witness-Running "${witness_cfg}" "${spec_dir}"
if ! grep -q "Error: Invariant NotAuditWitnessRunning is violated" "${work_dir}/witness-Running.log"; then
  cat "${work_dir}/witness-Running.log"
  echo "error: witness Running is not reached (tlc exit ${tlc_status})" >&2
  exit 1
fi
echo "witness Running reached"

if [[ "${run_mutants}" == "true" ]]; then
  # usage: mutate <name> <target invariant> <arm> <old> <new> [<arm> <old> <new>...]
  # Copies the specs, replaces exactly one <old> by <new> in each named arm,
  # runs every invariant with -continue and AuditReportGaps, and requires
  # the target among the invariants TLC reports violated.
  mutate() {
    local name="$1" target="$2"
    shift 2
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}/model.tla" "${spec_dir}/member_dispatch_admission_audit.tla" "${dir}/"
    MODEL="${dir}/model.tla" python3 - "$@" <<'PY'
import os, sys
path = os.environ["MODEL"]
text = open(path).read()
edits = sys.argv[1:]
assert edits and len(edits) % 3 == 0, "edits come in (arm, old, new) triples"
for i in range(0, len(edits), 3):
    arm, old, new = edits[i:i + 3]
    start = text.index("\n" + arm + "(") + 1
    end = text.index("\n\n", start)
    block = text[start:end]
    count = block.count(old)
    assert count == 1, f"expected one {old!r} in {arm}, found {count}"
    text = text[:start] + block.replace(old, new) + text[end:]
open(path, "w").write(text)
PY
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${cfg}" AuditSpec "
  AuditReportGaps"
    run_tlc "mutant-${name}" "${cfg}" "${dir}" -continue
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -q "Error: Invariant ${target} is violated" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by ${target} (tlc exit ${tlc_status})" >&2
      exit 1
    fi
    local broken gaps
    broken="$(grep -oE 'Error: Invariant [A-Za-z]+ is violated' "${log}" \
      | sed -E 's/Error: Invariant ([A-Za-z]+) is violated/\1/' | sort -u | tr '\n' ' ')"
    # TLC pretty-prints a long PrintT value over several lines.
    gaps="$(LOG="${log}" python3 -c '
import os, re
text = open(os.environ["LOG"]).read()
found = re.findall(r"<<\s*\"cases with.*?(?:>>\s*>>|\"none\">>)", text, re.S)
print("; ".join(" ".join(item.split()) for item in found))
')"
    echo "mutant ${name} refused by ${target}; breaks: ${broken}; ${gaps}"
  }

  A='ResolveMemberDispatchAdmissionAskHostRunning'
  I='ResolveMemberDispatchAdmissionInheritRunning'
  R='ResolveMemberDispatchAdmissionRefusedRunning'
  N='ResolveMemberDispatchAdmissionNoneRequiredRunning'

  # AskHost: the host is asked again after it answered.
  mutate ask_host_after_answer AuditHostAskedOnlyForGatedOrUnknownLineage \
    "${A}" '(host_answer = "NotAsked")' 'TRUE'
  # Inherit: the build inherits whenever the host was asked, an admission
  # derived or not.
  mutate inherit_without_admission AuditInheritNeedsDerived \
    "${I}" '(host_answer = "Derived")' '(host_answer # "NotAsked")'
  # Refused: a gated lineage the host derived none for is never refused.
  mutate refused_never_reached AuditVerdictTable \
    "${R}" '(host_answer = "NoneDerived")' 'FALSE'
  # NoneRequired: any lineage builds with what its request carries, a gated
  # one included.
  mutate none_required_for_any_lineage AuditGatedLineageNeverBuiltUngated \
    "${N}" '(IF (lineage = "NoneRecorded") THEN TRUE ELSE (IF (lineage = "SourceBuiltUngated") THEN TRUE ELSE (IF (lineage = "SourceRebuildUngated") THEN TRUE ELSE ((lineage = "Unknown") /\ (host_answer = "NoneDerived")))))' 'TRUE'
  # NoneRequired: an Unknown lineage is built with what its request carries
  # whatever the host answered, before it is asked included.
  mutate unknown_built_without_asking AuditUnknownLineageUngatedOnlyWhenTheHostDerivedNone \
    "${N}" '((lineage = "Unknown") /\ (host_answer = "NoneDerived"))' '(lineage = "Unknown")'
  # Refused: an Unknown lineage the host derived none for is refused, so a
  # host without a customizer could no longer rebuild it.
  mutate unknown_refused AuditRefusedOnlyForGatedNoneDerived \
    "${R}" '(lineage = "SourceRebuildGated")' '((lineage = "SourceRebuildGated") \/ (lineage = "Unknown"))'

  # The record's lookups, as the generated guards render them.
  L='(IF (agent_identity \in DOMAIN member_dispatch_admission_lineage) THEN Some((IF agent_identity \in DOMAIN member_dispatch_admission_lineage THEN member_dispatch_admission_lineage[agent_identity] ELSE "None")) ELSE None)'
  C='(IF (agent_identity \in DOMAIN identity_to_runtime) THEN Some((IF agent_identity \in DOMAIN identity_to_runtime THEN identity_to_runtime[agent_identity] ELSE "None")) ELSE None)'
  W='RecordMemberDispatchAdmissionLineageRunning'

  # The verdict's record match: a record of any incarnation of the identity
  # matches, a stale one included.
  mutate record_matches_any_incarnation AuditStaleRecordNeverMatches \
    "${A}" "(${L} = ${C})" 'TRUE' \
    "${I}" "(${L} = ${C})" 'TRUE' \
    "${R}" "(${L} = ${C})" 'TRUE' \
    "${N}" "(${L} # ${C})" 'FALSE'
  # NoneRequired's record guard: a member whose current incarnation is
  # recorded builds with what its request carries.
  mutate recorded_lineage_needs_none AuditGatedLineageNeverBuiltUngated \
    "${N}" '((agent_identity \in DOMAIN member_dispatch_admission_lineage) = FALSE)' 'TRUE'
  # The record's incarnation guard: any incarnation of the identity is
  # recorded, not only its current one.
  mutate record_any_incarnation AuditRecordOnlyTheCurrentLocalIncarnation \
    "${W}" "(${C} = Some(agent_runtime_id))" 'TRUE'
  # The record's locality guard: a placed member is recorded.
  mutate record_placed_member AuditRecordOnlyTheCurrentLocalIncarnation \
    "${W}" '((agent_identity \in DOMAIN member_placement) = FALSE)' 'TRUE'
fi
echo "member dispatch admission audit passed over every observation pair"
