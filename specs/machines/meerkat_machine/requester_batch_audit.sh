#!/usr/bin/env bash
# Bounded TLC audit of the requester batch rule (inputs of different
# requesters never share a run) and of the Steer lane's order across
# requesters (first in, first out).
#
# usage: requester_batch_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then gives the audit three
# inputs, two runs, one session and the keys, adds its invariants, its
# property and its step bound. An anchor this script expects but cannot find
# fails the run. It checks:
#   - requester safety, once per pair of different keys of the batch input
#     and the late input ((key_a, key_b), (key_a, none), (none, key_a)): the
#     inputs bound to a run share one key (AuditRunsShareOneKey) and the late
#     input is never associated with the batch input's run
#     (AuditLateNeverInBatchRun), through admission, joins, receipts,
#     staging, run endings and a fence gap's suspension and resume;
#   - order safety, once per earlier input of another key ahead of a late
#     input of the batch input's key ((key_a, key_b, key_a), (key_a, none,
#     key_a), (none, key_a, none)): no step binds an input waiting in the
#     Steer lane to a run while a Steer input admitted before it waits in
#     the Steer lane with another key (the action property
#     AuditSteerLaneFifo), and AuditRunsShareOneKey;
#   - witnesses: with the batch input's key the late input joins the running
#     run (TLC must report AuditLateNeverJoins violated), with another key it
#     is staged into a run of its own (AuditLateNeverRunsAlone); with an
#     earlier input of another key waiting, the earlier input runs in the
#     follow-up run (AuditEarlyNeverRunsAlone), and the late input joins the
#     running run once the earlier one left the Steer lane
#     (AuditLateNeverJoins);
#   - with --mutants, on a copy of the generated model: the durable join
#     without its guard live_join_same_requester, and admission that records
#     a batch key only for a governed binding (as before requester keys),
#     must each make the (key_a, key_b) requester run fail; ResumeRun staging
#     no batch key must make the (key_a, none) requester run fail; the
#     durable join, and StageForRun, without the order guard
#     steer_lane_fifo_across_requesters must each make the (key_a, key_b,
#     key_a) order run fail.
# Each TLC run is held to the canonical lane's per-run cap
# (`TLC_RUN_CAP_SECS`, default 900 s, see tlc_run_cap.sh); a run that hits it
# fails closed as TLC INCOMPLETE (exit 3).
set -euo pipefail

usage="usage: requester_batch_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 14 )); then
  echo "error: max-steps must be an integer >= 14 (the deterministic prefix is up to 10 steps and the resumed-run mutant is refused at 14)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/requester-batch-audit.XXXXXX")"
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

# usage: write_cfg <file> <batch key name> <late key name> <early key name>
#   <extra invariant lines> [<property lines>]
write_cfg() {
  local cfg="$1" batch_key="$2" late_key="$3" early_key="$4" extra="$5" properties="${6:-}"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "  InputIdValues = {}" '  InputIdValues = {"input_batch", "input_early", "input_late"}' "${cfg}"
  replace_exact_line "  RunIdValues = {}" '  RunIdValues = {"runid_1", "runid_2"}' "${cfg}"
  replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1"}' "${cfg}"
  replace_exact_line "  StringValues = {}" '  StringValues = {"input_batch", "input_early", "input_late", "key_a", "key_b", "alpha"}' "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}
  AuditBatchKeyName = \"${batch_key}\"
  AuditLateKeyName = \"${late_key}\"
  AuditEarlyKeyName = \"${early_key}\"" "${cfg}"
  if [[ -n "${properties}" ]]; then
    replace_exact_line "INVARIANTS" "PROPERTIES${properties}
INVARIANTS
  AuditRunsShareOneKey${extra}" "${cfg}"
  else
    replace_exact_line "INVARIANTS" "INVARIANTS
  AuditRunsShareOneKey${extra}" "${cfg}"
  fi
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
  tlc_run_capped requester_batch_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" requester_batch_audit.tla || status=$?
  grep -E 'states generated|distinct states|is violated|Error:' "${work_dir}/${name}.log" \
    | sed "s/^/[${name}] /" || true
  return "${status}"
}

