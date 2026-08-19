#!/usr/bin/env bash
# Pre-push deterministic test gate:
# - validates the exact detached pushed tree selected by pre-push-dispatch.sh
# - reuses source-test evidence across root lockfile-only commits
# - serializes runs so repeated pushes don't fight each other
# - retries nextest once if discovery hangs
set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"
GIT_BIN="${GIT_BIN:-git}"

CACHE_VERSION="v9"
NEXTEST_TIMEOUT_SECS="${MEERKAT_PRE_PUSH_NEXTEST_TIMEOUT_SECS:-300}"
BUILD_TIMEOUT_SECS="${MEERKAT_PRE_PUSH_BUILD_TIMEOUT_SECS:-${MEERKAT_PRE_PUSH_NARROW_BUILD_TIMEOUT_SECS:-900}}"
# The unit lane includes a dense-topology stress test with its own 300-second
# assertion budget. Compilation is a separate lane so linker time cannot spend
# the test-runtime budget.
UNIT_NEXTEST_TIMEOUT_SECS="${MEERKAT_PRE_PUSH_UNIT_NEXTEST_TIMEOUT_SECS:-900}"
# The integration lane includes downstream compile-policy canaries and more
# than 2,000 tests. Exact-tree linking is likewise budgeted separately.
INTEGRATION_NEXTEST_TIMEOUT_SECS="${MEERKAT_PRE_PUSH_INTEGRATION_NEXTEST_TIMEOUT_SECS:-900}"
LOCK_WAIT_SECS="${MEERKAT_PRE_PUSH_UNIT_LOCK_WAIT_SECS:-180}"
GIT_DIR_PATH="$("$GIT_BIN" rev-parse --git-common-dir)"
HOOK_CACHE_ROOT="${GIT_DIR_PATH}/meerkat-hook-cache"
HOOK_CACHE_DIR="${HOOK_CACHE_ROOT}/deterministic"
LOCK_DIR="${HOOK_CACHE_ROOT}/deterministic.lock"
PID_FILE="${LOCK_DIR}/pid"

mkdir -p "$HOOK_CACHE_DIR"

tree_key() {
  if "$GIT_BIN" rev-parse --verify 'HEAD^{tree}' >/dev/null 2>&1; then
    "$GIT_BIN" rev-parse 'HEAD^{tree}'
  else
    "$GIT_BIN" write-tree
  fi
}

source_test_fingerprint() {
  local tree_record tree_path
  # Root dependency locks are validated by the preceding Cargo and Bazel lock
  # gates, while CI remains authoritative for advisories. The always-run
  # changed-crate Clippy gate also compiles their resolved workspace graph.
  # Excluding only these two generated lock artifacts lets that narrow
  # dependency evidence reuse source-test results; every other tracked byte
  # remains fail-closed because tests may read fixtures or configuration with
  # arbitrary extensions.
  "$GIT_BIN" ls-tree -rz --full-tree HEAD |
    while IFS= read -r -d '' tree_record; do
      tree_path="${tree_record#*$'\t'}"
      case "$tree_path" in
        Cargo.lock | MODULE.bazel.lock)
          continue
          ;;
      esac
      printf '%s\0' "$tree_record"
    done |
    "$GIT_BIN" hash-object --stdin
}

require_exact_clean_head() {
  local expected_head="${PRE_COMMIT_TO_REF:-}"
  local actual_head worktree_status
  actual_head="$("$GIT_BIN" rev-parse HEAD)"
  if [[ -z "$expected_head" ]]; then
    echo "Pre-push validation requires PRE_COMMIT_TO_REF from the raw exact-ref dispatcher." >&2
    echo "Reinstall repository hooks with: make install-hooks" >&2
    return 1
  fi
  if [[ "$expected_head" != "$actual_head" ]]; then
    echo "Pre-push validation requires PRE_COMMIT_TO_REF (${expected_head}) to equal checked-out HEAD (${actual_head})." >&2
    echo "Push the checked-out branch alone, or validate the other ref from its own clean checkout." >&2
    return 1
  fi
  # Earlier push-stage hooks may rewrite generated files byte-identical,
  # leaving a stale index stat entry; refresh so the clean gate judges
  # content, not mtimes.
  "$GIT_BIN" update-index -q --refresh || true
  if ! worktree_status="$("$GIT_BIN" status --porcelain=v1 --untracked-files=all)"; then
    echo "Failed to determine whether the exact pushed worktree is clean." >&2
    return 1
  fi
  if [[ -n "$worktree_status" ]]; then
    echo "Pre-push validation requires a clean worktree so tested bytes equal pushed HEAD." >&2
    "$GIT_BIN" status --short --untracked-files=all >&2
    return 1
  fi
}

descendants_of() {
  local pid="$1"
  local child
  while read -r child; do
    [ -n "$child" ] || continue
    descendants_of "$child"
    echo "$child"
  done < <(pgrep -P "$pid" || true)
}

terminate_tree() {
  local pid="$1"
  local child
  while read -r child; do
    [ -n "$child" ] || continue
    kill "$child" 2>/dev/null || true
  done < <(descendants_of "$pid")
  kill "$pid" 2>/dev/null || true
}

