#!/usr/bin/env bash
# Pre-push deterministic test gate:
# - validates the exact detached pushed tree selected by pre-push-dispatch.sh
# - reuses source-test evidence across root lockfile-only commits
# - serializes identical source evidence while isolated worktree lanes run in parallel
# - retries nextest once if discovery hangs
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="${ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"
GIT_BIN="${GIT_BIN:-git}"
RELEASE_PROJECTION_ONLY="${RELEASE_PROJECTION_ONLY:-$SCRIPT_DIR/release-projection-only.mjs}"
PRE_PUSH_HARNESS_ONLY="${PRE_PUSH_HARNESS_ONLY:-$SCRIPT_DIR/pre-push-harness-only.sh}"

CACHE_VERSION="v10"
NEXTEST_TIMEOUT_SECS="${MEERKAT_PRE_PUSH_NEXTEST_TIMEOUT_SECS:-300}"
BUILD_TIMEOUT_SECS="${MEERKAT_PRE_PUSH_BUILD_TIMEOUT_SECS:-${MEERKAT_PRE_PUSH_NARROW_BUILD_TIMEOUT_SECS:-900}}"
# Generated machine state is intentionally broad enough that some integration
# fixtures exceed libtest's platform-default thread stack. Keep the detached
# Cargo/nextest authority lane aligned with the workspace-wide verified floor.
export RUST_MIN_STACK="${RUST_MIN_STACK:-33554432}"
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
LOCK_DIR=""
PID_FILE=""
lock_held=0

mkdir -p "$HOOK_CACHE_DIR"

tree_key() {
  if "$GIT_BIN" rev-parse --verify 'HEAD^{tree}' >/dev/null 2>&1; then
    "$GIT_BIN" rev-parse 'HEAD^{tree}'
  else
    "$GIT_BIN" write-tree
  fi
}

source_test_fingerprint() {
  local revision="${1:-HEAD}"
  local tree_record tree_path
  # Root dependency locks are validated by the preceding Cargo and Bazel lock
  # gates, while CI remains authoritative for advisories. The always-run
  # changed-crate Clippy gate also compiles their resolved workspace graph.
  # Excluding only these two generated lock artifacts lets that narrow
  # dependency evidence reuse source-test results; every other tracked byte
  # remains fail-closed because tests may read fixtures or configuration with
  # arbitrary extensions. The current lock graph is compiled, not re-tested:
  # reused test evidence was executed against the prior root lock graph.
  "$GIT_BIN" ls-tree -rz --full-tree "$revision" |
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
  lock_held=1
}