# usage: safety <name> <batch key> <late key> <early key> <extra invariants>
#   [<properties>] [extra tlc args...]
safety() {
  local name="$1" batch_key="$2" late_key="$3" early_key="$4" extra="$5" properties="$6"
  shift 6
  local cfg="${work_dir}/${name}.cfg"
  write_cfg "${cfg}" "${batch_key}" "${late_key}" "${early_key}" "${extra}" "${properties}"
  local status=0
  run_tlc "${name}" "${cfg}" "$@" || status=$?
  if [[ "${status}" != "0" ]] \
    || ! grep -q "Model checking completed. No error has been found." "${work_dir}/${name}.log"; then
    cat "${work_dir}/${name}.log" >&2
    echo "error: requester batch audit ${name} failed (tlc exit ${status}, bound ${max_steps})" >&2
    exit 1
  fi
}

for pair in "key_a key_b" "key_a none" "none key_a"; do
  read -r batch_key late_key <<<"${pair}"
  safety "safety-${batch_key}-${late_key}" "${batch_key}" "${late_key}" absent "
  AuditLateNeverInBatchRun" "" "$@"
done

for triple in "key_a key_b key_a" "key_a none key_a" "none key_a none"; do
  read -r batch_key early_key late_key <<<"${triple}"
  safety "order-${batch_key}-${early_key}-${late_key}" "${batch_key}" "${late_key}" \
    "${early_key}" "" "
  AuditSteerLaneFifo" "$@"
done

# usage: witness <name> <batch key> <late key> <early key> <goal invariant>
witness() {
  local name="$1" batch_key="$2" late_key="$3" early_key="$4" goal="$5"
  shift 5
  local cfg="${work_dir}/${name}.cfg"
  write_cfg "${cfg}" "${batch_key}" "${late_key}" "${early_key}" "
  ${goal}"
  local status=0
  run_tlc "${name}" "${cfg}" "$@" || status=$?
  if ! grep -q "Error: Invariant ${goal} is violated" "${work_dir}/${name}.log"; then
    cat "${work_dir}/${name}.log" >&2
    echo "error: requester batch audit witness ${name} is not reachable at bound ${max_steps} (tlc exit ${status})" >&2
    exit 1
  fi
  local depth
  depth="$(grep -cE '^State [0-9]+:' "${work_dir}/${name}.log" || true)"
  echo "witness ${name} reached in ${depth} states"
}

witness witness-same-requester-joins key_a key_a absent AuditLateNeverJoins "$@"
witness witness-other-requester-runs-alone key_a key_b absent AuditLateNeverRunsAlone "$@"
witness witness-earlier-steer-runs-alone key_a key_a key_b AuditEarlyNeverRunsAlone "$@"
witness witness-later-steer-joins-once-the-earlier-left key_a key_a key_b AuditLateNeverJoins "$@"