acquire_lock() {
  local start_ts now_ts owner_pid
  start_ts=$(date +%s)

  while ! mkdir "$LOCK_DIR" 2>/dev/null; do
    owner_pid=""
    if [[ -f "$PID_FILE" ]]; then
      owner_pid="$(cat "$PID_FILE" 2>/dev/null || true)"
    fi
    if [[ -n "$owner_pid" && ! "$owner_pid" =~ ^[0-9]+$ ]]; then
      owner_pid=""
    fi
    if [[ -n "$owner_pid" ]]; then
      if ! kill -0 "$owner_pid" 2>/dev/null; then
        rm -rf "$LOCK_DIR"
        continue
      fi
    fi

    now_ts=$(date +%s)
    if (( now_ts - start_ts >= LOCK_WAIT_SECS )); then
      echo "Timed out waiting ${LOCK_WAIT_SECS}s for pre-push unit lock." >&2
      return 1
    fi
    sleep 1
  done

  echo "$$" > "$PID_FILE"
}

release_lock() {
  rm -rf "$LOCK_DIR"
}

run_with_timeout() {
  local timeout_secs="$1"
  shift

  "$@" &
  local cmd_pid=$!
  local start_ts now_ts
  start_ts=$(date +%s)

  while kill -0 "$cmd_pid" 2>/dev/null; do
    now_ts=$(date +%s)
    if (( now_ts - start_ts >= timeout_secs )); then
      echo "Timed out after ${timeout_secs}s: $*" >&2
      terminate_tree "$cmd_pid"
      wait "$cmd_pid" 2>/dev/null || true
      return 124
    fi
    sleep 1
  done

  wait "$cmd_pid"
}

retry_lane() {
  local label="$1"
  local timeout_secs="$2"
  shift 2
  local lane_cmd=("$@")
  local status

  echo "Running ${label}..."
  if run_with_timeout "$timeout_secs" "${lane_cmd[@]}"; then
    return 0
  else
    status=$?
  fi

  if [[ "$status" -ne 124 ]]; then
    return "$status"
  fi

  echo "${label} timed out; retrying once with a clean process tree..." >&2
  sleep 1
  run_with_timeout "$timeout_secs" "${lane_cmd[@]}"
}

acquire_lock
trap release_lock EXIT

require_exact_clean_head
tree="$(tree_key)"
source_fingerprint="$(source_test_fingerprint)"
stamp_key="${CACHE_VERSION}-cargo-source-${source_fingerprint}"
stamp_path="${HOOK_CACHE_DIR}/${stamp_key}.ok"

if [[ "${MEERKAT_SKIP_PRE_PUSH_UNIT_CACHE:-0}" != "1" && -f "$stamp_path" ]]; then
  echo "deterministic pre-push source tests already validated for fingerprint ${source_fingerprint}; skipping."
  exit 0
fi

retry_lane \
  "workspace unit build" \
  "$BUILD_TIMEOUT_SECS" \
  "$CARGO" nextest run --workspace --lib --no-run
retry_lane \
  "workspace unit lane" \
  "$UNIT_NEXTEST_TIMEOUT_SECS" \
  "$CARGO" nextest run --workspace --lib --no-fail-fast \
    --show-progress none --status-level none --final-status-level fail
retry_lane \
  "workspace integration build" \
  "$BUILD_TIMEOUT_SECS" \
  "$CARGO" nextest run --workspace --tests --profile fast \
    --no-run -E 'kind(test)'
retry_lane \
  "workspace integration lane" \
  "$INTEGRATION_NEXTEST_TIMEOUT_SECS" \
  "$CARGO" nextest run --workspace --tests --profile fast --no-fail-fast \
    -E 'kind(test)' \
    --show-progress none --status-level none --final-status-level fail
retry_lane \
  "HeadCanonical cold-restart build" \
  "$BUILD_TIMEOUT_SECS" \
  "$CARGO" nextest run -p meerkat-mob --test cold_restart_mob_resume \
    --features test-support --profile fast --no-tests=fail --no-run
retry_lane \
  "HeadCanonical cold-restart lane" \
  "$NEXTEST_TIMEOUT_SECS" \
  "$CARGO" nextest run -p meerkat-mob --test cold_restart_mob_resume \
    --features test-support --profile fast --no-tests=fail \
    --show-progress none --status-level none --final-status-level fail
retry_lane \
  "e2e-fast build" \
  "$BUILD_TIMEOUT_SECS" \
  "$CARGO" nextest run -p meerkat-integration-tests --test e2e_fast_lane \
    --no-run
retry_lane \
  "e2e-fast lane" \
  "$NEXTEST_TIMEOUT_SECS" \
  "$CARGO" nextest run -p meerkat-integration-tests --test e2e_fast_lane \
    --no-fail-fast --show-progress none --status-level none --final-status-level fail

require_exact_clean_head
final_tree="$(tree_key)"
if [[ "$final_tree" != "$tree" ]]; then
  echo "Checked-out HEAD changed during pre-push validation; refusing a stale success stamp." >&2
  exit 1
fi
final_source_fingerprint="$(source_test_fingerprint)"
if [[ "$final_source_fingerprint" != "$source_fingerprint" ]]; then
  echo "Source-test inputs changed during pre-push validation; refusing a stale success stamp." >&2
  exit 1
fi
stamp_tmp="${stamp_path}.tmp.$$"
printf 'tree=%s\nsource_fingerprint=%s\nexcluded=Cargo.lock,MODULE.bazel.lock\nbackend=cargo\nrunners=unit,integration-fast,headcanonical-process-death,e2e-fast\n' \
  "$tree" "$source_fingerprint" > "$stamp_tmp"
mv "$stamp_tmp" "$stamp_path"
