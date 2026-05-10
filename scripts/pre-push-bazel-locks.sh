#!/usr/bin/env bash
# pre-push-bazel-locks.sh — fail closed when Bazel BUILD files or the
# module lockfile are stale, so CI doesn't reject the push for the same
# reason after a long round-trip.
#
# Two gates:
#   1. node scripts/generate-bazel-rust-builds.mjs --check
#      Verifies the generated Bazel BUILD files match the workspace's
#      Cargo metadata. Always runs (no `bb` CLI dependency).
#   2. bb mod deps --lockfile_mode=error
#      Verifies MODULE.bazel.lock matches MODULE.bazel. Requires the
#      pinned `bb` CLI. Skipped (with a clear note) when `bb` is not
#      available on this developer's machine — CI will catch staleness
#      regardless.
#
# Exit 0 = locks fresh (or skipped honestly), exit 1 = stale locks.

set -euo pipefail

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

# ---- Gate 2: MODULE.bazel.lock freshness ------------------------------------

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

if [[ -z "${bb_bin}" ]]; then
  printf '%bbb CLI not installed locally; skipping MODULE.bazel.lock check%b\n' \
    "${YELLOW}" "${NC}"
  printf '%b(CI runs this check too — install with `make buildbuddy-install` to catch it locally.)%b\n' \
    "${YELLOW}" "${NC}"
else
  lock_log="${TMPDIR:-/tmp}/meerkat-pre-push-bazel-lock.$$"
  if "${bb_bin}" mod deps --lockfile_mode=error >"${lock_log}" 2>&1; then
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