if [[ "${run_mutants}" == "true" ]]; then
  # usage: mutate <name> <batch key> <late key> <early key> <expected
  #   violation regex> <awk program> [extra tlc args...]; the awk program
  #   rewrites a copy of model.tla and exits non-zero when an anchor it
  #   expects is missing. The run of the given keys (its safety invariants
  #   and, for an order run, its property) must then fail with the expected
  #   violation.
  mutate() {
    local name="$1" batch_key="$2" late_key="$3" early_key="$4" expected="$5" program="$6"
    shift 6
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}"/*.tla "${dir}/"
    if ! awk "${program}" "${spec_dir}/model.tla" > "${dir}/model.tla"; then
      echo "error: mutant ${name}: an anchor of the generated model is missing" >&2
      exit 1
    fi
    if cmp -s "${spec_dir}/model.tla" "${dir}/model.tla"; then
      echo "error: mutant ${name} changed nothing" >&2
      exit 1
    fi
    local cfg="${work_dir}/mutant-${name}.cfg"
    if [[ "${early_key}" == "absent" ]]; then
      write_cfg "${cfg}" "${batch_key}" "${late_key}" absent "
  AuditLateNeverInBatchRun"
    else
      write_cfg "${cfg}" "${batch_key}" "${late_key}" "${early_key}" "" "
  AuditSteerLaneFifo"
    fi
    local status=0
    (cd "${dir}" && run_tlc "mutant-${name}" "${cfg}" "$@") || status=$?
    if ! grep -qE "Error: (Invariant|Action property) (${expected}) is violated" \
      "${work_dir}/mutant-${name}.log"; then
      cat "${work_dir}/mutant-${name}.log" >&2
      echo "error: mutant ${name} was not refused by ${expected} (tlc exit ${status})" >&2
      exit 1
    fi
    echo "mutant ${name} refused"
  }

  # The durable join without live_join_same_requester: its one guard line
  # that reads the staged batch becomes TRUE.
  mutate join-guard-removed key_a key_b absent \
    'AuditLateNeverInBatchRun|AuditRunsShareOneKey' '
    /^JoinLiveBoundaryDurableAppendRunning\(/ { in_join = 1 }
    in_join && /^$/ { in_join = 0 }
    in_join && /^    \/\\ / && index($0, "authority_staged_batch") > 0 && index($0, "'"'"'") == 0 {
      hits++
      print "    /\\ TRUE"
      next
    }
    { print }
    END { exit hits == 1 ? 0 : 1 }
  ' "$@"

  # Admission records a batch key only for a governed binding, as before
  # requester keys: every admission arm's key update keys on the binding.
  mutate admission-key-dropped key_a key_b absent \
    'AuditLateNeverInBatchRun|AuditRunsShareOneKey' '
    /^ResolveAdmissionPlan[A-Za-z]*\(/ { in_admission = 1; arms++ }
    in_admission && /^$/ { in_admission = 0 }
    in_admission && index($0, "    /\\ input_authority_batch_keys'"'"' = IF (authority_batch_key # None) THEN ") == 1 {
      sub(/IF \(authority_batch_key # None\) THEN/, "IF (authority_binding # None) THEN")
      edits++
    }
    { print }
    END { exit (arms >= 8 && edits == arms) ? 0 : 1 }
  ' "$@"

  # ResumeRun stages no batch key: the resumed run takes any key, and a
  # requester-less steer is staged into a person's resumed run.
  mutate resume-stages-no-key key_a none absent \
    'AuditLateNeverInBatchRun|AuditRunsShareOneKey' '
    /^ResumeRunRunning\(/ { in_resume = 1 }
    in_resume && /^$/ { in_resume = 0 }
    in_resume && index($0, "    /\\ authority_staged_batch'"'"' = ") == 1 {
      hits++
      print "    /\\ authority_staged_batch'"'"' = None"
      next
    }
    { print }
    END { exit hits == 1 ? 0 : 1 }
  ' "$@"

  # The order guard removed: its one guard line (the only one that names the
  # waiting input) becomes TRUE, in the durable join, then in StageForRun.
  for action in JoinLiveBoundaryDurableAppendRunning StageForRunRunning; do
    mutate "fifo-guard-removed-${action}" key_a key_a key_b 'AuditSteerLaneFifo' '
      /^'"${action}"'\(/ { in_action = 1 }
      in_action && /^$/ { in_action = 0 }
      in_action && /^    \/\\ / && index($0, "waiting_input_id") > 0 {
        hits++
        print "    /\\ TRUE"
        next
      }
      { print }
      END { exit hits == 1 ? 0 : 1 }
    ' "$@"
  done
fi

echo "requester batch audit passed at model_step_count <= ${max_steps} (three key pairs safe, three order runs safe; the same requester joins, another runs alone, an earlier steer of another key runs first, a later one joins once it left$([[ "${run_mutants}" == "true" ]] && echo '; all five mutants refused'))"
