#!/usr/bin/env bash
# Bounded TLC audit of several durable Steer inputs at one model boundary.
#
# usage: durable_multi_steer_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives every TLC config from the generated ci.cfg next to this script
# (every generated constant and invariant, unchanged), then adds the audit's
# finite model values, its invariants and action properties, its step bound
# and its deterministic-prefix constraint. Nothing is hand-copied, so a DSL
# change regenerates ci.cfg and the audit follows it; an anchor this script
# expects but cannot find fails the run instead of silently narrowing the
# check.
#
# It runs, in order:
#   - two safety runs (three early Steer inputs with recall; two with recall
#     and one admitted later): the exactly-once invariants, AuditRetainedOnce,
#     AuditRecalledNeverJoined, AuditJoinNotBeforeItsBoundary and the action
#     properties AuditRecalledIsFinal and AuditRecallKeepsTheOthers must hold;
#   - six witness runs, each of which must end in the named goal violated:
#     two and three early inputs all joined in the first stay at the model
#     boundary and retained; the first of three recalled, and the middle one
#     recalled, with the other two retained; three joins of one stay each
#     resolved on its own; an input accepted after the turn left the first
#     stay joined in the second;
#   - with --mutants, on a copy of the generated model: the durable join
#     without its model-boundary guard, the recall without its Queued guard
#     and a recall that drops every delivery, each of which the safety
#     invariant or action property written for it must refuse.
#
# Each TLC run is held to the canonical lane's per-run cap
# (`TLC_RUN_CAP_SECS`, default 900 s, see tlc_run_cap.sh); a run that hits it
# fails closed as TLC INCOMPLETE (exit 3).
set -euo pipefail

usage="usage: durable_multi_steer_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 20 )); then
  echo "error: max-steps must be an integer >= 20 (the deterministic prefix is up to 14 steps and the deepest witness is reached at 20)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/durable-multi-steer-audit.XXXXXX")"
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

safety_invariants="
  AuditRetainedJoinNeverRequeued
  AuditPublishedJoinIsStaged
  AuditJoinedInputNotInAnyLane
  AuditRetainedOnce
  AuditRecalledNeverJoined
  AuditJoinNotBeforeItsBoundary"
safety_properties="
  AuditRecalledIsFinal
  AuditRecallKeepsTheOthers"

# usage: write_cfg <file> <steer count: 2|3> <after: TRUE|FALSE>
#   <recall: TRUE|FALSE> <extra invariant lines> [<property lines>]
write_cfg() {
  local cfg="$1" count="$2" after="$3" recall="$4" extra="$5" properties="${6:-}"
  local ids='"input_batch", "input_a", "input_b"'
  if [[ "${count}" == "3" ]]; then
    ids+=', "input_c"'
  fi
  if [[ "${after}" == "TRUE" ]]; then
    ids+=', "input_after"'
  fi
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "  InputIdValues = {}" "  InputIdValues = {${ids}}" "${cfg}"
  replace_exact_line "  RunIdValues = {}" '  RunIdValues = {"runid_1"}' "${cfg}"
  replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1"}' "${cfg}"
  replace_exact_line "  StringValues = {}" "  StringValues = {${ids}}" "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}
  AuditSteerCount = ${count}
  AuditHasAfter = ${after}
  AuditRecall = ${recall}" "${cfg}"
  if [[ -n "${properties}" ]]; then
    replace_exact_line "INVARIANTS" "PROPERTIES${properties}
INVARIANTS${safety_invariants}${extra}" "${cfg}"
  else
    replace_exact_line "INVARIANTS" "INVARIANTS${safety_invariants}${extra}" "${cfg}"
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
  tlc_run_capped durable_multi_steer_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" durable_multi_steer_audit.tla || status=$?
  grep -E 'states generated|distinct states|is violated|Error:' "${work_dir}/${name}.log" \
    | sed "s/^/[${name}] /" || true
  return "${status}"
}

# usage: safety <name> <steer count> <after> <recall> [extra tlc args...]
safety() {
  local name="$1" count="$2" after="$3" recall="$4"
  shift 4
  local cfg="${work_dir}/${name}.cfg"
  write_cfg "${cfg}" "${count}" "${after}" "${recall}" "" "${safety_properties}"
  local status=0
  run_tlc "${name}" "${cfg}" "$@" || status=$?
  if [[ "${status}" != "0" ]] \
    || ! grep -q "Model checking completed. No error has been found." "${work_dir}/${name}.log"; then
    cat "${work_dir}/${name}.log" >&2
    echo "error: durable multi-steer audit ${name} failed (tlc exit ${status}, bound ${max_steps})" >&2
    exit 1
  fi
}

safety safety-three-with-recall 3 FALSE TRUE "$@"
safety safety-two-with-recall-and-a-later-one 2 TRUE TRUE "$@"

