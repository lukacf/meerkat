# shellcheck shell=bash
# The sourcing script assigns what this file reads (work_dir, audit_cfg,
# max_steps, spec_dir, run_mutants, tlc_status) and reads the worker count a
# mutant run sets.
# shellcheck disable=SC2154,SC2034
# The recovery scene of the durable in-turn Steer audit. Sourced by
# durable_in_turn_steer_audit.sh when it is given --recovery-scene, after it
# has derived the audit's base config and defined run_tlc and
# replace_exact_line; never run on its own.
#
# The scene's prefix is eleven steps (twelve for a governed row, which is
# bound again). Each run then explores RECOVERY_FREE_STEPS free steps: the
# scene's questions are answered at the resumed run's first boundaries (a
# join is one step from the prefix, a second boundary three more), and what a
# joined input does after that is the main audit's, whose late input is in
# the same state once admitted. The default keeps the lane short; a deeper
# scene runs by hand with DURABLE_STEER_AUDIT_RECOVERY_FREE_STEPS.
recovery_free_steps="${DURABLE_STEER_AUDIT_RECOVERY_FREE_STEPS:-4}"
if ! [[ "${recovery_free_steps}" =~ ^[0-9]+$ ]] || (( recovery_free_steps < 4 )); then
  echo "error: DURABLE_STEER_AUDIT_RECOVERY_FREE_STEPS must be an integer >= 4 (a second boundary and its join)" >&2
  exit 2
fi

# recovery_cfg <name> <rows operator> <governed> [<extra invariant> ...]
#   -> ${work_dir}/<name>.cfg
recovery_cfg() {
  local name="$1" rows="$2" governed="$3"
  shift 3
  local cfg="${work_dir}/${name}.cfg"
  local bound=$(( 11 + recovery_free_steps ))
  if [[ "${governed}" == "TRUE" ]]; then
    bound=$(( bound + 1 ))
  fi
  cp "${audit_cfg}" "${cfg}"
  replace_exact_line "SPECIFICATION AuditSpec" "SPECIFICATION AuditRecoverySpec" "${cfg}"
  replace_exact_line "  AuditMaxSteps = ${max_steps}" "  AuditMaxSteps = ${bound}" "${cfg}"
  replace_exact_line "  AuditRows <- AuditRowsThatNeverLeft" "  AuditRows <- ${rows}" "${cfg}"
  replace_exact_line "  AuditRowGoverned = FALSE" "  AuditRowGoverned = ${governed}" "${cfg}"
  printf 'INVARIANT\n  AuditGovernedRecoveredInputHoldsNoClass\n' >> "${cfg}"
  local invariant
  for invariant in "$@"; do
    printf '  %s\n' "${invariant}" >> "${cfg}"
  done
}

# recovery_holds <name> <what failed>: the run must complete without an error.
recovery_holds() {
  local name="$1" what="$2"
  run_tlc "${name}" "${work_dir}/${name}.cfg"
  if [[ "${tlc_status}" != "0" ]] \
    || ! grep -q "Model checking completed. No error has been found." "${work_dir}/${name}.log"; then
    cat "${work_dir}/${name}.log" >&2
    echo "error: durable in-turn steer audit: ${what} (tlc exit ${tlc_status}, ${recovery_free_steps} free steps)" >&2
    exit 1
  fi
}

# The row of a steer that never left the Steer lane keeps its class: the
# scene is safe, and the recovered input joins the resumed run.
recovery_cfg recovery-never-left-safety AuditRowsThatNeverLeft FALSE
recovery_holds recovery-never-left-safety "the recovery scene failed for a row that never left the Steer lane"
recovery_cfg recovery-never-left-join AuditRowsThatNeverLeft FALSE AuditLateNeverJoins
run_tlc recovery-never-left-join "${work_dir}/recovery-never-left-join.cfg"
if ! grep -q "AuditLateNeverJoins is violated" "${work_dir}/recovery-never-left-join.log"; then
  cat "${work_dir}/recovery-never-left-join.log" >&2
  echo "error: durable in-turn steer audit: a recovered steer that never left the Steer lane never joins the resumed run within ${recovery_free_steps} free steps" >&2
  exit 1
fi

# Every row that left the lane holds no class after recovery and never joins.
recovery_cfg recovery-rows-that-left AuditRowsThatLeft FALSE AuditLateHoldsNoClass AuditLateNeverJoins
recovery_holds recovery-rows-that-left "a recovered row that left the Steer lane holds a class or joins the resumed run"

