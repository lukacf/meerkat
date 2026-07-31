#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-pre-push-dispatch.XXXXXX")"
HARNESS_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-pre-push-dispatch-harness.XXXXXX")"
trap 'rm -rf "$TEST_ROOT" "$HARNESS_ROOT"' EXIT

git -C "$TEST_ROOT" init -q
git -C "$TEST_ROOT" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit --allow-empty -qm "base"
base_sha="$(git -C "$TEST_ROOT" rev-parse HEAD)"
git -C "$TEST_ROOT" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit --allow-empty -qm "candidate"
head_sha="$(git -C "$TEST_ROOT" rev-parse HEAD)"

FAKE_PRE_COMMIT="${HARNESS_ROOT}/pre-commit"
INVOCATION_LOG="${HARNESS_ROOT}/invocation"
cat > "$FAKE_PRE_COMMIT" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
{
  printf 'args=%s\n' "$*"
  printf 'cwd=%s\n' "$PWD"
  printf 'head=%s\n' "$(git rev-parse HEAD)"
  printf 'to=%s\n' "${PRE_COMMIT_TO_REF:-}"
  printf 'from=%s\n' "${PRE_COMMIT_FROM_REF:-}"
  printf 'remote_name=%s\n' "${PRE_COMMIT_REMOTE_NAME:-}"
  printf 'remote_url=%s\n' "${PRE_COMMIT_REMOTE_URL:-}"
  printf 'lane=%s\n' "${RUST_LANE_ID:-}"
  if [[ -e dirty-source-only ]]; then
    printf 'dirty_source_visible=yes\n'
  fi
} > "$MEERKAT_DISPATCH_INVOCATION_LOG"
EOF
chmod +x "$FAKE_PRE_COMMIT"

run_dispatch() {
  local stdin_payload="$1"
  (
    cd "$TEST_ROOT"
    PATH="${HARNESS_ROOT}:$PATH" \
      MEERKAT_DISPATCH_INVOCATION_LOG="$INVOCATION_LOG" \
      RUST_LANE_ID="" \
      "$REPO_ROOT/scripts/pre-push-dispatch.sh" origin example.invalid \
      <<<"$stdin_payload"
  )
}

assert_log_line() {
  local expected="$1"
  if ! grep -Fxq "$expected" "$INVOCATION_LOG"; then
    echo "missing dispatcher log line: ${expected}" >&2
    sed -n '1,120p' "$INVOCATION_LOG" >&2
    exit 1
  fi
}

touch "$TEST_ROOT/dirty-source-only"
: > "$INVOCATION_LOG"
run_dispatch "refs/heads/main ${head_sha} refs/heads/main ${base_sha}"
assert_log_line "args=run --config .pre-commit-config.yaml --hook-stage pre-push --from-ref ${base_sha} --to-ref ${head_sha}"
assert_log_line "head=${head_sha}"
assert_log_line "to=${head_sha}"
assert_log_line "from=${base_sha}"
assert_log_line "remote_name=origin"
assert_log_line "remote_url=example.invalid"
assert_log_line "lane=pre-push"
if grep -Fq "dirty_source_visible=yes" "$INVOCATION_LOG"; then
  echo "dispatcher exposed dirty source-worktree bytes to validation" >&2
  exit 1
fi
validated_cwd="$(sed -n 's/^cwd=//p' "$INVOCATION_LOG")"
if [[ -e "$validated_cwd" ]]; then
  echo "dispatcher leaked its detached validation worktree: ${validated_cwd}" >&2
  exit 1
fi

: > "$INVOCATION_LOG"
run_dispatch "refs/heads/new ${head_sha} refs/heads/new ${ZERO_SHA:-0000000000000000000000000000000000000000}"
assert_log_line "args=run --config .pre-commit-config.yaml --hook-stage pre-push --all-files"
assert_log_line "from=4b825dc642cb6eb9a060e54bf8d69288fbee4904"

tag_object="$(git -C "$TEST_ROOT" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  tag -a dispatch-test -m dispatch-test && git -C "$TEST_ROOT" rev-parse dispatch-test)"
: > "$INVOCATION_LOG"
run_dispatch "refs/tags/dispatch-test ${tag_object} refs/tags/dispatch-test 0000000000000000000000000000000000000000"
assert_log_line "head=${head_sha}"
assert_log_line "to=${head_sha}"

assert_rejected_without_invocation() {
  local label="$1"
  local stdin_payload="$2"
  : > "$INVOCATION_LOG"
  set +e
  run_dispatch "$stdin_payload" >/dev/null 2>&1
  local status=$?
  set -e
  if [[ "$status" -eq 0 ]]; then
    echo "dispatcher ${label} case unexpectedly succeeded" >&2
    exit 1
  fi
  if [[ -s "$INVOCATION_LOG" ]]; then
    echo "dispatcher ${label} case invoked pre-commit before rejecting" >&2
    exit 1
  fi
}

assert_rejected_without_invocation \
  "non-HEAD" \
  "refs/heads/base ${base_sha} refs/heads/base 0000000000000000000000000000000000000000"
assert_rejected_without_invocation \
  "multi-ref" \
  "$(printf 'refs/heads/main %s refs/heads/main %s\nrefs/tags/x %s refs/tags/x %s' \
    "$head_sha" "$base_sha" "$tag_object" 0000000000000000000000000000000000000000)"
