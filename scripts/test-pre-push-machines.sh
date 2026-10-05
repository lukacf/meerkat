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
# Dirty the tree only on the protocol-codegen call, so the protocol
# clean-tree check is proven to fail on its own rather than inheriting the
# machine-codegen check's verdict.
if [[ "${MEERKAT_MACHINE_TEST_DIRTY_PROTOCOL_CODEGEN:-0}" == "1" \
      && "$*" == "xtask protocol-codegen" ]]; then
  touch "$MEERKAT_MACHINE_TEST_ROOT/protocol-generated-untracked"
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
  local mode="${3:-}"
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
      "$REPO_ROOT/scripts/pre-push-machines.sh" ${mode:+"$mode"}
  )
}

run_case 1 0
if [[ -s "$CALL_LOG" ]]; then
  echo "unchanged machine authority unexpectedly ran validation" >&2
  exit 1
fi

run_case 0 0
expected_calls=$'cargo xtask machine-codegen --all\ncargo xtask protocol-codegen\nmake -C '"$TEST_ROOT"$' machine-verify'
if [[ "$(cat "$CALL_LOG")" != "$expected_calls" ]]; then
  echo "changed machine authority ran unexpected commands:" >&2
  cat "$CALL_LOG" >&2
  exit 1
fi

# The split hooks: machine-codegen-drift runs only the two codegens under the
# clean-tree contract, machine-codegen-verify only the TLC lane, each behind
# the same classifier.
run_case 0 0 --codegen-only
expected_calls=$'cargo xtask machine-codegen --all\ncargo xtask protocol-codegen'
if [[ "$(cat "$CALL_LOG")" != "$expected_calls" ]]; then
  echo "--codegen-only ran unexpected commands:" >&2
  cat "$CALL_LOG" >&2
  exit 1
fi
run_case 0 0 --verify-only
if [[ "$(cat "$CALL_LOG")" != "make -C ${TEST_ROOT} machine-verify" ]]; then
  echo "--verify-only ran unexpected commands:" >&2
  cat "$CALL_LOG" >&2
  exit 1
fi
for mode in --codegen-only --verify-only; do
  run_case 1 0 "$mode"
  if [[ -s "$CALL_LOG" ]]; then
    echo "unchanged machine authority ran validation under ${mode}" >&2
    exit 1
  fi
done
if "$REPO_ROOT/scripts/pre-push-machines.sh" --bogus >/dev/null 2>&1; then
  echo "an unknown mode was accepted" >&2
  exit 1
fi

# The protocol clean-tree check must fail on its own. Dirty the tree only on
# the protocol-codegen call, so machine-codegen's check passes first and the
# non-zero exit can only come from the protocol check.
: > "$CALL_LOG"
set +e
(
  ROOT="$TEST_ROOT" \
    CARGO="$FAKE_CARGO" \
    MAKE_BIN="$FAKE_MAKE" \
    MACHINE_AUTHORITY_CHANGED="$FAKE_CLASSIFIER" \
    PRE_COMMIT_FROM_REF="$test_head" \
    PRE_COMMIT_TO_REF="$test_head" \
    MEERKAT_MACHINE_TEST_CLASSIFIER_STATUS=0 \
    MEERKAT_MACHINE_TEST_DIRTY_CODEGEN=0 \
    MEERKAT_MACHINE_TEST_DIRTY_PROTOCOL_CODEGEN=1 \
    MEERKAT_MACHINE_TEST_CALL_LOG="$CALL_LOG" \
    MEERKAT_MACHINE_TEST_ROOT="$TEST_ROOT" \
    "$REPO_ROOT/scripts/pre-push-machines.sh"
) >/dev/null 2>&1
dirty_protocol_failure=$?
set -e
rm -f "$TEST_ROOT/protocol-generated-untracked"
if [[ "$dirty_protocol_failure" -eq 0 ]]; then
  echo "dirty protocol codegen unexpectedly passed the machine gate" >&2
  exit 1
fi
if ! grep -Fxq "cargo xtask protocol-codegen" "$CALL_LOG"; then
  echo "protocol codegen was not invoked by the machine gate" >&2
  exit 1
fi
if grep -Fq "machine-verify" "$CALL_LOG"; then
  echo "dirty protocol codegen did not stop before machine-verify" >&2
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
