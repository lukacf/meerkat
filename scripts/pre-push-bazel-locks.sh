#!/usr/bin/env bash
# pre-push-bazel-locks.sh — fail closed when Bazel BUILD files or the
# module lockfile are stale, so CI doesn't reject the push for the same
# reason after a long round-trip.
#
# Four gates:
#   1. node scripts/generate-bazel-rust-builds.mjs --check
#      Verifies the generated Bazel BUILD files match the workspace's
#      Cargo metadata. Always runs (no `bb` CLI dependency).
#   2. scripts/check-bazel-path-patch-runfiles.py
#      Verifies every in-tree [patch] path dependency crosses its Bazel package
#      boundary into workspace_runfiles for nested Cargo builds.
#   3. scripts/check_bazel_module_lock_inputs.py
#      Verifies MODULE.bazel.lock still matches the workspace files it
#      recorded as crate_universe extension inputs (Cargo.lock and every
#      member manifest). Offline and always runs: this is the class that
#      broke every BuildBuddy release-binary lane in 0.8.22, when a
#      one-line Cargo.lock heal landed without a module-lock refresh.
#   4. bb mod deps --lockfile_mode=error
#      Full authority over MODULE.bazel.lock vs MODULE.bazel, including
#      registry inputs gate 3 cannot see. Requires the pinned `bb` CLI.
#      Skipped (with a clear note) when `bb` is not available, unless
#      --require-bb is passed (the release preflight does).
#
# Usage: pre-push-bazel-locks.sh [--require-bb]
# Exit 0 = locks fresh (or gate 4 skipped honestly), exit 1 = stale locks.

set -euo pipefail

require_bb=0
for arg in "$@"; do
  case "$arg" in
    --require-bb)
      require_bb=1
      ;;
    *)
      echo "usage: pre-push-bazel-locks.sh [--require-bb]" >&2
      exit 2
      ;;
  esac
done

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'

ROOT="$(git rev-parse --show-toplevel)"
cd "${ROOT}"

failed=0

# ---- Gate 1: generated Bazel BUILD freshness --------------------------------

generate_log="${TMPDIR:-/tmp}/meerkat-pre-push-bazel-generate.$$"
trap 'rm -f "${generate_log}"' EXIT

if node scripts/generate-bazel-rust-builds.mjs --check >"${generate_log}" 2>&1; then
  printf '%bGenerated Bazel BUILD files are up to date%b\n' "${GREEN}" "${NC}"
else
  printf '%bGenerated Bazel BUILD files are stale.%b\n' "${RED}" "${NC}"
  cat "${generate_log}"
  printf '\n%bRefresh with:%b  make buildbuddy-generate\n' "${YELLOW}" "${NC}"
  printf '%bThen stage the diff and retry the push.%b\n\n' "${YELLOW}" "${NC}"
  failed=1
fi

# ---- Gate 2: Cargo path patches included in Bazel runfiles ------------------

PYTHON="${PYTHON:-$(command -v python3.11 2>/dev/null || command -v python3)}"

if "${PYTHON}" scripts/check-bazel-path-patch-runfiles.py "${ROOT}"; then
  printf '%bCargo path patches are present in Bazel workspace runfiles%b\n' "${GREEN}" "${NC}"
else
  printf '%bCargo path patches are missing from Bazel workspace runfiles.%b\n\n' \
    "${RED}" "${NC}"
  failed=1
fi

# ---- Gate 3: recorded workspace inputs vs on-disk content -------------------

if "${PYTHON}" scripts/check_bazel_module_lock_inputs.py "${ROOT}"; then
  printf '%bMODULE.bazel.lock matches its recorded workspace inputs%b\n' "${GREEN}" "${NC}"
else
  printf '%bMODULE.bazel.lock no longer matches the workspace files it recorded.%b\n' \
    "${RED}" "${NC}"
  printf '%bEvery BuildBuddy lane reads this lock in error mode; it fails there instead.%b\n\n' \
    "${YELLOW}" "${NC}"
  failed=1
