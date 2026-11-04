#!/usr/bin/env bash
# Bounded TLC audit of the settlement of an until-changed switch-turn: the
# withdrawal of a failed one (AbortUntilChangedSwitchTurnReconfigure), the
# refusal of a second one while one is pending, and the completion in every
# phase the session can reach in the reconfigure gap.
#
# usage: switch_turn_withdrawal_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's finite
# model values (one session, one input, two requests, the model names and the
# stop reason), its four action properties and its step bound, and checks
# them over the whole bounded space. It then reruns the same space once per
# reachability witness, with that witness's negation as an extra action
# property, and requires TLC to report exactly that violation, so the
# properties are not vacuous. With --mutants it then seeds six defects into a
# copy of the generated model (an abort that records a terminal, an abort
# that erases the inputs' phases, an abort without its pending-request guard,
# an abort that keeps its request's approval entry, an accept without its
# pending-switch guard, a completion that keeps the old baseline) and
# requires TLC to refuse each with the property it breaks. An anchor this
# script expects but cannot find fails the run. Each TLC run is held to the
# canonical lane's per-run cap (tlc_run_cap.sh) and fails closed as TLC
# INCOMPLETE.
set -euo pipefail

usage="usage: switch_turn_withdrawal_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 9 )); then
  echo "error: max-steps must be an integer >= 9 (the longest witness is 9 steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/switch-turn-withdrawal-audit.XXXXXX")"
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
  replace_exact_line "  InputIdValues = {}" '  InputIdValues = {"input_1"}' "${cfg}"
  replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1"}' "${cfg}"
  replace_exact_line "  StringValues = {}" \
    '  StringValues = {"input_1", "request_1", "request_2", "model_baseline", "model_target", "stop"}' "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}" "${cfg}"
  replace_exact_line "INVARIANTS" "PROPERTIES
  AuditAbortWithdrawsOnlyTheRequest
  AuditAbortOnlyForThePendingRequest
  AuditPendingSwitchEndsOnlyWithItsRequest
  AuditCompleteSettlesTheRequest${extra}