release_lock() {
  if [[ "$lock_held" -eq 1 ]]; then
    rm -rf "$LOCK_DIR"
    lock_held=0
  fi
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

NEXTEST_REUSE_DIR=""

release_resources() {
  if [[ -n "$NEXTEST_REUSE_DIR" && -d "$NEXTEST_REUSE_DIR" ]]; then
    rm -rf -- "$NEXTEST_REUSE_DIR"
  fi
  release_lock
}

trap release_resources EXIT
NEXTEST_REUSE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-pre-push-nextest.XXXXXX")"

require_exact_clean_head
tree="$(tree_key)"
source_fingerprint="$(source_test_fingerprint)"
stamp_key="${CACHE_VERSION}-cargo-source-${source_fingerprint}"
stamp_path="${HOOK_CACHE_DIR}/${stamp_key}.ok"
LOCK_DIR="${HOOK_CACHE_ROOT}/deterministic-locks/${stamp_key}.lock"
PID_FILE="${LOCK_DIR}/pid"

if [[ "${MEERKAT_SKIP_PRE_PUSH_UNIT_CACHE:-0}" != "1" && -f "$stamp_path" ]]; then
  echo "reusing deterministic source-test evidence for fingerprint ${source_fingerprint}."
  echo "Current root lock graph was compiled by pre-push Clippy but is not re-tested locally; CI is authoritative."
  exit 0
fi

# A cargo-release projection rewrites version-bearing manifests, generated
# metadata, SDK/schema mirrors, and docs without changing executable source or
# dependency selection. The fail-closed classifier proves that boundary. Reuse
# the exact base fingerprint only when its broad source evidence is already
# present, then record the candidate fingerprint as a derived evidence alias so
# a later lock-only repair does not rebuild the world either.
if [[ "${MEERKAT_SKIP_PRE_PUSH_UNIT_CACHE:-0}" != "1" && \
      -n "${PRE_COMMIT_FROM_REF:-}" && -n "${PRE_COMMIT_TO_REF:-}" ]]; then
  if "$RELEASE_PROJECTION_ONLY" \
      --base "$PRE_COMMIT_FROM_REF" --head "$PRE_COMMIT_TO_REF"; then
    base_source_fingerprint="$(source_test_fingerprint "$PRE_COMMIT_FROM_REF")"
    base_stamp_path="${HOOK_CACHE_DIR}/${CACHE_VERSION}-cargo-source-${base_source_fingerprint}.ok"
    if [[ -f "$base_stamp_path" ]]; then
      stamp_tmp="${stamp_path}.tmp.$$"
      printf 'tree=%s\nsource_fingerprint=%s\nreuse_parent_fingerprint=%s\nclassifier=release-projection-only\nbackend=cargo\nrunners=reused-parent-source-evidence\n' \
        "$tree" "$source_fingerprint" "$base_source_fingerprint" > "$stamp_tmp"
      mv "$stamp_tmp" "$stamp_path"
      echo "Release projection only; reusing deterministic parent source-test evidence ${base_source_fingerprint}."
      exit 0
    fi
    echo "Release projection parent has no reusable source-test evidence; running the deterministic gate."
  else
    release_projection_status=$?
    if [[ "$release_projection_status" -ne 1 ]]; then
      echo "release projection classification failed with status ${release_projection_status}" >&2
      exit "$release_projection_status"
    fi
  fi
fi

# Hook-harness edits are validated by the dedicated pre-push dispatcher,
# machine, unit-status, release-projection, lock, and enumeration contract
# suites that run before this broad Cargo lane. They do not change Rust test
# inputs. Reuse parent Rust evidence only for additions/modifications inside
# that closed harness path set; deletions and every other path fail closed.
if [[ "${MEERKAT_SKIP_PRE_PUSH_UNIT_CACHE:-0}" != "1" && \
      -n "${PRE_COMMIT_FROM_REF:-}" && -n "${PRE_COMMIT_TO_REF:-}" ]]; then
  if "$PRE_PUSH_HARNESS_ONLY" \
      --base "$PRE_COMMIT_FROM_REF" --head "$PRE_COMMIT_TO_REF"; then
    base_source_fingerprint="$(source_test_fingerprint "$PRE_COMMIT_FROM_REF")"
    base_stamp_path="${HOOK_CACHE_DIR}/${CACHE_VERSION}-cargo-source-${base_source_fingerprint}.ok"
    if [[ -f "$base_stamp_path" ]]; then
      stamp_tmp="${stamp_path}.tmp.$$"
      printf 'tree=%s\nsource_fingerprint=%s\nreuse_parent_fingerprint=%s\nclassifier=pre-push-harness-only\nbackend=cargo\nrunners=reused-parent-source-evidence\n' \
        "$tree" "$source_fingerprint" "$base_source_fingerprint" > "$stamp_tmp"
      mv "$stamp_tmp" "$stamp_path"
      echo "Pre-push harness only; reusing deterministic parent Rust-test evidence ${base_source_fingerprint}."
      exit 0
    fi
    echo "Pre-push harness parent has no reusable Rust-test evidence; running the deterministic gate."
  else
    harness_classifier_status=$?
    if [[ "$harness_classifier_status" -ne 1 ]]; then
      echo "pre-push harness classification failed with status ${harness_classifier_status}" >&2
      exit "$harness_classifier_status"
    fi
  fi
fi

mkdir -p "$(dirname "$LOCK_DIR")"
acquire_lock

# A peer validating the same source fingerprint may have completed while this
# process waited. Reuse its evidence instead of rebuilding the same graph.
if [[ "${MEERKAT_SKIP_PRE_PUSH_UNIT_CACHE:-0}" != "1" && -f "$stamp_path" ]]; then
  echo "reusing deterministic source-test evidence for fingerprint ${source_fingerprint}."
  echo "Current root lock graph was compiled by pre-push Clippy but is not re-tested locally; CI is authoritative."
  exit 0
fi

workspace_cargo_metadata="${NEXTEST_REUSE_DIR}/workspace-cargo-metadata.json"
workspace_binaries_metadata="${NEXTEST_REUSE_DIR}/workspace-binaries-metadata.json"
headcanonical_binaries_metadata="${NEXTEST_REUSE_DIR}/headcanonical-binaries-metadata.json"

write_cargo_metadata() {
  "$CARGO" metadata --format-version 1 >"$workspace_cargo_metadata"
}

build_workspace_inventory() {
  "$CARGO" nextest list --workspace \
    --list-type binaries-only --message-format json \
    -E 'kind(lib) or kind(test)' >"$workspace_binaries_metadata"
}

build_headcanonical_inventory() {
  "$CARGO" nextest list -p meerkat-mob --test cold_restart_mob_resume \
    --features test-support --profile fast \
    --list-type binaries-only --message-format json \
    >"$headcanonical_binaries_metadata"
}

write_cargo_metadata
retry_lane \
  "workspace default-feature test build" \
  "$BUILD_TIMEOUT_SECS" \
  build_workspace_inventory
retry_lane \
  "workspace unit lane" \
  "$UNIT_NEXTEST_TIMEOUT_SECS" \
  "$CARGO" nextest run \
    --cargo-metadata "$workspace_cargo_metadata" \
    --binaries-metadata "$workspace_binaries_metadata" \
    -E 'kind(lib)' --no-tests=fail --no-fail-fast \
    --show-progress none --status-level none --final-status-level fail
retry_lane \
  "workspace integration lane" \
  "$INTEGRATION_NEXTEST_TIMEOUT_SECS" \
  "$CARGO" nextest run \
    --cargo-metadata "$workspace_cargo_metadata" \
    --binaries-metadata "$workspace_binaries_metadata" \
    --profile fast -E 'kind(test)' --no-tests=fail --no-fail-fast \
    --show-progress none --status-level none --final-status-level fail
retry_lane \
  "HeadCanonical cold-restart build" \
  "$BUILD_TIMEOUT_SECS" \
  build_headcanonical_inventory
retry_lane \
  "HeadCanonical cold-restart lane" \
  "$NEXTEST_TIMEOUT_SECS" \
  "$CARGO" nextest run \
    --cargo-metadata "$workspace_cargo_metadata" \
    --binaries-metadata "$headcanonical_binaries_metadata" \
    --profile fast -E 'binary(cold_restart_mob_resume)' --no-tests=fail \
    --show-progress none --status-level none --final-status-level fail
retry_lane \
  "e2e-fast lane" \
  "$NEXTEST_TIMEOUT_SECS" \
  "$CARGO" nextest run \
    --cargo-metadata "$workspace_cargo_metadata" \
    --binaries-metadata "$workspace_binaries_metadata" \
    -E 'binary(e2e_fast_lane)' --no-tests=fail --no-fail-fast \
    --show-progress none --status-level none --final-status-level fail

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
printf 'tree=%s\nsource_fingerprint=%s\nexcluded=Cargo.lock,MODULE.bazel.lock\nreuse_boundary=current-lock-graph-compiled-not-retested\nbackend=cargo\nrunners=unit,integration-fast,headcanonical-process-death,e2e-fast\n' \
  "$tree" "$source_fingerprint" > "$stamp_tmp"
mv "$stamp_tmp" "$stamp_path"
