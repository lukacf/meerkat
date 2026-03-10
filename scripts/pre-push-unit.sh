#!/usr/bin/env bash
# Pre-push unit test gate:
# - skips reruns for the same committed tree
# - serializes runs so repeated pushes don't fight each other
# - retries nextest once if discovery hangs
set -euo pipefail

CACHE_VERSION="v2"
NEXTEST_TIMEOUT_SECS="${MEERKAT_PRE_PUSH_NEXTEST_TIMEOUT_SECS:-120}"
LOCK_WAIT_SECS="${MEERKAT_PRE_PUSH_UNIT_LOCK_WAIT_SECS:-180}"
GIT_DIR_PATH="$(git rev-parse --git-dir)"
HOOK_CACHE_ROOT="${GIT_DIR_PATH}/meerkat-hook-cache"
HOOK_CACHE_DIR="${HOOK_CACHE_ROOT}/unit"
LOCK_DIR="${HOOK_CACHE_ROOT}/unit.lock"
PID_FILE="${LOCK_DIR}/pid"

mkdir -p "$HOOK_CACHE_DIR"

tree_key() {
  if git rev-parse --verify HEAD^{tree} >/dev/null 2>&1; then
    git rev-parse HEAD^{tree}
  else
    git write-tree
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

retry_nextest() {
  local nextest_cmd=(
    cargo nextest run --workspace --lib
    --show-progress none
    --status-level none
    --final-status-level fail
  )

  echo "Running workspace unit tests with nextest..."
  if run_with_timeout "$NEXTEST_TIMEOUT_SECS" "${nextest_cmd[@]}"; then
    return 0
  fi

  local status=$?
  if [[ "$status" -ne 124 ]]; then
    return "$status"
  fi

  echo "nextest timed out; retrying once with a clean process tree..." >&2
  sleep 1
  run_with_timeout "$NEXTEST_TIMEOUT_SECS" "${nextest_cmd[@]}"
}

tree="$(tree_key)"
stamp_key="${CACHE_VERSION}-${tree}"
stamp_path="${HOOK_CACHE_DIR}/${stamp_key}.ok"

if [[ "${MEERKAT_SKIP_PRE_PUSH_UNIT_CACHE:-0}" != "1" && -f "$stamp_path" ]]; then
  echo "cargo unit already validated for tree ${tree}; skipping."
  exit 0
fi

acquire_lock
trap release_lock EXIT

if [[ "${MEERKAT_SKIP_PRE_PUSH_UNIT_CACHE:-0}" != "1" && -f "$stamp_path" ]]; then
  echo "cargo unit already validated for tree ${tree}; skipping."
  exit 0
fi

retry_nextest
printf 'tree=%s\nrunner=nextest\n' "$tree" > "$stamp_path"
