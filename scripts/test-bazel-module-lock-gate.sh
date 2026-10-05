#!/usr/bin/env bash
# Contract test for the offline MODULE.bazel.lock freshness gate.
#
# Reproduces the 0.8.22 shape: Cargo.lock changed (a one-line heal) without a
# module-lock refresh, so every BuildBuddy release-binary lane refused with
# "MODULE.bazel.lock is no longer up-to-date" after the tag already existed.
# The recorded input hashes make that decidable offline, with no `bb` CLI.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON="$("$(dirname "${BASH_SOURCE[0]}")/require-python" 3.11 "$(basename "${BASH_SOURCE[0]}")")" || exit 1
CHECKER="${REPO_ROOT}/scripts/check_bazel_module_lock_inputs.py"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-bazel-module-lock.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

MUTATOR="${TEST_ROOT}/mutate_recorded_input.py"
cat > "$MUTATOR" <<'PYEOF'
"""Rewrite one recorded Cargo.lock extension input: `rehash` it or drop it."""

import json
import pathlib
import sys

source, destination, mode = sys.argv[1], sys.argv[2], sys.argv[3]
lock = json.loads(pathlib.Path(source).read_text())
mutated = False
for extension in lock["moduleExtensions"].values():
    for scope in extension.values():
        if not isinstance(scope, dict) or mutated:
            continue
        recorded = scope.get("recordedInputs")
        if not isinstance(recorded, list):
            continue
        for index, entry in enumerate(recorded):
            if not isinstance(entry, str) or not entry.startswith("FILE:@@//Cargo.lock "):
                continue
            if mode == "rehash":
                recorded[index] = "FILE:@@//Cargo.lock " + "0" * 64
            else:
                del recorded[index]
            mutated = True
            break
if not mutated:
    raise SystemExit("MODULE.bazel.lock records no Cargo.lock input to mutate")
pathlib.Path(destination).write_text(json.dumps(lock, indent=2))
PYEOF

run_checker() {
  local module_lock="$1"
  local output_path="$2"
  set +e
  "$PYTHON" "$CHECKER" "$REPO_ROOT" "$module_lock" >"$output_path" 2>&1
  local status=$?
  set -e
  printf '%s' "$status"
}

fail() {
  echo "bazel module lock gate contract violated: $1" >&2
  shift
  for extra in "$@"; do
    echo "  ${extra}" >&2
  done
  exit 1
}

# 1. The committed module lock must match the committed workspace files.
status="$(run_checker "${REPO_ROOT}/MODULE.bazel.lock" "${TEST_ROOT}/committed.log")"
if [[ "$status" -ne 0 ]]; then
  fail "the committed MODULE.bazel.lock is reported stale" \
    "$(cat "${TEST_ROOT}/committed.log")"
fi

# 2. Stale recorded Cargo.lock hash: what a lock heal without a module-lock
#    refresh leaves behind.
stale_lock="${TEST_ROOT}/stale-MODULE.bazel.lock"
"$PYTHON" "$MUTATOR" "${REPO_ROOT}/MODULE.bazel.lock" "$stale_lock" rehash
status="$(run_checker "$stale_lock" "${TEST_ROOT}/stale.log")"
if [[ "$status" -eq 0 ]]; then
  fail "a stale recorded Cargo.lock hash was accepted"
fi
if ! grep -q "Cargo.lock: recorded 000000000000, on disk" "${TEST_ROOT}/stale.log"; then
  fail "the stale-hash failure does not name Cargo.lock and both digests" \
    "$(cat "${TEST_ROOT}/stale.log")"
fi

# 3. An unrecorded workspace input: what adding a crate without a module-lock
#    refresh leaves behind.
unrecorded_lock="${TEST_ROOT}/unrecorded-MODULE.bazel.lock"
"$PYTHON" "$MUTATOR" "${REPO_ROOT}/MODULE.bazel.lock" "$unrecorded_lock" drop
status="$(run_checker "$unrecorded_lock" "${TEST_ROOT}/unrecorded.log")"
if [[ "$status" -eq 0 ]]; then
  fail "an unrecorded workspace input was accepted"
fi
if ! grep -q "Cargo.lock: workspace file is not recorded as an extension input" \
  "${TEST_ROOT}/unrecorded.log"; then
  fail "the unrecorded-input failure does not name the missing file" \
    "$(cat "${TEST_ROOT}/unrecorded.log")"
fi

echo "bazel module lock gate contract holds"