# usage: witness <name> <steer count> <after> <recall> <goal invariant>
#   [extra tlc args...]
witness() {
  local name="$1" count="$2" after="$3" recall="$4" goal="$5"
  shift 5
  local cfg="${work_dir}/${name}.cfg"
  write_cfg "${cfg}" "${count}" "${after}" "${recall}" "
  ${goal}"
  local status=0
  run_tlc "${name}" "${cfg}" "$@" || status=$?
  if ! grep -q "Error: Invariant ${goal} is violated" "${work_dir}/${name}.log"; then
    cat "${work_dir}/${name}.log" >&2
    echo "error: durable multi-steer audit witness ${name} is not reachable at bound ${max_steps} (tlc exit ${status})" >&2
    exit 1
  fi
  local depth
  depth="$(grep -cE '^State [0-9]+:' "${work_dir}/${name}.log" || true)"
  echo "witness ${name} reached in ${depth} states"
}

witness witness-two-retained-at-the-first-boundary 2 FALSE FALSE AuditEarlyNeverAllRetainedAtFirstBoundary "$@"
witness witness-three-retained-at-the-first-boundary 3 FALSE FALSE AuditEarlyNeverAllRetainedAtFirstBoundary "$@"
witness witness-head-recalled-others-retained 3 FALSE TRUE AuditHeadNeverRecalledWithOthersRetained "$@"
witness witness-middle-recalled-others-retained 3 FALSE TRUE AuditMiddleNeverRecalledWithOthersRetained "$@"
witness witness-each-join-resolved-on-its-own 3 FALSE FALSE AuditNeverMixedOutcomesAtFirstBoundary "$@"
witness witness-later-one-joins-the-next-boundary 2 TRUE FALSE AuditAfterNeverJoinsTheNextBoundary "$@"

if [[ "${run_mutants}" == "true" ]]; then
  # usage: mutate <name> <steer count> <after> <recall> <expected violation
  #   regex> <awk program> [extra tlc args...]; the awk program rewrites
  #   a copy of model.tla and exits non-zero when the anchor it expects is
  #   missing. The run (its safety invariants and action properties) must
  #   then fail with the expected violation.
  mutate() {
    local name="$1" count="$2" after="$3" recall="$4" expected="$5" program="$6"
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
    write_cfg "${cfg}" "${count}" "${after}" "${recall}" "" "${safety_properties}"
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

  # The durable join without turn_at_model_boundary: its one guard line that
  # reads the turn phase becomes TRUE, so an input accepted during a tool
  # round joins the stay the turn already left.
  mutate join-ignores-the-model-boundary 2 TRUE FALSE AuditJoinNotBeforeItsBoundary '
    /^JoinLiveBoundaryDurableAppendRunning\(/ { in_join = 1 }
    in_join && /^$/ { in_join = 0 }
    in_join && $0 == "    /\\ (turn_phase = \"CallingLlm\")" {
      hits++
      print "    /\\ TRUE"
      next
    }
    { print }
    END { exit hits == 1 ? 0 : 1 }
  ' "$@"

  # The recall without its Queued guard: its one guard line (the only
  # unprimed line of the action that reads the input phases) becomes TRUE,
  # so a recall abandons an input whose join was retained.
  mutate recall-ignores-the-queued-phase 2 FALSE TRUE AuditRecalledNeverJoined '
    /^AbandonQueuedInputRunning\(/ { in_recall = 1 }
    in_recall && /^$/ { in_recall = 0 }
    in_recall && /^    \/\\ / && index($0, "input_phases") > 0 && index($0, "'"'"'") == 0 {
      hits++
      print "    /\\ TRUE"
      next
    }
    { print }
    END { exit hits == 1 ? 0 : 1 }
  ' "$@"

  # The recall drops every live-boundary delivery instead of its own input's:
  # the Steer inputs waiting behind the recalled one stay in the lane but
  # can no longer join this run durably.
  mutate recall-drops-every-delivery 2 FALSE TRUE AuditRecallKeepsTheOthers '
    /^AbandonQueuedInputRunning\(/ { in_recall = 1 }
    in_recall && /^$/ { in_recall = 0 }
    in_recall && $0 == "    /\\ input_live_boundary_delivery'"'"' = MapRemove(input_live_boundary_delivery, input_id)" {
      hits++
      print "    /\\ input_live_boundary_delivery'"'"' = [x \\in {} |-> None]"
      next
    }
    { print }
    END { exit hits == 1 ? 0 : 1 }
  ' "$@"
fi

echo "durable multi-steer audit passed at model_step_count <= ${max_steps} (three early steers safe with recall, two with recall and a later one; two and three retained at the first boundary, also behind a recalled first or middle one; each join resolved on its own; a later steer at the next boundary$([[ "${run_mutants}" == "true" ]] && echo '; all three mutants refused'))"
