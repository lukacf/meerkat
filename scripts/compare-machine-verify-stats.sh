#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <main-worktree> <feature-worktree>" >&2
  exit 2
fi

main_worktree="$1"
feature_worktree="$2"
parser_root="$(cd "$(dirname "$0")/.." && pwd)"

for worktree in "$main_worktree" "$feature_worktree"; do
  if [[ ! -d "$worktree" ]]; then
    echo "worktree not found: $worktree" >&2
    exit 2
  fi
done

main_tmp="$(mktemp -d)"
feature_tmp="$(mktemp -d)"
trap 'rm -rf "$main_tmp" "$feature_tmp"' EXIT

main_log="$main_tmp/machine-verify.log"
feature_log="$feature_tmp/machine-verify.log"
main_json="$main_tmp/machine-verify-stats.json"
feature_json="$feature_tmp/machine-verify-stats.json"

(
  cd "$main_worktree"
  make machine-verify | tee "$main_log"
)
(
  cd "$parser_root"
  ./scripts/repo-cargo run -p xtask --features machine-authority -- \
    machine-verify-stats --log "$main_log" > "$main_json"
)

(
  cd "$feature_worktree"
  make machine-verify | tee "$feature_log"
)
(
  cd "$parser_root"
  ./scripts/repo-cargo run -p xtask --features machine-authority -- \
    machine-verify-stats --log "$feature_log" > "$feature_json"
)

diff -u "$main_json" "$feature_json"

echo "machine verify stats match"
