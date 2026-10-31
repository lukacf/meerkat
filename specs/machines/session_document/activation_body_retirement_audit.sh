#!/usr/bin/env bash
# TLC audit of the activation-body retirement region of the generated
# SessionDocumentMachine, driven as the shell drives it and checked on the
# machine's own per-key state: only the retirement decision retires a body
# (a recovery carries the retired counts the transcript holds), the decision
# retires every row a later row of its key follows, never lowers a recovered
# count and changes nothing but the retired counts, and a key whose newest
# row is the session's own keeps that row's body.
#
# usage: activation_body_retirement_audit.sh [--mutants] [extra tlc args...]
#
# Derives the TLC config from the generated ci.cfg next to this script (every
# generated constant, invariant and the step constraint, unchanged) and adds
# the audit's invariants. It first checks that the generated model defines
# both arms with the parameters the audit passes, so a renamed arm or field
# fails the run instead of a vacuous pass. It then checks the invariants
# (safety), and reruns once per reachability witness, with that witness's
# negation as an extra invariant, requiring TLC to report exactly that
# violation. With --mutants it seeds five defects into a copy of the
# generated model and requires TLC to refuse each with the invariant it
# targets, checked as the run's only invariant:
#   - retire every row (the decision takes the row counts);
#   - retire nothing new (the decision keeps the recovered counts);
#   - drop the own-head clause from the recovery guard;
#   - retire inside the recovery;
#   - lower a recovered count (the decision takes the row counts less one,
#     whatever was retired).
# Every mutant keeps the three activation maps of the session over one key
# set (each mutated update ranges over the keys of the rows map, and the
# weakened recovery guard keeps the key-set clauses), so no mutant leaves a
# map lookup outside its domain for TLC to abort on (exit 75).
# No run uses -continue. TLC checks the invariants of every new state and of
# every successor past the step constraint, each time it generates one, and
# under -continue prints a whole trace for each violation but names only the
# first invariant a state violates, so a witness or a mutant target that
# another violated invariant implies can never be named. Without -continue a
# run stops at its first violation: a witness run at the first state it
# finds that reaches the witness, and a mutant run, whose only invariant is
# its target, exactly when some reachable state of the mutant breaks the
# target, whatever else that state breaks.
# Each TLC run is held to the canonical lane's per-run cap
# (`TLC_RUN_CAP_SECS`, default 900 s, see tlc_run_cap.sh); a run that hits
# it fails closed as TLC INCOMPLETE (exit 3).
# An anchor this script expects but cannot find fails the run.
set -euo pipefail

run_mutants=false
if [[ "${1:-}" == "--mutants" ]]; then
  run_mutants=true
  shift
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci_cfg="${spec_dir}/ci.cfg"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/activation-body-retirement-audit.XXXXXX")"
# An interrupted or killed run ends its TLC child too (tlc_run_cap.sh).
trap 'if declare -F tlc_run_cap_reap >/dev/null; then tlc_run_cap_reap; fi; rm -rf "${work_dir}"' EXIT

# The premise: the generated model defines both arms with the parameters,
# in the order, the audit passes them, and the helper and native they call.
MODEL="${spec_dir}/model.tla" python3 - <<'PY'
import os, re
text = open(os.environ["MODEL"]).read()
arms = {
    "RecoverSessionActivationBodies": "session_id, rows, retired, head_origin",
    "AuthorizeActivationBodyRetirement": "session_id",
}
for arm, params in arms.items():
    heads = re.findall(r"(?m)^" + arm + r"\(([^)]*)\) ==$", text)
    assert heads == [params], f"expected {arm}({params}) in model.tla, found {heads}"
for name in (
    "activation_body_maps_consistent(",
    "session_document_activation_retired_after(",
    "activation_body_maps_lockstep",
    "activation_effective_body_live",
):
    assert name in text, f"model.tla declares no {name}"
print("premise holds: the generated model defines both activation-body arms the audit drives")
PY

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

audit_invariants="  AuditRetiredChangesOnlyAtTheDecision
  AuditSupersededRowsRetired
  AuditOwnHeadKeepsItsBody
  AuditDecisionNeverLowersACount
  AuditDecisionKeepsRowsAndHeads"

witnesses="BodyRetired CopiedRetiredHeadKept ThreeRowsRetireTwo"

# usage: write_cfg <file> <extra invariant lines>
write_cfg() {
  local cfg="$1" extra="$2"
  cp "${ci_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION Spec" "SPECIFICATION AuditSpec" "${cfg}"
  replace_exact_line "INVARIANTS" "INVARIANTS
${audit_invariants}${extra}" "${cfg}"
  printf 'CHECK_DEADLOCK FALSE\n' >> "${cfg}"
}

