#!/usr/bin/env bash
# Bounded TLC audit of the HostStoreWriterFenceMachine model, driven by a
# host store and a second writer (writer_fence_audit.tla): grants in token
# order, one holder's lease on the store's clock, acquires the writer gave up
# on that the store still decides later (the abandoned acquire), a second
# writer that takes the fence over once, renews, commits and stops, and a
# waited-out end that a store answering a rounded-down remaining lease makes
# come early.
#
# usage: writer_fence_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's step
# bound and its five properties, overrides the model's None with the audit's
# AuditNone (the generated absent option cannot be compared with a present
# token by TLC; see writer_fence_audit.tla), and checks them over the whole
# bounded space (safety). It then reruns the space once per reachability witness, with that
# witness's negation as an extra invariant, and requires TLC to report
# exactly that violation, so no property holds vacuously:
#   - a resume after a stop (ResumedAfterStop);
#   - the writer's own late grant waited out, then a resume
#     (WaitedOutOwnGrantResumed);
#   - a waited-out holder that renewed loses the realm (HolderRenewed);
#   - a foreign batch found after a wait (ForeignAfterWait);
#   - a listing that failed after a grant, then a resume on a takeover of
#     that grant (ListingFailedResumed);
#   - the documented residual: a takeover the writer gave up on, granted
#     after a later grant of its own, supersedes its live token (Residual);
#   - a holder nobody renewed, met again after an end recorded early, waited
#     out again and the writer resumed (WaitedOutAgainResumed).
# With --mutants it seeds five defects into a copy of the generated model and
# requires TLC to refuse each with the property it breaks:
#   - unrenewed_holder_lost: the renewed-holder loss ignores whether the
#     holder's lease moved (AuditNoFalseLoss);
#   - renewed_holder_waited_again: the wait-again arm ignores whether the
#     holder's lease moved (AuditWaitsAgainOnlyUnrenewed);
#   - late_grant_lost: an unknown holder loses the realm even after an
#     acquire of the writer's own timed out (AuditNoFalseLoss);
#   - failed_listing_resumes: a failed listing resumes the writer
#     (AuditNoResumeWithoutProof, or the generated grant invariant first);
#   - foreign_listing_installs: a listing with a foreign batch installs the
#     grant (AuditNoResumeOverForeignBatch).
# A mutant run uses one worker, so the violation TLC reports is the
# shallowest one. An anchor this script expects but cannot find fails the
# run. Each TLC run is held to the canonical lane's per-run cap
# (tlc_run_cap.sh) and fails closed as TLC INCOMPLETE.
set -euo pipefail

usage="usage: writer_fence_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 15 )); then
  echo "error: max-steps must be an integer >= 15 (the longest witness takes 15 machine steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/writer-fence-audit.XXXXXX")"
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

audit_properties="  AuditNoFalseLoss
  AuditNoResumeWithoutProof
  AuditNoResumeOverForeignBatch
  AuditWaitsAgainOnlyUnrenewed
  AuditTokenNeverAboveTheStore"

# usage: write_cfg <file> <extra invariant lines>
write_cfg() {
  local cfg="$1" extra="$2"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}
  None <- AuditNone" "${cfg}"
  replace_exact_line "INVARIANTS" "INVARIANTS
