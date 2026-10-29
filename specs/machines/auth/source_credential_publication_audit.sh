#!/usr/bin/env bash
# Bounded TLC audit of the AuthMachine's source-credential publication.
#
# usage: source_credential_publication_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds a second credential
# value, the audit's action property and invariant, and its step bound, and
# checks them over the whole bounded space. It then reruns the same space
# once per reachability witness, with that witness's negation as an extra
# invariant, and requires TLC to report exactly that violation, so the
# properties are not vacuous. With --mutants it then seeds two defects, each
# into its own copy of the generated model, and requires TLC to refuse each:
# Acquire without its guard credential_material_publishable (refused by the
# action property), and the renewal arms AcquireFromExpiring and
# AcquireFromExpired disabled (refused by the verdict invariant). An anchor
# this script expects but cannot find fails the run.
set -euo pipefail

usage="usage: source_credential_publication_audit.sh <max-steps> [--mutants] [extra tlc args...]"
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
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/source-credential-publication-audit.XXXXXX")"
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

# usage: write_cfg <file> <invariant lines>
write_cfg() {
  local cfg="$1" invariants="$2"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line '  StringValues = {"alpha"}' '  StringValues = {"alpha", "beta"}' "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}" "${cfg}"
  replace_exact_line "INVARIANTS" "PROPERTIES
  AuditWithdrawnMaterialNeverRepublished
INVARIANTS${invariants}" "${cfg}"
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
    source_credential_publication_audit.tla > "${work_dir}/${name}.log" 2>&1
  local status=$?
  set -e
  return "${status}"
}

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" "
  AuditVerdictMatchesAcquire"
safety_status=0
run_tlc safety "${safety_cfg}" "$@" || safety_status=$?
cat "${work_dir}/safety.log"
if [[ "${safety_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: source-credential publication audit failed (tlc exit ${safety_status}, bound ${max_steps})" >&2
  exit 1
fi

for witness in RefusalHealed LogoutHealed WindowRenewed RefusedValueBlocked \
  LoggedOutValueBlocked; do
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
  # The guard dropped from Acquire: a refused or logged-out value can be
  # published again. The action property alone must refuse it (the run
  # leaves out the verdict invariant, which would stop TLC a step earlier).
  dir="${work_dir}/mutant-unguarded-acquire"
  mkdir -p "${dir}"
  cp "${spec_dir}"/*.tla "${dir}/"
  MODEL="${dir}/model.tla" python3 - <<'PY'
import os
import re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^Acquire\(.*?\n\n", text, re.S | re.M)
assert block, "Acquire not found"
lines = block.group(0).split("\n")
hits = [
    i
    for i, line in enumerate(lines)
    if line.startswith("    /\\ ") and "arg_credential_material_fingerprint = None" in line
]
assert len(hits) == 1, f"expected one credential_material_publishable conjunct in Acquire, found {len(hits)}"
del lines[hits[0]]
text = text[: block.start()] + "\n".join(lines) + text[block.end():]
open(path, "w").write(text)
PY
  cfg="${work_dir}/mutant-unguarded-acquire.cfg"
  write_cfg "${cfg}" ""
  status=0
  (cd "${dir}" && run_tlc mutant-unguarded-acquire "${cfg}" "$@") || status=$?
  log="${work_dir}/mutant-unguarded-acquire.log"
  if ! grep -q "Error: Action property AuditWithdrawnMaterialNeverRepublished is violated" "${log}"; then
    cat "${log}"
    echo "error: mutant unguarded-acquire was not refused by AuditWithdrawnMaterialNeverRepublished (tlc exit ${status})" >&2
    exit 1
  fi
  echo "mutant unguarded-acquire refused by AuditWithdrawnMaterialNeverRepublished"

  # The renewal arms disabled: in Expiring and Expired the classifier still
  # answers Publish for the value the lease records, while no Acquire arm
  # accepts it there. The verdict invariant must refuse it.
  dir="${work_dir}/mutant-no-renewal"
  mkdir -p "${dir}"
  cp "${spec_dir}"/*.tla "${dir}/"
  MODEL="${dir}/model.tla" python3 - <<'PY'
import os
import re
path = os.environ["MODEL"]
text = open(path).read()
for arm in ("AcquireFromExpiring", "AcquireFromExpired"):
    header = re.search(r"^" + arm + r"\([^\n]*\) ==\n", text, re.M)
    assert header, f"{arm} not found"
    text = text[: header.end()] + "    /\\ FALSE\n" + text[header.end():]
open(path, "w").write(text)
PY
  cfg="${work_dir}/mutant-no-renewal.cfg"
  write_cfg "${cfg}" "
  AuditVerdictMatchesAcquire"
  status=0
  (cd "${dir}" && run_tlc mutant-no-renewal "${cfg}" "$@") || status=$?
  log="${work_dir}/mutant-no-renewal.log"
  if ! grep -q "Error: Invariant AuditVerdictMatchesAcquire is violated" "${log}"; then
    cat "${log}"
    echo "error: mutant no-renewal was not refused by AuditVerdictMatchesAcquire (tlc exit ${status})" >&2
    exit 1
  fi
  echo "mutant no-renewal refused by AuditVerdictMatchesAcquire"
fi
echo "source-credential publication audit passed at model_step_count <= ${max_steps}"
