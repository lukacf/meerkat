#!/usr/bin/env bash
# Bounded TLC audit of the empty-resume retirement verdict and the
# registration's residency owner.
#
# usage: empty_resume_residency_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's finite
# model values (one session, one runtime id), its invariant, its two action
# properties and its step bound, and checks them over the whole bounded
# space. It then reruns the same space once per reachability witness, with
# that witness's negation as an extra action property, and requires TLC to
# report exactly that violation, so the properties are not vacuous. With
# --mutants it then seeds two defects into a copy of the generated model (the
# Retire verdict without its owner guard, a placement that records no owner)
# and requires TLC to refuse each with the property it breaks. An anchor this
# script expects but cannot find fails the run. Each TLC run is held to the
# canonical lane's per-run cap (tlc_run_cap.sh) and fails closed as TLC
# INCOMPLETE.
set -euo pipefail

usage="usage: empty_resume_residency_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 4 )); then
  echo "error: max-steps must be an integer >= 4 (the longest witness is 4 steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/empty-resume-residency-audit.XXXXXX")"
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

# usage: write_cfg <file> <extra property lines>
write_cfg() {
  local cfg="$1" extra="$2"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1"}' "${cfg}"
  replace_exact_line "  AgentRuntimeIdValues = {}" '  AgentRuntimeIdValues = {"runtime_1"}' "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}" "${cfg}"
  replace_exact_line "INVARIANTS" "PROPERTIES
  AuditRetireOnlySessionOwned
  AuditPlacementOwnership${extra}
INVARIANTS
  AuditVerdictExactlyOne" "${cfg}"
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
# shellcheck source=tlc_run_cap.sh
source "${spec_dir}/tlc_run_cap.sh"
cd "${spec_dir}"

# usage: run_tlc <name> <cfg> [extra tlc args...]; runs in the current
# directory and leaves the log at ${work_dir}/<name>.log
run_tlc() {
  local name="$1" cfg="$2"
  shift 2
  local status=0
  # Witness and mutant runs end in a violation by design: -noGenerateSpecTE
  # keeps TLC from writing trace-exploration files into the spec directory.
  tlc_run_capped empty_resume_residency_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" empty_resume_residency_audit.tla || status=$?
  return "${status}"
}

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" ""
safety_status=0
run_tlc safety "${safety_cfg}" "$@" || safety_status=$?
cat "${work_dir}/safety.log"
if [[ "${safety_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: empty-resume residency audit failed (tlc exit ${safety_status}, bound ${max_steps})" >&2
  exit 1
fi

for witness in SessionOwnedRetires CompositionKeepsServing QueuedKeepsServing CompositionClaimed; do
  cfg="${work_dir}/witness-${witness}.cfg"
  write_cfg "${cfg}" "
  NotAuditWitness${witness}"
  status=0
  run_tlc "witness-${witness}" "${cfg}" "$@" || status=$?
  log="${work_dir}/witness-${witness}.log"
  if ! grep -q "Error: Action property NotAuditWitness${witness} is violated" "${log}"; then
    cat "${log}"
    echo "error: witness ${witness} is not reachable at bound ${max_steps} (tlc exit ${status})" >&2
    exit 1
  fi
  depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
  echo "witness ${witness} reached in ${depth} states"
done

if [[ "${run_mutants}" == "true" ]]; then
  # Shared edit helpers for the mutants: each mutant's python edit runs after
  # this prelude, rewrites the copied model.tla in place and must find each
  # anchor it names.
  read -r -d '' mutant_prelude <<'PY' || true
import os, re
path = os.environ["MODEL"]
text = open(path).read()

def _replace_body(arm, edit):
    global text
    found = list(re.finditer(r"^" + re.escape(arm) + r"(\([^)]*\))? ==\n.*?\n\n", text, re.S | re.M))
    assert len(found) == 1, f"expected one definition of {arm}, found {len(found)}"
    start, end = found[0].start(), found[0].end()
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

def save():
    open(path, "w").write(text)
PY

  # usage: mutate <name> <expected refusal regex> <python edit> [extra tlc args...]
  mutate() {
    local name="$1" refusal="$2" edit="$3"
    shift 3
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}"/*.tla "${dir}/"
    printf '%s\n%s\nsave()\n' "${mutant_prelude}" "${edit}" | MODEL="${dir}/model.tla" python3 -
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${cfg}" ""
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

  # The Retire verdict without its owner guard: a Composition-owned
  # registration with nothing queued would be unloaded (both verdicts answer).
  mutate verdict_ignores_owner \
    "Error: (Invariant AuditVerdictExactlyOne|Action property AuditRetireOnlySessionOwned) is violated" \
    'drop_guard("ResolveEmptyResumeRetirementRetireAttached", "residency_owner")' "$@"
  # A Composition placement that records no owner.
  mutate placement_records_no_owner \
    "Error: Action property AuditPlacementOwnership is violated" \
    'keep_var("PrepareBindingsIdle", "residency_owner")' "$@"
fi
echo "empty-resume residency audit passed at model_step_count <= ${max_steps}"