fi

# ---- Gate 4: MODULE.bazel.lock freshness ------------------------------------

# Locate the pinned `bb` CLI the same way scripts/buildbuddy-doctor does.
bb_bin=""
if [[ -n "${BUILDBUDDY_BB:-}" && -x "${BUILDBUDDY_BB}" ]]; then
  bb_bin="${BUILDBUDDY_BB}"
elif command -v bb >/dev/null 2>&1; then
  bb_bin="$(command -v bb)"
elif [[ -x "${XDG_CACHE_HOME:-${HOME}/.cache}/meerkat/buildbuddy-cli/5.0.350/bin/bb" ]]; then
  bb_bin="${XDG_CACHE_HOME:-${HOME}/.cache}/meerkat/buildbuddy-cli/5.0.350/bin/bb"
elif [[ -x /tmp/buildbuddy-poc/bin/bb ]]; then
  bb_bin="/tmp/buildbuddy-poc/bin/bb"
fi

if [[ -z "${bb_bin}" && "${require_bb}" -eq 1 ]]; then
  printf '%bbb CLI is required for the authoritative MODULE.bazel.lock check.%b\n' \
    "${RED}" "${NC}"
  printf '%bInstall it with:%b  make buildbuddy-install\n' "${YELLOW}" "${NC}"
  printf '%bThe release path may not skip this: a stale module lock fails every%b\n' \
    "${YELLOW}" "${NC}"
  printf '%bBuildBuddy release-binary lane after the tag already exists.%b\n\n' \
    "${YELLOW}" "${NC}"
  failed=1
elif [[ -z "${bb_bin}" ]]; then
  printf '%bbb CLI not installed locally; skipping full lockfile_mode=error check%b\n' \
    "${YELLOW}" "${NC}"
  printf '%b(Gate 3 above still covers the workspace-input class offline. Install%b\n' \
    "${YELLOW}" "${NC}"
  printf '%bwith `make buildbuddy-install`, or run `make verify-bazel-locks-strict`.)%b\n' \
    "${YELLOW}" "${NC}"
else
  lock_log="${TMPDIR:-/tmp}/meerkat-pre-push-bazel-lock.$$"
  bb_startup_args=()
  bb_has_startup_args=0
  if [[ -n "${MEERKAT_PRE_PUSH_BAZEL_OUTPUT_BASE:-}" ]]; then
    mkdir -p "$(dirname "${MEERKAT_PRE_PUSH_BAZEL_OUTPUT_BASE}")"
    bb_startup_args+=(
      "--output_base=${MEERKAT_PRE_PUSH_BAZEL_OUTPUT_BASE}"
      "--max_idle_secs=${MEERKAT_PRE_PUSH_BAZEL_MAX_IDLE_SECS:-600}"
    )
    bb_has_startup_args=1
  fi
  lock_ok=0
  if [[ "${bb_has_startup_args}" -eq 1 ]]; then
    "${bb_bin}" "${bb_startup_args[@]}" mod deps --lockfile_mode=error >"${lock_log}" 2>&1 && lock_ok=1
  else
    "${bb_bin}" mod deps --lockfile_mode=error >"${lock_log}" 2>&1 && lock_ok=1
  fi
  if [[ "${lock_ok}" -eq 1 ]]; then
    printf '%bBazel module lockfile is up to date%b\n' "${GREEN}" "${NC}"
  else
    printf '%bMODULE.bazel.lock is stale.%b\n' "${RED}" "${NC}"
    tail -40 "${lock_log}"
    printf '\n%bRefresh with:%b  make buildbuddy-lock-update\n' "${YELLOW}" "${NC}"
    printf '%bThen stage MODULE.bazel.lock and retry the push.%b\n\n' "${YELLOW}" "${NC}"
    failed=1
  fi
  rm -f "${lock_log}"
fi

exit "${failed}"