# A governed row that never left loses its class at its binding.
recovery_cfg recovery-governed AuditRowsThatNeverLeft TRUE AuditLateNeverJoins
recovery_holds recovery-governed "a governed recovered row holds a class or joins the resumed run"

if [[ "${run_mutants}" == "true" ]]; then
  # usage: mutate_recovery <name> <rows operator> <governed> <expected
  # violation regex> [<extra invariant> ...]; reads a python edit on stdin
  # that rewrites the copied model.tla through edit_arms, which fails on any
  # anchor it cannot find.
  mutate_recovery() {
    local name="$1" rows="$2" governed="$3" expected="$4"
    shift 4
    local dir="${work_dir}/mutant-${name}"
    mkdir -p "${dir}"
    cp "${spec_dir}"/*.tla "${dir}/"
    {
      cat <<'PRELUDE'
import os, re
path = os.environ["MODEL"]
text = open(path).read()
PHASES = ("Idle", "Attached", "Running", "Retired", "Stopped")

def edit_arms(name, select, rewrite):
    global text
    for phase in PHASES:
        arm = name + phase
        found = re.compile(r"^" + arm + r"\(.*?\n\n", re.S | re.M).search(text)
        assert found, f"{arm} not found"
        lines = found.group(0).split("\n")
        hits = [index for index, line in enumerate(lines) if select(line)]
        assert len(hits) == 1, f"expected one matching line in {arm}, found {len(hits)}"
        changed = rewrite(lines[hits[0]])
        assert changed != lines[hits[0]], f"the edit changed nothing in {arm}"
        lines[hits[0]] = changed
        text = text[: found.start()] + "\n".join(lines) + text[found.end():]

def class_update(line):
    return line.startswith("    /\\ input_live_boundary_delivery' = ")

def drop_conjunct(conjunct):
    def rewrite(line):
        assert line.count(conjunct) == 1, f"expected one {conjunct!r} in the recovered class update"
        return line.replace(conjunct, "")
    return rewrite
PRELUDE
      cat
      printf '%s\n' 'open(path, "w").write(text)'
    } | MODEL="${dir}/model.tla" python3 -
    local cfg_name="mutant-${name}"
    recovery_cfg "${cfg_name}" "${rows}" "${governed}" "$@"
    # One worker: TLC explores breadth first in order, so it reports the
    # shallowest violation, the one each mutant names.
    local workers=1
    (cd "${dir}" && run_tlc "${cfg_name}" "${work_dir}/${cfg_name}.cfg") || true
    if ! grep -qE "${expected}" "${work_dir}/${cfg_name}.log"; then
      cat "${work_dir}/${cfg_name}.log" >&2
      echo "error: durable in-turn steer audit: mutant ${name} was not refused by '${expected}'" >&2
      exit 1
    fi
    echo "mutant ${name} refused: ${expected}"
  }

  no_class="Invariant AuditLateHoldsNoClass is violated"

  # A row with a stage attempt gets its class back.
  mutate_recovery recovery_ignores_attempts AuditRowsThatLeft FALSE "${no_class}" AuditLateHoldsNoClass <<'PY'
edit_arms("RecoverInputLifecycle", class_update, drop_conjunct(" /\\ (attempt_count = 0)"))
PY

  # A row that still names a run gets its class back: the condition that
  # carries exactly once.
  mutate_recovery recovery_ignores_run AuditRowsThatLeft FALSE "${no_class}" AuditLateHoldsNoClass <<'PY'
edit_arms("RecoverInputLifecycle", class_update, drop_conjunct(" /\\ (run_id = None)"))
PY

  # A row in the Queue lane gets a class: the generated invariant refuses it
  # in the same state as the audit's.
  mutate_recovery recovery_ignores_lane AuditRowsThatLeft FALSE \
    "Invariant (live_boundary_delivery_only_for_queued_steer|AuditLateHoldsNoClass) is violated" \
    AuditLateHoldsNoClass <<'PY'
edit_arms("RecoverInputLifecycle", class_update, drop_conjunct(" /\\ (lane = Some(\"Steer\"))"))
PY

  # The binding recovery restores keeps a governed row's class.
  mutate_recovery binding_keeps_class AuditRowsThatNeverLeft TRUE \
    "Invariant AuditGovernedRecoveredInputHoldsNoClass is violated" <<'PY'
edit_arms(
    "BindInputAuthority",
    class_update,
    lambda line: "    /\\ input_live_boundary_delivery' = input_live_boundary_delivery",
)
PY
fi

echo "durable in-turn steer recovery scene passed with ${recovery_free_steps} free steps (the row that never left the Steer lane joins the resumed run; the four rows that left it and the governed row hold no class and never join)"