${audit_properties}${extra}" "${cfg}"
  replace_exact_line "  CiStateConstraint" "  AuditStateConstraint" "${cfg}"
  printf 'CHECK_DEADLOCK FALSE\n' >> "${cfg}"
}

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
  tlc_run_capped writer_fence_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" writer_fence_audit.tla || status=$?
  return "${status}"
}

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" ""
safety_status=0
run_tlc safety "${safety_cfg}" "$@" || safety_status=$?
cat "${work_dir}/safety.log"
if [[ "${safety_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: writer fence audit failed (tlc exit ${safety_status}, bound ${max_steps})" >&2
  exit 1
fi

# Witness and mutant runs use one worker: TLC then stops at the shallowest
# violation and prints its trace (with several workers it may report a
# witness's violation without one).
workers=1
for witness in ResumedAfterStop WaitedOutOwnGrantResumed HolderRenewed ForeignAfterWait \
  ListingFailedResumed Residual WaitedOutAgainResumed; do
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
  # Shared by every mutant edit: edit_arms rewrites, in each named arm of the
  # generated model, the lines `select` matches, and requires exactly
  # `per_arm` of them in every arm.
  mutate_prelude='
import os, re
path = os.environ["MODEL"]
text = open(path).read()

def edit_arms(arms, select, rewrite, per_arm=1):
    global text
    for arm in arms:
        found = re.compile(r"^" + arm + r"\(.*?\n\n", re.S | re.M).search(text)
        assert found, f"{arm} not found"
        lines = found.group(0).split("\n")
        hits = [index for index, line in enumerate(lines) if select(line)]
        assert len(hits) == per_arm, f"expected {per_arm} matching line(s) in {arm}, found {len(hits)}"
        for index in hits:
            lines[index] = rewrite(lines[index])
        text = text[: found.start()] + "\n".join(lines) + text[found.end():]

def guard_on(name):
    return lambda line: (line.startswith("    /\\ ") and re.search(r"\b" + name + r"\b", line) is not None
                         and "'"'"'" not in line)

def replace_arm(arm, body):
    global text
    found = re.compile(r"^" + arm + r"\(.*?\n\n", re.S | re.M).search(text)
    assert found, f"{arm} not found"
    text = text[: found.start()] + body + "\n\n" + text[found.end():]
'

  # usage: mutate <name> <expected violation regex>; reads a python edit on
  # stdin (run after the prelude above) that rewrites model.tla in place
  # through edit_arms, which fails on any anchor it cannot find.
  mutate() {
    local name="$1" expected="$2"
    shift 2
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}"/*.tla "${dir}/"
    { printf '%s\n' "${mutate_prelude}"; cat; printf '%s\n' 'open(path, "w").write(text)'; } \
      | MODEL="${dir}/model.tla" python3 -
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${cfg}" ""
    local status=0
    # One worker: TLC explores breadth first in order, so it reports the
    # shallowest violation, the one each mutant names.
    local workers=1
    (cd "${dir}" && run_tlc "mutant-${name}" "${cfg}" "$@") || status=$?
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -qE "${expected}" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by '${expected}' (tlc exit ${status})" >&2
      exit 1
    fi
    echo "mutant ${name} refused: ${expected}"
  }

  mutate unrenewed_holder_lost "Invariant AuditNoFalseLoss is violated" "$@" <<'PY'
edit_arms(["AcquireHeldByRenewedHolder"], guard_on("lease_moved"), lambda line: "    /\\ TRUE")
PY

  mutate renewed_holder_waited_again "Invariant AuditWaitsAgainOnlyUnrenewed is violated" "$@" <<'PY'
edit_arms(["AcquireHeldByUnrenewedHolder"], guard_on("lease_moved"), lambda line: "    /\\ TRUE")
PY

  mutate late_grant_lost "Invariant AuditNoFalseLoss is violated" "$@" <<'PY'
edit_arms(["AcquireHeldByAnother"], guard_on("timed_out"), lambda line: "    /\\ TRUE")
PY

  mutate failed_listing_resumes \
    "Invariant (AuditNoResumeWithoutProof|proving_and_resuming_carry_their_grant) is violated" "$@" <<'PY'
edit_arms(
    ["ListingFailed"],
    lambda line: line.startswith("    /\\ phase' = "),
    lambda line: "    /\\ phase' = \"Resuming\"",
)
PY

  mutate foreign_listing_installs "Invariant AuditNoResumeOverForeignBatch is violated" "$@" <<'PY'
replace_arm(
    "ListingForeign",
    "ListingForeign(outcome, arg_token) ==\n"
    "    /\\ (outcome = \"ForeignBatch\")\n"
    "    /\\ ListingClean(\"Clean\", arg_token)",
)
PY
fi
echo "writer fence audit passed at model_step_count <= ${max_steps}"
