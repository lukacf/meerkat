#!/usr/bin/env bash
# Bounded TLC audit of the scoped tool grant gate's server gate.
#
# usage: tool_grant_gate_audit.sh <max-steps> [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant and invariant, unchanged), then adds the audit's finite
# model values (one session, two tool names, two servers), its five action
# properties and its step bound, and checks them over the whole bounded
# space. It then reruns the same space once per reachability witness, with
# that witness's negation as an extra invariant, and requires TLC to report
# exactly that violation, so the properties are not vacuous. With --mutants
# it then seeds nine defects into a copy of the generated model (an extension
# that opens its names, an extension without its server guard, a gated stage
# without its server guard, an open set without its gated-names guard, an
# extension that leaves an added name ignored, a gate install without either
# disjointness guard, and a gated stage whose server predicate drops its
# ungated exception or its mode test) and requires TLC to refuse each with
# the property or invariant it breaks. An anchor this script expects but
# cannot find fails the run.
set -euo pipefail

usage="usage: tool_grant_gate_audit.sh <max-steps> [--mutants] [extra tlc args...]"
max_steps="${1:?${usage}}"
shift
run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi
if ! [[ "${max_steps}" =~ ^[0-9]+$ ]] || (( max_steps < 5 )); then
  echo "error: max-steps must be an integer >= 5 (the longest witness is 5 steps)" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/tool-grant-gate-audit.XXXXXX")"
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
  replace_exact_line "  SessionIdValues = {}" '  SessionIdValues = {"sessionid_1"}' "${cfg}"
  replace_exact_line "  StringValues = {}" '  StringValues = {"server_1", "server_2"}' "${cfg}"
  replace_exact_line "  SetOfStringValues = {{}}" \
    '  SetOfStringValues = {{}, {"server_1"}, {"server_2"}, {"server_1", "server_2"}}' "${cfg}"
  replace_exact_line "  ToolNameValues = {}" '  ToolNameValues = {"toolname_1", "toolname_2"}' "${cfg}"
  replace_exact_line "  SetOfToolNameValues = {{}}" \
    '  SetOfToolNameValues = {{}, {"toolname_1"}, {"toolname_2"}, {"toolname_1", "toolname_2"}}' "${cfg}"
  replace_exact_line "CONSTANTS" "CONSTANTS
  AuditMaxSteps = ${max_steps}" "${cfg}"
  replace_exact_line "INVARIANTS" "PROPERTIES
  AuditGatedNamesShrinkOnlyOnReplace
  AuditServersChangeOnlyOnReplace
  AuditExtendNeverOpens
  AuditExtendOnlyForGatedServers
  AuditGatedStageOnlyStagesGatedServers
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
    tool_grant_gate_audit.tla > "${work_dir}/${name}.log" 2>&1
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
  echo "error: tool grant gate audit failed (tlc exit ${safety_status}, bound ${max_steps})" >&2
  exit 1
fi

