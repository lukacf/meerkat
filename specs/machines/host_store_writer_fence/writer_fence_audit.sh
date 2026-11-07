#!/usr/bin/env bash
# Bounded TLC audit of the HostStoreWriterFenceMachine model
# (writer_fence_audit.tla), in two parts over the same generated model.
#
# The fence part drives it with a host store and a second writer: grants in
# token order, one holder's lease on the store's clock, acquires the writer
# gave up on that the store still decides later (the abandoned acquire), a
# second writer that takes the fence over once, renews, commits and stops,
# and a waited-out end that a store answering a rounded-down remaining lease
# makes come early. The scopes part drops the second writer and the
# abandoned acquires and drives the scope classifications instead: the
# quarantine set (a conflicted incarnation reported under its key), the
# poison map, and the open's listing that may fail.
#
# usage: writer_fence_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives each part's TLC config from the generated ci.cfg next to this
# script (every generated constant and invariant, unchanged), then adds the
# audit's step bound and part, its six invariants and its action property
# (the classifications only grow: no key, record or poison removed, no record
# or cause changed), overrides the model's None with the audit's AuditNone
# (the generated absent option cannot be compared with a present token by
# TLC; see writer_fence_audit.tla), and checks them over the whole bounded
# space (safety). It then reruns the space once per reachability witness,
# with that witness's negation as an extra invariant, and requires TLC to
# report exactly that violation, so no property holds vacuously. The fence
# part's witnesses:
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
# The scopes part's witnesses:
#   - a scope quarantined before a stop is still quarantined once the writer
#     re-acquired and resumed (QuarantineKeptAcrossReacquire);
#   - the same for a poisoned scope (PoisonKeptAcrossReacquire);
#   - a re-acquire refused after the open's listing failed
#     (FailedOpenRefusesReacquire).
# With --mutants it seeds one defect at a time into a copy of the generated
# model and requires TLC to refuse each with the property it breaks. The
# fence part's:
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
# The scopes part's:
#   - resume_clears_poison: a resume clears the poison map
#     (AuditClassificationsMonotone);
#   - repeated_poison_overwrites: a repeated poison replaces the first cause
#     (AuditClassificationsMonotone);
#   - quarantine_ignores_poison: a quarantine ignores the poison map
#     (no_scope_is_quarantined_and_poisoned);
#   - poison_ignores_quarantine: its mirror (the same invariant);
#   - reacquire_after_failed_open: a re-acquire ignores the failed open
#     listing (AuditNoReacquireAfterFailedOpen).
# Witness and mutant runs use one worker, so TLC stops at the shallowest
# violation and prints its trace. An anchor this script expects but cannot
# find fails the run. Each TLC run is held to the canonical lane's per-run
# cap (tlc_run_cap.sh) and fails closed as TLC INCOMPLETE.
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

audit_invariants="  AuditNoFalseLoss
  AuditNoResumeWithoutProof
  AuditNoResumeOverForeignBatch
  AuditWaitsAgainOnlyUnrenewed
  AuditTokenNeverAboveTheStore
  AuditNoReacquireAfterFailedOpen"

# usage: write_cfg <file> <part> <extra invariant lines>
write_cfg() {
  local cfg="$1" part="$2" extra="$3"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}
  AuditPart = \"${part}\"
  None <- AuditNone" "${cfg}"
  replace_exact_line "INVARIANTS" "PROPERTIES
  AuditClassificationsMonotone
INVARIANTS
${audit_invariants}${extra}" "${cfg}"
  replace_exact_line "  CiStateConstraint" "  AuditStateConstraint" "${cfg}"
  printf 'CHECK_DEADLOCK FALSE\n' >> "${cfg}"
}

if [[ " ${JAVA_TOOL_OPTIONS:-} " != *" -Xss"* ]]; then
  export JAVA_TOOL_OPTIONS="-Xss512m -XX:+UseParallelGC${JAVA_TOOL_OPTIONS:+ ${JAVA_TOOL_OPTIONS}}"
fi
if [[ " ${JDK_JAVA_OPTIONS:-} " != *" -Xss"* ]]; then
  export JDK_JAVA_OPTIONS="-Xss512m${JDK_JAVA_OPTIONS:+ ${JDK_JAVA_OPTIONS}}"
fi
safety_workers="${TLC_WORKERS:-auto}"
workers="${safety_workers}"
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

