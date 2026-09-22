#!/usr/bin/env bash
# Lock consistency gate, aimed at merge commits.
#
# A release branch is always self-consistent: whatever cargo resolved there was
# written by cargo. The dangling reference is manufactured by the MERGE, where
# git resolves a shared [[package]] block textually and leaves a branch-only
# crate pointing at the version it was locked against. No conflict marker is
# produced, and every cargo mode except --locked silently reresolves instead of
# refusing, so tests and acceptance boards stay green while `cargo publish`
# (which is --locked) dies seconds in.
#
# Two phases:
#   1. scripts/check_cargo_lock_consistency.py - offline structural read that
#      names the exact dangling reference.
#   2. cargo metadata --locked - the authority cargo publish itself applies.
#
# Usage: verify-lock-consistency.sh [ROOT_DIR]
# Exit 0 = the lock resolves under --locked, exit 1 = it does not.

set -euo pipefail

SCRIPT_REPO="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="${1:-$SCRIPT_REPO}"
PYTHON="${PYTHON:-$(command -v python3.11 2>/dev/null || command -v python3)}"
CARGO="${CARGO:-$SCRIPT_REPO/scripts/repo-cargo}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'

remediation() {
  printf '\n%bRepair (all three steps; the second and third are what 0.8.22 missed):%b\n' \
    "${YELLOW}" "${NC}"
  printf '  1. ./scripts/repo-cargo metadata --format-version 1 >/dev/null  # reresolves Cargo.lock in place\n'
  printf '  2. git add Cargo.lock\n'
  printf '  3. make buildbuddy-lock-update && git add MODULE.bazel.lock     # Cargo.lock is a crate_universe input\n\n'
}

# Merge context is reported, not required: squash merges mean main may never
# carry a merge commit, and then the pull-request merge ref is the only place
# this class is observable at all.
if git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
  head_sha="$(git -C "$ROOT" rev-parse HEAD)"
  read -r -a head_lineage <<< "$(git -C "$ROOT" rev-list --parents -n 1 HEAD)"
  parent_count=$(( ${#head_lineage[@]} - 1 ))
  is_shallow="$(git -C "$ROOT" rev-parse --is-shallow-repository 2>/dev/null || echo unknown)"
  ci_pull_request=0
  if [[ "${GITHUB_ACTIONS:-}" == "true" && "${GITHUB_EVENT_NAME:-}" == "pull_request" ]]; then
    ci_pull_request=1
  fi

  if (( parent_count >= 2 )); then
    printf 'Lock gate context: %s is a merge commit of %d parents (%s)\n' \
      "${head_sha:0:12}" "$parent_count" "${head_lineage[*]:1}"
  elif (( parent_count == 0 )) && [[ "$is_shallow" == "true" ]]; then
    # A depth-1 clone grafts HEAD to have no parents at all, so merge topology
    # is not observable rather than absent.
    printf 'Lock gate context: %s parents are not fetched (shallow clone)\n' \
      "${head_sha:0:12}"
    if (( ci_pull_request == 1 )); then
      printf '%bA pull_request run validates refs/pull/N/merge, whose parents this%b\n' \
        "${RED}" "${NC}"
      printf '%bshallow checkout hides.%b Check out with fetch-depth >= 2 so the merge\n' \
        "${RED}" "${NC}"
      printf 'context this gate reports is real.\n'
      exit 1
    fi
  else
    printf 'Lock gate context: %s is a single-parent commit\n' "${head_sha:0:12}"
    if (( ci_pull_request == 1 )); then
      printf '%bA pull_request run must validate the merge preview, not the branch head.%b\n' \
        "${RED}" "${NC}"
      printf 'Check out refs/pull/N/merge (actions/checkout default) so the merged\n'
      printf 'Cargo.lock is the lock this gate resolves.\n'
      exit 1
    fi
  fi
fi

failed=0

if "$PYTHON" "$SCRIPT_REPO/scripts/check_cargo_lock_consistency.py" "$ROOT/Cargo.lock"; then
  printf '%bCargo.lock has no dangling package references%b\n' "${GREEN}" "${NC}"
else
  printf '%bCargo.lock failed the structural consistency read (detail above).%b\n' \
    "${RED}" "${NC}"
  printf 'A reference to a version no [[package]] block provides is the textual-merge\n'
  printf 'artifact class: cargo publish resolves --locked and refuses it.\n'
  failed=1
fi

if "$CARGO" metadata --format-version 1 --locked --manifest-path "$ROOT/Cargo.toml" >/dev/null; then
  printf '%bCargo.lock resolves under --locked%b\n' "${GREEN}" "${NC}"
else
  printf '%bCargo.lock does not resolve under --locked.%b\n' "${RED}" "${NC}"
  printf 'Every other cargo mode heals this silently; cargo publish does not.\n'
  failed=1
fi

if (( failed != 0 )); then
  remediation
fi

if ! CARGO="$CARGO" "$PYTHON" "$SCRIPT_REPO/scripts/example-locks.py" check --root "$ROOT"; then
  failed=1
fi

exit "$failed"
