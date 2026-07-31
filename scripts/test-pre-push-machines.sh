#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-pre-push-machines.XXXXXX")"
HARNESS_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-pre-push-machines-harness.XXXXXX")"
trap 'rm -rf "$TEST_ROOT" "$HARNESS_ROOT"' EXIT

git -C "$TEST_ROOT" init -q
git -C "$TEST_ROOT" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit --allow-empty -qm "fixture"
test_head="$(git -C "$TEST_ROOT" rev-parse HEAD)"

CALL_LOG="${HARNESS_ROOT}/calls"
FAKE_CLASSIFIER="${HARNESS_ROOT}/classifier"
FAKE_CARGO="${HARNESS_ROOT}/cargo"
FAKE_MAKE="${HARNESS_ROOT}/make"
FAKE_GIT="${HARNESS_ROOT}/git"

cat > "$FAKE_CLASSIFIER" <<'EOF'
#!/usr/bin/env bash
exit "$MEERKAT_MACHINE_TEST_CLASSIFIER_STATUS"
EOF
cat > "$FAKE_CARGO" <<'EOF'
#!/usr/bin/env bash
printf 'cargo %s\n' "$*" >> "$MEERKAT_MACHINE_TEST_CALL_LOG"
if [[ "${MEERKAT_MACHINE_TEST_DIRTY_CODEGEN:-0}" == "1" ]]; then
  touch "$MEERKAT_MACHINE_TEST_ROOT/generated-untracked"
fi
EOF
cat > "$FAKE_MAKE" <<'EOF'
#!/usr/bin/env bash
printf 'make %s\n' "$*" >> "$MEERKAT_MACHINE_TEST_CALL_LOG"
EOF
cat > "$FAKE_GIT" <<'EOF'
#!/usr/bin/env bash
if [[ "$1" == "status" ]]; then
  exit 73
fi
exec git "$@"
EOF
chmod +x "$FAKE_CLASSIFIER" "$FAKE_CARGO" "$FAKE_MAKE" "$FAKE_GIT"

run_case() {
  local classifier_status="$1"
  local dirty_codegen="$2"
  : > "$CALL_LOG"
  (
    ROOT="$TEST_ROOT" \
      CARGO="$FAKE_CARGO" \
      MAKE_BIN="$FAKE_MAKE" \
      MACHINE_AUTHORITY_CHANGED="$FAKE_CLASSIFIER" \
      PRE_COMMIT_FROM_REF="$test_head" \
      PRE_COMMIT_TO_REF="$test_head" \
      MEERKAT_MACHINE_TEST_CLASSIFIER_STATUS="$classifier_status" \
      MEERKAT_MACHINE_TEST_DIRTY_CODEGEN="$dirty_codegen" \
      MEERKAT_MACHINE_TEST_CALL_LOG="$CALL_LOG" \
      MEERKAT_MACHINE_TEST_ROOT="$TEST_ROOT" \
      "$REPO_ROOT/scripts/pre-push-machines.sh"
  )
}

run_case 1 0
if [[ -s "$CALL_LOG" ]]; then
  echo "unchanged machine authority unexpectedly ran validation" >&2
  exit 1
fi

run_case 0 0
expected_calls=$'cargo xtask machine-codegen --all\nmake -C '"$TEST_ROOT"$' machine-verify'
if [[ "$(cat "$CALL_LOG")" != "$expected_calls" ]]; then
  echo "changed machine authority ran unexpected commands:" >&2
  cat "$CALL_LOG" >&2
  exit 1
fi

set +e
run_case 2 0 >/dev/null 2>&1
classifier_failure=$?
set -e
if [[ "$classifier_failure" -ne 2 || -s "$CALL_LOG" ]]; then
  echo "classifier error was not propagated exactly" >&2
  exit 1
fi

set +e
(
  ROOT="$TEST_ROOT" \
    CARGO="$FAKE_CARGO" \
    MAKE_BIN="$FAKE_MAKE" \
    MACHINE_AUTHORITY_CHANGED="$FAKE_CLASSIFIER" \
    GIT_BIN="$FAKE_GIT" \
    PRE_COMMIT_FROM_REF="$test_head" \
    PRE_COMMIT_TO_REF="$test_head" \
    MEERKAT_MACHINE_TEST_CLASSIFIER_STATUS=0 \
    MEERKAT_MACHINE_TEST_DIRTY_CODEGEN=0 \
    MEERKAT_MACHINE_TEST_CALL_LOG="$CALL_LOG" \
    MEERKAT_MACHINE_TEST_ROOT="$TEST_ROOT" \
    "$REPO_ROOT/scripts/pre-push-machines.sh"
) >/dev/null 2>&1
cleanliness_failure=$?
set -e
if [[ "$cleanliness_failure" -eq 0 ]]; then
  echo "failing cleanliness probe unexpectedly passed the machine gate" >&2
  exit 1
fi

mkdir -p "$TEST_ROOT/specs/machines/deletion_probe"
printf '%s\n' "---- MODULE deletion_probe ----" \
  > "$TEST_ROOT/specs/machines/deletion_probe/model.tla"
git -C "$TEST_ROOT" add specs/machines/deletion_probe/model.tla
git -C "$TEST_ROOT" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit -qm "add machine authority fixture"
deletion_base="$(git -C "$TEST_ROOT" rev-parse HEAD)"
git -C "$TEST_ROOT" rm -q specs/machines/deletion_probe/model.tla
git -C "$TEST_ROOT" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit -qm "delete machine authority fixture"
deletion_head="$(git -C "$TEST_ROOT" rev-parse HEAD)"
DELETION_CONFIG="${HARNESS_ROOT}/deletion-config.yaml"
cat > "$DELETION_CONFIG" <<EOF
repos:
  - repo: local
    hooks:
      - id: machine-deletion-probe
        name: machine deletion probe
        entry: ${REPO_ROOT}/scripts/pre-push-machines.sh
        language: system
        pass_filenames: false
        always_run: true
        stages: [pre-push]
EOF
: > "$CALL_LOG"
(
  cd "$TEST_ROOT"
  ROOT="$TEST_ROOT" \
    CARGO="$FAKE_CARGO" \
    MAKE_BIN="$FAKE_MAKE" \
    MACHINE_AUTHORITY_CHANGED="$FAKE_CLASSIFIER" \
    PRE_COMMIT_FROM_REF="$deletion_base" \
    PRE_COMMIT_TO_REF="$deletion_head" \
    MEERKAT_MACHINE_TEST_CLASSIFIER_STATUS=0 \
    MEERKAT_MACHINE_TEST_DIRTY_CODEGEN=0 \
    MEERKAT_MACHINE_TEST_CALL_LOG="$CALL_LOG" \
    MEERKAT_MACHINE_TEST_ROOT="$TEST_ROOT" \
    pre-commit run --config "$DELETION_CONFIG" machine-deletion-probe \
      --hook-stage pre-push --from-ref "$deletion_base" --to-ref "$deletion_head"
)
if ! grep -Fxq "cargo xtask machine-codegen --all" "$CALL_LOG"; then
  echo "deletion-only machine change skipped the always-run gate" >&2
  exit 1
fi

set +e
run_case 0 1 >/dev/null 2>&1
dirty_failure=$?
set -e
if [[ "$dirty_failure" -eq 0 ]]; then
  echo "dirty codegen unexpectedly passed the machine gate" >&2
  exit 1
fi