for witness in GatedServer IgnoredName ModeWithUngated ExtendChanged ExtendUnchanged \
  ExtendByMode ExtendGatedIgnored OpenedExtended StagedNamed StagedByMode; do
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

  # An extension that also opens its names: the action assigns the open names
  # and runs under a copy of its frame without them.
  mutate extend_opens "Action property AuditExtendNeverOpens" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^ExtendToolGrantGateChangedIdle\(.*?\n\n", text, re.S | re.M)
assert block, "ExtendToolGrantGateChangedIdle not found"
body = block.group(0)
frames = re.findall(r"UnchangedFrame_[0-9a-f]+", body)
assert len(frames) == 1, "expected one frame in ExtendToolGrantGateChangedIdle, found %d" % len(frames)
frame = frames[0]
definition = re.search(r"^" + frame + r" == (UNCHANGED << .* >>)$", text, re.M)
assert definition, frame + " definition not found"
tuple_text = definition.group(1)
assert tuple_text.count(", tool_grant_open_names,") == 1, "the frame must hold the open names once"
mutant_tuple = tuple_text.replace(", tool_grant_open_names,", ",")
body = body.replace(
    "    /\\ " + frame,
    "    /\\ tool_grant_open_names' = (tool_grant_open_names \\cup gated)\n"
    "    /\\ UnchangedFrameExtendOpens",
)
text = text[: block.start()] + body + text[block.end():]
text = text.replace(
    definition.group(0),
    definition.group(0) + "\nUnchangedFrameExtendOpens == " + mutant_tuple,
    1,
)
open(path, "w").write(text)
PY

  mutate extend_without_server_guard "Action property AuditExtendOnlyForGatedServers" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^ExtendToolGrantGateChangedIdle\(.*?\n\n", text, re.S | re.M)
assert block, "ExtendToolGrantGateChangedIdle not found"
body = block.group(0)
lines = body.split("\n")
hits = [i for i, line in enumerate(lines) if "tool_grant_gated_servers" in line and "/\\ (" in line]
assert len(hits) == 1, "expected one server guard in ExtendToolGrantGateChangedIdle, found %d" % len(hits)
lines[hits[0]] = "    /\\ TRUE"
text = text[: block.start()] + "\n".join(lines) + text[block.end():]
open(path, "w").write(text)
PY

  mutate gated_stage_without_server_guard "Action property AuditGatedStageOnlyStagesGatedServers" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^SurfaceStageGatedAddIdle\(.*?\n\n", text, re.S | re.M)
assert block, "SurfaceStageGatedAddIdle not found"
body = block.group(0)
lines = body.split("\n")
hits = [i for i, line in enumerate(lines) if "tool_grant_gated_servers" in line and "/\\ (" in line]
assert len(hits) == 1, "expected one server guard in SurfaceStageGatedAddIdle, found %d" % len(hits)
lines[hits[0]] = "    /\\ TRUE"
text = text[: block.start()] + "\n".join(lines) + text[block.end():]
open(path, "w").write(text)
PY

  mutate open_without_gated_guard "Invariant tool_grant_open_names_are_gated" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^ApplyToolGrantOpenSetChangedIdle\(.*?\n\n", text, re.S | re.M)
assert block, "ApplyToolGrantOpenSetChangedIdle not found"
old = "    /\\ (\\A name \\in open : (name \\in tool_grant_gated_names))"
body = block.group(0)
assert body.count(old) == 1, "expected one gated-names guard in ApplyToolGrantOpenSetChangedIdle"
body = body.replace(old, "    /\\ TRUE")
text = text[: block.start()] + body + text[block.end():]
open(path, "w").write(text)
PY

  mutate extend_keeps_ignored "Invariant tool_grant_not_mounted_are_not_gated" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^ExtendToolGrantGateChangedIdle\(.*?\n\n", text, re.S | re.M)
assert block, "ExtendToolGrantGateChangedIdle not found"
body = block.group(0)
lines = body.split("\n")
hits = [i for i, line in enumerate(lines) if line.startswith("    /\\ tool_grant_not_mounted_names' = ")]
assert len(hits) == 1, "expected one ignored-names update in ExtendToolGrantGateChangedIdle, found %d" % len(hits)
lines[hits[0]] = "    /\\ tool_grant_not_mounted_names' = tool_grant_not_mounted_names"
text = text[: block.start()] + "\n".join(lines) + text[block.end():]
open(path, "w").write(text)
PY

  mutate replace_without_ignored_guard "Invariant tool_grant_not_mounted_are_not_gated" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^ReplaceToolGrantGateIdle\(.*?\n\n", text, re.S | re.M)
assert block, "ReplaceToolGrantGateIdle not found"
body = block.group(0)
lines = body.split("\n")
hits = [i for i, line in enumerate(lines) if line.startswith("    /\\ (\\A name \\in not_mounted_names")]
assert len(hits) == 1, "expected one ignored-names guard in ReplaceToolGrantGateIdle, found %d" % len(hits)
lines[hits[0]] = "    /\\ TRUE"
text = text[: block.start()] + "\n".join(lines) + text[block.end():]
open(path, "w").write(text)
PY

  mutate replace_without_server_guard "Invariant tool_grant_ungated_servers_are_not_gated" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^ReplaceToolGrantGateIdle\(.*?\n\n", text, re.S | re.M)
assert block, "ReplaceToolGrantGateIdle not found"
body = block.group(0)
lines = body.split("\n")
hits = [i for i, line in enumerate(lines) if line.startswith("    /\\ (\\A server \\in ungated_servers")]
assert len(hits) == 1, "expected one server-sets guard in ReplaceToolGrantGateIdle, found %d" % len(hits)
lines[hits[0]] = "    /\\ TRUE"
text = text[: block.start()] + "\n".join(lines) + text[block.end():]
open(path, "w").write(text)
PY

  mutate stage_drops_ungated_exception "Action property AuditGatedStageOnlyStagesGatedServers" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^SurfaceStageGatedAddIdle\(.*?\n\n", text, re.S | re.M)
assert block, "SurfaceStageGatedAddIdle not found"
body = block.group(0)
old = " /\\ ~((surface_id \\in tool_grant_ungated_servers))"
assert body.count(old) == 1, "expected one ungated exception in SurfaceStageGatedAddIdle"
body = body.replace(old, "")
text = text[: block.start()] + body + text[block.end():]
open(path, "w").write(text)
PY

  mutate stage_drops_mode_test "Action property AuditGatedStageOnlyStagesGatedServers" "$@" <<'PY'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
block = re.search(r"^SurfaceStageGatedAddIdle\(.*?\n\n", text, re.S | re.M)
assert block, "SurfaceStageGatedAddIdle not found"
body = block.group(0)
old = "tool_grant_gates_unconfigured_servers /\\ "
assert body.count(old) == 1, "expected one mode test in SurfaceStageGatedAddIdle"
body = body.replace(old, "")
text = text[: block.start()] + body + text[block.end():]
open(path, "w").write(text)
PY
fi
echo "tool grant gate audit passed at model_step_count <= ${max_steps}"
