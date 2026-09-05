#!/usr/bin/env bash
# usage: restack.sh <worktree> <branch> <old_base> <old_tip> <onto>
set -uo pipefail
wt="$1"; br="$2"; old_base="$3"; old_tip="$4"; onto="$5"
G="git -C $wt -c core.hooksPath=/dev/null"
$G cherry-pick --quit 2>/dev/null; $G rebase --quit 2>/dev/null; $G reset -q --hard
$G checkout -q -B "$br" "$onto" || exit 1
for c in $($G rev-list --reverse "$old_base..$old_tip"); do
  if $G cherry-pick "$c" >/dev/null 2>&1; then
    (cd "$wt" && python3 /tmp/rb/refit-changelog2.py "$old_base" "$c" "$onto") || exit 3
    $G add CHANGELOG.md; $G diff --cached --quiet || $G commit -q --amend --no-edit
  else
    conflicts=$($G diff --name-only --diff-filter=U | tr '\n' ' ')
    [ "$conflicts" = "CHANGELOG.md " ] || { echo "  NON-CHANGELOG CONFLICT [$conflicts] at $($G log -1 --format=%s $c | cut -c1-60)"; exit 2; }
    (cd "$wt" && python3 /tmp/rb/refit-changelog2.py "$old_base" "$c" "$onto") || exit 3
    $G add CHANGELOG.md
    if $G diff --cached --quiet && $G diff --quiet; then $G cherry-pick --skip >/dev/null 2>&1; echo "  skipped now-empty commit: $($G log -1 --format=%s $c | cut -c1-70)"
    else GIT_EDITOR=true $G cherry-pick --continue >/dev/null 2>&1 || { echo "  continue failed at $c"; exit 4; }; fi
  fi
done
main=$(git -C /home/luka/src/meerkat-mobkit rev-parse origin/main)
echo "  $br -> $($G rev-parse --short=8 HEAD) commits=$($G rev-list --count $onto..HEAD)/$($G rev-list --count $old_base..$old_tip) unreleased=$(awk '/^## \[Unreleased\]/{f=1} /^## \[0\.8\.31\]/{f=0} f' $wt/CHANGELOG.md | grep -c '^- ') tail-identical-to-main=$(diff <(awk '/^## \[0\.8\.31\]/{f=1} f' $wt/CHANGELOG.md) <(git -C /home/luka/src/meerkat-mobkit show $main:CHANGELOG.md | awk '/^## \[0\.8\.31\]/{f=1} f') >/dev/null && echo yes || echo NO)"
echo "  non-changelog before [$($G diff --stat $old_base $old_tip -- . ':!CHANGELOG.md' | tail -1)] after [$($G diff --stat $onto HEAD -- . ':!CHANGELOG.md' | tail -1)]"
