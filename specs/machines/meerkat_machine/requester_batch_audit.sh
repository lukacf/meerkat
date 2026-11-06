#!/usr/bin/env bash
# Bounded TLC audit of the requester batch rule: inputs of different
# requesters never share a run.
#
# usage: requester_batch_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then gives the audit two
# inputs, two runs, one session and the keys, adds its invariants and its step
# bound. An anchor this script expects but cannot find fails the run. It
# checks:
#   - safety, once per pair of different keys of the batch input and the late
#     input ((key_a, key_b), (key_a, none), (none, key_a)): the inputs bound
#     to a run share one key (AuditRunsShareOneKey) and the late input is
#     never associated with the batch input's run (AuditLateNeverInBatchRun);
#   - witnesses: with the batch input's key the late input joins the running
#     run (TLC must report AuditLateNeverJoins violated), and with another
#     key it is staged into a run of its own (AuditLateNeverRunsAlone);
#   - with --mutants, on a copy of the generated model: the durable join
#     without its guard live_join_same_requester, and admission that records a
#     batch key only for a governed binding (as before requester keys), must
#     each make the (key_a, key_b) safety run fail.
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
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 12 )); then
  echo "error: max-steps must be an integer >= 12 (the deterministic prefix is 8 steps and the own-run witness is reached at 12)" >&2
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

# usage: write_cfg <file> <batch key name> <late key name> <extra invariant lines>
write_cfg() {
  local cfg="$1" batch_key="$2" late_key="$3" extra="$4"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "  InputIdValues = {}" '  InputIdValues = {"input_batch", "input_late"}' "${cfg}"
  replace_exact_line "  RunIdValues = {}" '  RunIdValues = {"runid_1", "runid_2"}' "${cfg}"
  replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1"}' "${cfg}"
  replace_exact_line "  StringValues = {}" '  StringValues = {"input_batch", "input_late", "key_a", "key_b", "alpha"}' "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}
  AuditBatchKeyName = \"${batch_key}\"
  AuditLateKeyName = \"${late_key}\"" "${cfg}"
  replace_exact_line "INVARIANTS" "INVARIANTS
  AuditRunsShareOneKey${extra}" "${cfg}"
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

for pair in "key_a key_b" "key_a none" "none key_a"; do
  read -r batch_key late_key <<<"${pair}"
  name="safety-${batch_key}-${late_key}"
  cfg="${work_dir}/${name}.cfg"
  write_cfg "${cfg}" "${batch_key}" "${late_key}" "
  AuditLateNeverInBatchRun"
  status=0
  run_tlc "${name}" "${cfg}" "$@" || status=$?
  if [[ "${status}" != "0" ]] \
    || ! grep -q "Model checking completed. No error has been found." "${work_dir}/${name}.log"; then
    cat "${work_dir}/${name}.log" >&2
    echo "error: requester batch audit ${name} failed (tlc exit ${status}, bound ${max_steps})" >&2
    exit 1
  fi
done

# usage: witness <name> <batch key> <late key> <goal invariant>
witness() {
  local name="$1" batch_key="$2" late_key="$3" goal="$4"
  shift 4
  local cfg="${work_dir}/${name}.cfg"
  write_cfg "${cfg}" "${batch_key}" "${late_key}" "
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

witness witness-same-requester-joins key_a key_a AuditLateNeverJoins "$@"
witness witness-other-requester-runs-alone key_a key_b AuditLateNeverRunsAlone "$@"

if [[ "${run_mutants}" == "true" ]]; then
  # usage: mutate <name> <awk program> [extra tlc args...]; the awk program
  # rewrites a copy of model.tla and exits non-zero when an anchor it
  # expects is missing. The (key_a, key_b) safety run must then fail.
  mutate() {
    local name="$1" program="$2"
    shift 2
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
    write_cfg "${cfg}" key_a key_b "
  AuditLateNeverInBatchRun"
    local status=0
    (cd "${dir}" && run_tlc "mutant-${name}" "${cfg}" "$@") || status=$?
    if ! grep -qE "Error: Invariant (AuditLateNeverInBatchRun|AuditRunsShareOneKey) is violated" \
      "${work_dir}/mutant-${name}.log"; then
      cat "${work_dir}/mutant-${name}.log" >&2
      echo "error: mutant ${name} was not refused by the requester batch invariants (tlc exit ${status})" >&2
      exit 1
    fi
    echo "mutant ${name} refused"
  }

  # The durable join without live_join_same_requester: its one guard line
  # that reads the staged batch becomes TRUE.
  mutate join-guard-removed '
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
  mutate admission-key-dropped '
    /^ResolveAdmissionPlan[A-Za-z]*\(/ { in_admission = 1; arms++ }
    in_admission && /^$/ { in_admission = 0 }
    in_admission && index($0, "    /\\ input_authority_batch_keys'"'"' = IF (authority_batch_key # None) THEN ") == 1 {
      sub(/IF \(authority_batch_key # None\) THEN/, "IF (authority_binding # None) THEN")
      edits++
    }
    { print }
    END { exit (arms >= 8 && edits == arms) ? 0 : 1 }
  ' "$@"
fi

echo "requester batch audit passed at model_step_count <= ${max_steps} (three key pairs safe; the same requester joins, another runs alone$([[ "${run_mutants}" == "true" ]] && echo '; both mutants refused'))"
