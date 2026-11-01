#!/usr/bin/env bash
# Bounded TLC audit of incarnation-bound mob operator grants.
#
# usage: mob_operator_grant_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's finite
# model values (two mob ids, two incarnations, one spawn profile), its four
# action properties and its step bound, and checks them over the whole
# bounded space. It then reruns the same space once per reachability witness,
# with that witness's negation as an extra invariant, and requires TLC to
# report exactly that violation, so the properties are not vacuous. With
# --mutants it then seeds three defects into a copy of the generated model (a
# manage rebind that keeps the stale spawn scope, a restore without its
# bound-scope guard, a manage grant that records no binding) and requires TLC
# to refuse each with the property it breaks. An anchor this script expects
# but cannot find fails the run.
set -euo pipefail

usage="usage: mob_operator_grant_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 3 )); then
  echo "error: max-steps must be an integer >= 3 (the longest witness is 3 steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/mob-operator-grant-audit.XXXXXX")"
trap 'rm -rf "${work_dir}"' EXIT

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
  replace_exact_line "  StringValues = {}" '  StringValues = {"mobid_1", "mobid_2", "profile_1"}' "${cfg}"
  replace_exact_line "  SetOfStringValues = {{}}" '  SetOfStringValues = {{}, {"profile_1"}}' "${cfg}"
  replace_exact_line "  MobIncarnationIdValues = {}" \
    '  MobIncarnationIdValues = {"mobincarnationid_1", "mobincarnationid_2"}' "${cfg}"
  replace_exact_line "  OpaquePrincipalTokenValues = {}" \
    '  OpaquePrincipalTokenValues = {"opaqueprincipaltoken_1"}' "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}" "${cfg}"
  replace_exact_line "INVARIANTS" "PROPERTIES
  AuditGrantBindsTheNamedIncarnation
  AuditRebindDropsTheOtherScope
  AuditBoundIncarnationKeepsTheOtherScope
  AuditResolveClearsScope
INVARIANTS${extra}" "${cfg}"
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
cd "${spec_dir}"

# usage: run_tlc <name> <cfg> [extra tlc args...]; runs in the current
# directory and leaves the log at ${work_dir}/<name>.log
run_tlc() {
  local name="$1" cfg="$2"
  shift 2
  set +e
  # Witness runs end in a violation by design: -noGenerateSpecTE keeps TLC
  # from writing trace-exploration files into the spec directory.
  tlc -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" \
    mob_operator_grant_audit.tla > "${work_dir}/${name}.log" 2>&1
  local status=$?
  set -e
  return "${status}"
}

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" ""
safety_status=0
run_tlc safety "${safety_cfg}" "$@" || safety_status=$?
cat "${work_dir}/safety.log"
if [[ "${safety_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: mob operator grant audit failed (tlc exit ${safety_status}, bound ${max_steps})" >&2
  exit 1
fi

for witness in Rebound RebindDroppedSpawn RebindDroppedManage BothScopes \
  TwoIncarnations RestoredBindings ResolveCleared; do
  cfg="${work_dir}/witness-${witness}.cfg"
  write_cfg "${cfg}" "
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
    write_cfg "${cfg}" ""
    local status=0
    (cd "${dir}" && run_tlc "mutant-${name}" "${cfg}" "$@") || status=$?
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -qE "Error: (${violation}) is violated" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by ${violation} (tlc exit ${status})" >&2
      exit 1
    fi
    echo "mutant ${name} refused by $(grep -oE "Error: (${violation}) is violated" "${log}" | head -1)"
  }

  mutate rebind_keeps_spawn_scope "Action property AuditRebindDropsTheOtherScope" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^RebindMobOperatorManageMob\(.*?\n\n", text, re.S | re.M)
assert block, "RebindMobOperatorManageMob not found"
old = "    /\\ mob_operator_spawn_profile_scope' = MapRemove(mob_operator_spawn_profile_scope, mob_id)"
body = block.group(0)
assert body.count(old) == 1, "expected one spawn scope drop in RebindMobOperatorManageMob"
body = body.replace(old, "    /\\ mob_operator_spawn_profile_scope' = mob_operator_spawn_profile_scope")
text = text[: block.start()] + body + text[block.end():]
open(path, "w").write(text)
PY

  mutate restore_without_guard "Invariant mob_operator_scopes_are_bound_to_an_incarnation" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^RestoreMobOperatorAuthority\(.*?\n\n", text, re.S | re.M)
assert block, "RestoreMobOperatorAuthority not found"
old = ("    /\\ ((\\A mob_id \\in managed_mob_scope : (mob_id \\in DOMAIN mob_incarnations))"
       " /\\ (\\A mob_id \\in DOMAIN spawn_profile_scope : (mob_id \\in DOMAIN mob_incarnations)))")
body = block.group(0)
assert body.count(old) == 1, "expected one bound-scope guard in RestoreMobOperatorAuthority"
body = body.replace(old, "    /\\ TRUE")
text = text[: block.start()] + body + text[block.end():]
open(path, "w").write(text)
PY

  mutate grant_records_no_binding \
    "Invariant mob_operator_scopes_are_bound_to_an_incarnation|Action property AuditGrantBindsTheNamedIncarnation" \
    "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^GrantMobOperatorManageMob\(.*?\n\n", text, re.S | re.M)
assert block, "GrantMobOperatorManageMob not found"
old = "    /\\ mob_operator_mob_incarnations' = MapSet(mob_operator_mob_incarnations, mob_id, mob_incarnation)"
body = block.group(0)
assert body.count(old) == 1, "expected one binding update in GrantMobOperatorManageMob"
body = body.replace(old, "    /\\ mob_operator_mob_incarnations' = mob_operator_mob_incarnations")
text = text[: block.start()] + body + text[block.end():]
open(path, "w").write(text)
PY
fi
echo "mob operator grant audit passed at model_step_count <= ${max_steps}"