# usage: check_part <part> <witness...>
check_part() {
  local part="$1"
  shift
  local safety_cfg="${work_dir}/${part}-safety.cfg"
  write_cfg "${safety_cfg}" "${part}" ""
  local safety_status=0
  workers="${safety_workers}"
  run_tlc "${part}-safety" "${safety_cfg}" "${tlc_args[@]}" || safety_status=$?
  cat "${work_dir}/${part}-safety.log"
  if [[ "${safety_status}" != "0" ]] \
    || ! grep -q "Model checking completed. No error has been found." "${work_dir}/${part}-safety.log"; then
    echo "error: writer fence audit (${part}) failed (tlc exit ${safety_status}, bound ${max_steps})" >&2
    exit 1
  fi
  # Witness runs use one worker: TLC then stops at the shallowest violation
  # and prints its trace (with several workers it may report a witness's
  # violation without one).
  workers=1
  local witness cfg status log depth
  for witness in "$@"; do
    cfg="${work_dir}/${part}-witness-${witness}.cfg"
    write_cfg "${cfg}" "${part}" "
  NotAuditWitness${witness}"
    status=0
    run_tlc "${part}-witness-${witness}" "${cfg}" "${tlc_args[@]}" || status=$?
    log="${work_dir}/${part}-witness-${witness}.log"
    if ! grep -q "Error: Invariant NotAuditWitness${witness} is violated" "${log}"; then
      cat "${log}"
      echo "error: witness ${witness} (${part}) is not reachable at bound ${max_steps} (tlc exit ${status})" >&2
      exit 1
    fi
    depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
    echo "witness ${witness} (${part}) reached in ${depth} states"
  done
}

tlc_args=("$@")
check_part fence ResumedAfterStop WaitedOutOwnGrantResumed HolderRenewed ForeignAfterWait \
  ListingFailedResumed Residual WaitedOutAgainResumed
check_part scopes QuarantineKeptAcrossReacquire PoisonKeptAcrossReacquire \
  FailedOpenRefusesReacquire

if [[ "${run_mutants}" == "true" ]]; then
  # Shared by every mutant edit: edit_arms rewrites, in each named arm of the
  # generated model, the lines `select` matches, and requires exactly
  # `per_arm` of them in every arm; replace_arm replaces a whole arm; and
  # rewrite_frame replaces an arm's unchanged frame with `assignments` and an
  # explicit UNCHANGED of every other variable the arm does not prime.
  mutate_prelude='
import os, re
path = os.environ["MODEL"]
text = open(path).read()
VARS = [name.strip() for name in re.search(r"^VARIABLES (.*)$", text, re.M).group(1).split(",")]

def arm_block(arm):
    found = re.compile(r"^" + arm + r"(\(| ==).*?\n\n", re.S | re.M).search(text)
    assert found, f"{arm} not found"
    return found

def edit_arms(arms, select, rewrite, per_arm=1):
    global text
    for arm in arms:
        found = arm_block(arm)
        lines = found.group(0).split("\n")
        hits = [index for index, line in enumerate(lines) if select(line)]
        assert len(hits) == per_arm, f"expected {per_arm} matching line(s) in {arm}, found {len(hits)}"
        for index in hits:
            lines[index] = rewrite(lines[index])
        text = text[: found.start()] + "\n".join(lines) + text[found.end():]

def replace_arm(arm, body):
    global text
    found = arm_block(arm)
    text = text[: found.start()] + body + "\n\n" + text[found.end():]

