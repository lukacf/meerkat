#!/usr/bin/env bash
# usage: restack-pick.sh <worktree> <branch> <range old_base..old_tip> <onto>
set -uo pipefail
wt="$1"; br="$2"; range="$3"; onto="$4"
G() { git -C "$wt" -c core.hooksPath=/dev/null "$@"; }
G cherry-pick --quit 2>/dev/null; G rebase --quit 2>/dev/null; G reset -q --hard
G checkout -q -B "$br" "$onto" || exit 1
for c in $(G rev-list --reverse "$range"); do
  if ! G cherry-pick "$c" >/dev/null 2>&1; then
    cf=$(G diff --name-only --diff-filter=U | tr '\n' ' ')
    [ "$cf" = "CHANGELOG.md " ] || { echo "  NON-CHANGELOG CONFLICT [$cf] at $(G log -1 --format=%s "$c" | cut -c1-50)"; exit 2; }
    (cd "$wt" && python3 /tmp/rb/resolve-changelog-union.py >/dev/null) || exit 3
    G add CHANGELOG.md; GIT_EDITOR=true G cherry-pick --continue >/dev/null 2>&1 || exit 4
  fi
done
echo "  $br -> $(G rev-parse --short=9 HEAD) commits=$(G rev-list --count "$onto..HEAD") generate-check=$(cd "$wt" && node scripts/generate-bazel-rust-builds.mjs --check >/dev/null 2>&1 && echo PASS || echo FAIL) non-changelog=[$(G diff --stat "$onto" HEAD -- . ':!CHANGELOG.md' | tail -1)] markers=$(grep -cE '^(<<<<<<< |=======\s*$|>>>>>>> )' "$wt/CHANGELOG.md")"