# usage: write_target_cfg <file> <invariant>; the generated constants and
# step constraint, unchanged, with <invariant> as the only invariant.
write_target_cfg() {
  local cfg="$1" target="$2"
  write_cfg "${cfg}" ""
  CFG="${cfg}" TARGET="${target}" python3 - <<'PY'
import os, re
path, target = os.environ["CFG"], os.environ["TARGET"]
lines = open(path).read().split("\n")
start = lines.index("INVARIANTS") + 1
end = start
while end < len(lines) and lines[end].startswith("  "):
    end += 1
assert end < len(lines) and re.fullmatch(r"[A-Z_]+", lines[end]), f"no section follows the invariants in {path}"
lines[start:end] = ["  " + target]
open(path, "w").write("\n".join(lines))
PY
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
extra_tlc_args=("$@")
# Each TLC run is held to the canonical lane's per-run cap (fails closed as
# TLC INCOMPLETE, exit 3).
# shellcheck source=../meerkat_machine/tlc_run_cap.sh
source "${spec_dir}/../meerkat_machine/tlc_run_cap.sh"

# usage: run_tlc <name> <cfg> <dir> [tlc args...]; leaves the log at
# ${work_dir}/<name>.log
run_tlc() {
  local name="$1" cfg="$2" dir="$3"
  shift 3
  tlc_status=0
  # Witness and mutant runs end in a violation by design: -noGenerateSpecTE
  # keeps TLC from writing trace-exploration files next to the specs.
  tlc_run_capped activation_body_retirement_audit "${name}" "${work_dir}/${name}.log" \
    -workers "${workers}" -metadir "${work_dir}/${name}-states" -noGenerateSpecTE \
    -config "${cfg}" "$@" "${extra_tlc_args[@]}" "${dir}/activation_body_retirement_audit.tla" \
    || tlc_status=$?
}

safety_cfg="${work_dir}/safety.cfg"
write_cfg "${safety_cfg}" ""
run_tlc safety "${safety_cfg}" "${spec_dir}"
cat "${work_dir}/safety.log"
if [[ "${tlc_status}" != "0" ]] \
  || ! grep -q "Model checking completed. No error has been found." "${work_dir}/safety.log"; then
  echo "error: activation body retirement audit failed (tlc exit ${tlc_status})" >&2
  exit 1
fi

# One run per witness: the safety run showed that every other invariant
# holds, so the run stops at the first state it finds that reaches the
# witness.
for witness in ${witnesses}; do
  witness_cfg="${work_dir}/witness-${witness}.cfg"
  write_cfg "${witness_cfg}" "
  NotAuditWitness${witness}"
  run_tlc "witness-${witness}" "${witness_cfg}" "${spec_dir}"
  witness_log="${work_dir}/witness-${witness}.log"
  if ! grep -q "Error: Invariant NotAuditWitness${witness} is violated" "${witness_log}"; then
    cat "${witness_log}"
    echo "error: witness ${witness} is not reached (tlc exit ${tlc_status})" >&2
    exit 1
  fi
  depth="$(grep -cE '^State [0-9]+:' "${witness_log}" || true)"
  echo "witness ${witness} reached in ${depth} states"
done

if [[ "${run_mutants}" == "true" ]]; then
  # usage: mutate <name> <target invariant> <arm> <anchor> <new line>
  # Copies the specs, replaces the one line of <arm> that contains <anchor>
  # with <new line>, runs the target as the only invariant, and requires TLC
  # to report it violated.
  mutate() {
    local name="$1" target="$2" arm="$3" anchor="$4" line="$5"
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}/model.tla" "${spec_dir}/activation_body_retirement_audit.tla" "${dir}/"
    MODEL="${dir}/model.tla" ARM="${arm}" ANCHOR="${anchor}" LINE="${line}" python3 - <<'PY'
import os
path = os.environ["MODEL"]
arm, anchor, line = os.environ["ARM"], os.environ["ANCHOR"], os.environ["LINE"]
text = open(path).read()
start = text.index("\n" + arm + "(") + 1
end = text.index("\n\n", start)
lines = text[start:end].split("\n")
hits = [i for i, current in enumerate(lines) if anchor in current]
assert len(hits) == 1, f"expected one line with {anchor!r} in {arm}, found {len(hits)}"
lines[hits[0]] = line
text = text[:start] + "\n".join(lines) + text[end:]
open(path, "w").write(text)
PY
    local cfg="${work_dir}/mutant-${name}.cfg"
    write_target_cfg "${cfg}" "${target}"
    run_tlc "mutant-${name}" "${cfg}" "${dir}"
    local log="${work_dir}/mutant-${name}.log"
    if ! grep -q "Error: Invariant ${target} is violated" "${log}"; then
      cat "${log}"
      echo "error: mutant ${name} was not refused by ${target} (tlc exit ${tlc_status})" >&2
      exit 1
    fi
    local depth
    depth="$(grep -cE '^State [0-9]+:' "${log}" || true)"
    echo "mutant ${name} refused by ${target} in ${depth} states"
  }

  mutate retire_every_row AuditOwnHeadKeepsItsBody \
    AuthorizeActivationBodyRetirement "session_activation_retired' =" \
    "    /\\ session_activation_retired' = MapSet(session_activation_retired, session_id, session_activation_rows[session_id])"
  mutate retire_nothing_new AuditSupersededRowsRetired \
    AuthorizeActivationBodyRetirement "session_activation_retired' =" \
    "    /\\ session_activation_retired' = session_activation_retired"
  mutate own_head_clause_dropped AuditOwnHeadKeepsItsBody \
    RecoverSessionActivationBodies 'activation_body_maps_consistent(' \
    '    /\ (DOMAIN rows = DOMAIN retired) /\ (DOMAIN rows = DOMAIN head_origin) /\ (\A activation_key \in DOMAIN rows : rows[activation_key] >= 1 /\ retired[activation_key] <= rows[activation_key])'
  mutate retire_inside_recovery AuditRetiredChangesOnlyAtTheDecision \
    RecoverSessionActivationBodies "session_activation_retired' =" \
    "    /\\ session_activation_retired' = MapSet(session_activation_retired, session_id, session_document_activation_retired_after(rows, retired))"
  mutate lower_a_recovered_count AuditDecisionNeverLowersACount \
    AuthorizeActivationBodyRetirement "session_activation_retired' =" \
    "    /\\ session_activation_retired' = MapSet(session_activation_retired, session_id, [activation_key \\in DOMAIN session_activation_rows[session_id] |-> IF session_activation_rows[session_id][activation_key] > 0 THEN session_activation_rows[session_id][activation_key] - 1 ELSE 0])"
fi
echo "activation body retirement audit passed"