def primed_name(line):
    stripped = line.strip()
    if stripped.startswith("/\\ ") and "'"'"' = " in stripped:
        return stripped[3:].split("'"'"'", 1)[0]
    return None

def rewrite_frame(arm, assignments):
    global text
    found = arm_block(arm)
    lines = found.group(0).split("\n")
    frames = [index for index, line in enumerate(lines) if line.strip().startswith("/\\ UnchangedFrame_")]
    assert len(frames) == 1, f"expected one unchanged frame in {arm}, found {len(frames)}"
    primed = {primed_name(line) for line in lines + assignments} - {None}
    kept = [name for name in VARS if name not in primed]
    lines[frames[0]: frames[0] + 1] = assignments + ["    /\\ UNCHANGED << " + ", ".join(kept) + " >>"]
    text = text[: found.start()] + "\n".join(lines) + text[found.end():]

def guard_on(name):
    return lambda line: (line.startswith("    /\\ ") and re.search(r"\b" + name + r"\b", line) is not None
                         and "'"'"'" not in line)

OPENED = ["Live", "Stopped", "Acquiring", "Proving", "Resuming", "WaitingOut"]
'

  # usage: mutate <part> <name> <expected violation regex>; reads a python
  # edit on stdin (run after the prelude above) that rewrites model.tla in
  # place, failing on any anchor it cannot find.
  mutate() {
    local part="$1" name="$2" expected="$3"
    shift 3
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}"/*.tla "${dir}/"
    { printf '%s\n' "${mutate_prelude}"; cat; printf '%s\n' 'open(path, "w").write(text)'; } \
      | MODEL="${dir}/model.tla" python3 -
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_cfg "${cfg}" "${part}" ""
    local status=0
    # One worker: TLC explores breadth first in order, so it reports the
    # shallowest violation, the one each mutant names.
    local workers=1
    (cd "${dir}" && run_tlc "mutant-${name}" "${cfg}" "${tlc_args[@]}") || status=$?
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -qE "${expected}" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} (${part}) was not refused by '${expected}' (tlc exit ${status})" >&2
      exit 1
    fi
    echo "mutant ${name} (${part}) refused: ${expected}"
  }

  mutate fence unrenewed_holder_lost "Invariant AuditNoFalseLoss is violated" <<'PY'
edit_arms(["AcquireHeldByRenewedHolder"], guard_on("lease_moved"), lambda line: "    /\\ TRUE")
PY

  mutate fence renewed_holder_waited_again "Invariant AuditWaitsAgainOnlyUnrenewed is violated" <<'PY'
edit_arms(["AcquireHeldByUnrenewedHolder"], guard_on("lease_moved"), lambda line: "    /\\ TRUE")
PY

  mutate fence late_grant_lost "Invariant AuditNoFalseLoss is violated" <<'PY'
edit_arms(["AcquireHeldByAnother"], guard_on("timed_out"), lambda line: "    /\\ TRUE")
PY

  mutate fence failed_listing_resumes \
    "Invariant (AuditNoResumeWithoutProof|proving_and_resuming_carry_their_grant) is violated" <<'PY'
edit_arms(
    ["ListingFailed"],
    lambda line: line.startswith("    /\\ phase' = "),
    lambda line: "    /\\ phase' = \"Resuming\"",
)
PY

  mutate fence foreign_listing_installs "Invariant AuditNoResumeOverForeignBatch is violated" <<'PY'
replace_arm(
    "ListingForeign",
    "ListingForeign(outcome, arg_token) ==\n"
    "    /\\ (outcome = \"ForeignBatch\")\n"
    "    /\\ ListingClean(\"Clean\", arg_token)",
)
PY

  mutate scopes resume_clears_poison "Action property AuditClassificationsMonotone is violated" <<'PY'
rewrite_frame("ResumeRenewed", ["    /\\ poisoned_scopes' = [scope \\in {} |-> \"PlanFailed\"]"])
PY

  mutate scopes repeated_poison_overwrites "Action property AuditClassificationsMonotone is violated" <<'PY'
for phase in OPENED:
    rewrite_frame(
        "ScopePoisonedAgain" + phase,
        ["    /\\ poisoned_scopes' = MapSet(poisoned_scopes, scope, cause)"],
    )
PY

  mutate scopes quarantine_ignores_poison \
    "Invariant no_scope_is_quarantined_and_poisoned is violated" <<'PY'
edit_arms(
    ["ScopeQuarantined" + phase for phase in OPENED],
    guard_on("poisoned_scopes"),
    lambda line: "    /\\ TRUE",
)
PY

  mutate scopes poison_ignores_quarantine \
    "Invariant no_scope_is_quarantined_and_poisoned is violated" <<'PY'
edit_arms(
    ["ScopePoisoned" + phase for phase in OPENED],
    guard_on("quarantined_keys"),
    lambda line: "    /\\ TRUE",
)
PY

  mutate scopes reacquire_after_failed_open "Invariant AuditNoReacquireAfterFailedOpen is violated" <<'PY'
edit_arms(["BeginReacquire"], guard_on("open_listing_failed"), lambda line: "    /\\ TRUE")
PY
fi
echo "writer fence audit passed at model_step_count <= ${max_steps}"
