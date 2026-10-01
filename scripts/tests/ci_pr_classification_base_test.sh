#!/usr/bin/env bash
# Contract test for scripts/ci-pr-classification-base: a pull_request run is
# classified against the first parent of the checked-out merge commit, never
# the (possibly stale) event payload base.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
resolver="${ROOT}/scripts/ci-pr-classification-base"
repo="$(mktemp -d "${TMPDIR:-/tmp}/ci-pr-base.XXXXXX")"
trap 'rm -rf "$repo"' EXIT

g() { git -C "$repo" -c user.name=Meerkat -c user.email=meerkat@example.invalid "$@"; }
g init -q -b main
g commit --allow-empty -qm base
payload_base="$(g rev-parse HEAD)"
g checkout -qb pr
echo pr > "$repo/pr-only.txt"; g add pr-only.txt; g commit -qm pr
pr_head="$(g rev-parse HEAD)"
g checkout -q main
echo main > "$repo/main-only.txt"; g add main-only.txt; g commit -qm "main advanced after the event was queued"
advanced_main="$(g rev-parse HEAD)"
# GitHub's refs/pull/N/merge: base-branch tip first, PR head second.
g merge -q --no-ff -m merge "$pr_head"

resolved="$(cd "$repo" && "$resolver" pull_request "$payload_base" 2>/dev/null)"
if [[ "$resolved" != "$advanced_main" ]]; then
  echo "pull_request base resolved to ${resolved}, expected the merge commit's first parent ${advanced_main}" >&2
  exit 1
fi
changed="$(g diff --name-only "${resolved}...HEAD")"
if [[ "$changed" != "pr-only.txt" ]]; then
  echo "classification diff includes main's own changes: ${changed}" >&2
  exit 1
fi
stale_changed="$(g diff --name-only "${payload_base}...HEAD" | sort | paste -sd, -)"
if [[ "$stale_changed" != "main-only.txt,pr-only.txt" ]]; then
  echo "fixture no longer reproduces the stale payload-base race: ${stale_changed}" >&2
  exit 1
fi

# push events keep the payload base.
if [[ "$(cd "$repo" && "$resolver" push "$payload_base")" != "$payload_base" ]]; then
  echo "push base must be the payload base" >&2
  exit 1
fi

# A pull_request checkout that is not a merge commit fails closed.
g checkout -q "$pr_head"
if (cd "$repo" && "$resolver" pull_request "$payload_base") >/dev/null 2>&1; then
  echo "a non-merge pull_request checkout must fail closed" >&2
  exit 1
fi

echo "ci-pr-classification-base contracts hold"