INVARIANTS" "${cfg}"
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
  tlc_run_capped switch_turn_withdrawal_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" switch_turn_withdrawal_audit.tla || status=$?
  return "${status}"
}

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" ""
safety_status=0
run_tlc safety "${safety_cfg}" "$@" || safety_status=$?
cat "${work_dir}/safety.log"
if [[ "${safety_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: switch-turn withdrawal audit failed (tlc exit ${safety_status}, bound ${max_steps})" >&2
  exit 1
fi

for witness in QueuedInputKept StoppedWithdrawn ConflictRefused StoppedCompleted; do
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
  # this prelude, rewrites every arm of one family (the abort arms unless it
  # names another) of the copied model.tla in place and must find each anchor
  # it names.
  read -r -d '' mutant_prelude <<'PY' || true
import os, re
path = os.environ["MODEL"]
text = open(path).read()
ABORT_ARMS = ["AbortUntilChangedSwitchTurnReconfigure" + phase
              for phase in ("Idle", "Attached", "Running", "Retired", "Stopped")]
COMPLETE_ARMS = ["CompleteUntilChangedSwitchTurnReconfigure" + phase
                 for phase in ("Idle", "Attached", "Running", "Retired", "Stopped")]
ACCEPT_ARMS = ["RequestUntilChangedSwitchTurnAccepted" + phase
               for phase in ("Idle", "Attached", "Running")]

def _replace_body(arm, edit):
    global text
    found = list(re.finditer(r"^" + re.escape(arm) + r"(\([^)]*\))? ==\n.*?\n\n", text, re.S | re.M))
    assert len(found) == 1, f"expected one definition of {arm}, found {len(found)}"
    start, end = found[0].start(), found[0].end()
    text = text[:start] + edit(text[start:end]) + text[end:]

def frame_with(var, assignment, arms=ABORT_ARMS):
    """Every arm assigns `var`; its frame keeps each other variable."""
    def edit(body):
        lines = body.split("\n")
        hits = [i for i, line in enumerate(lines)
                if re.fullmatch(r"    /\\ UnchangedFrame_[0-9a-f]+", line)]
        assert len(hits) == 1, f"expected one frame line, found {len(hits)}"
        name = lines[hits[0]].split()[-1]
        definition = re.search(r"(?m)^" + name + r" == UNCHANGED << (.*) >>$", text)
        assert definition, f"frame {name} not found"
        listed = [v.strip() for v in definition.group(1).split(",")]
        kept = [v for v in listed if v != var]
        assert len(kept) + 1 == len(listed), f"{var} is not in frame {name}"
        lines[hits[0]] = ("    /\\ " + var + "' = " + assignment + "\n"
                          "    /\\ UNCHANGED << " + ", ".join(kept) + " >>")
        return "\n".join(lines)
    for arm in arms:
        _replace_body(arm, edit)

def keep_var(var, arms=ABORT_ARMS):
    """Every arm leaves `var` as it was instead of updating it."""
    def edit(body):
        lines = body.split("\n")
        hits = [i for i, line in enumerate(lines) if line.startswith("    /\\ " + var + "' = ")]
        assert len(hits) == 1, f"expected one update of {var}, found {len(hits)}"
        lines[hits[0]] = "    /\\ " + var + "' = " + var
        return "\n".join(lines)
    for arm in arms:
        _replace_body(arm, edit)

def drop_guard(needle, arms=ABORT_ARMS):
    """Delete the one guard line of every arm that contains `needle`."""
    def edit(body):
        lines = body.split("\n")
        hits = [i for i, line in enumerate(lines)
                if line.startswith("    /\\ ") and "'" not in line and needle in line]
        assert len(hits) == 1, f"expected one guard with {needle!r}, found {len(hits)}"
        del lines[hits[0]]
        return "\n".join(lines)
    for arm in arms:
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

  # An abort that records a terminal, as if the withdrawn request had ended.
  mutate abort_records_terminal \
    "Error: Action property AuditAbortWithdrawsOnlyTheRequest is violated" \
    'frame_with("model_routing_switch_terminal", "MapSet(model_routing_switch_terminal, request_id, \"Denied\")")' "$@"
  # An abort that erases the inputs' phases, as the whole-state restore it
  # replaced erased an input admitted in the reconfigure gap. The generated
  # invariants read input phases only under a domain check, so a refusal by
  # one of them is a refusal too.
  mutate abort_erases_input_phases \
    "Error: (Action property AuditAbortWithdrawsOnlyTheRequest|Invariant [A-Za-z_]+) is violated" \
    'frame_with("input_phases", "[x \\in {} |-> None]")' "$@"
  # An abort without its pending-request guard: a completed or withdrawn
  # request, or one never accepted, would withdraw what it does not own.
  mutate abort_ignores_pending_request \
    "Error: Action property AuditAbortOnlyForThePendingRequest is violated" \
    'drop_guard("model_routing_pending_switch_request_id")' "$@"
  # An abort that keeps its request's approval entry: an approval would be
  # left on record for a switch that never applied.
  mutate abort_keeps_approval \
    "Error: Action property AuditAbortWithdrawsOnlyTheRequest is violated" \
    'keep_var("model_routing_approval_phases")' "$@"
  # An accept without its pending-switch guard: a second request would
  # replace another request's pending switch, which could then neither
  # complete nor withdraw.
  mutate accept_replaces_pending \
    "Error: Action property AuditPendingSwitchEndsOnlyWithItsRequest is violated" \
    'drop_guard("model_routing_pending_switch_request_id", ACCEPT_ARMS)' "$@"
  # A completion that keeps the old baseline: a switch completed in a
  # stopped session would leave the baseline behind its applied model.
  mutate complete_keeps_baseline \
    "Error: Action property AuditCompleteSettlesTheRequest is violated" \
    'keep_var("model_routing_baseline_model", COMPLETE_ARMS)' "$@"
fi
echo "switch-turn withdrawal audit passed at model_step_count <= ${max_steps}"
