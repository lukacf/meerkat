#!/usr/bin/env bash
# usage: rebase-branch.sh <worktree> <branch>
set -euo pipefail
cd "$1"; b="$2"
git fetch origin --quiet
git checkout -q "$b"; git reset -q --hard "origin/$b"
before_stat=$(git diff --stat "$(git merge-base origin/main HEAD)" HEAD -- . ':!CHANGELOG.md' | tail -1)
if ! git rebase origin/main >/tmp/rb/rebase-$$.log 2>&1; then
  while [ -d .git/rebase-merge ] || git rev-parse -q --verify REBASE_HEAD >/dev/null 2>&1; do
    conflicted=$(git diff --name-only --diff-filter=U)
    if [ "$conflicted" != "CHANGELOG.md" ]; then echo "NON-CHANGELOG CONFLICT in $b: $conflicted"; exit 2; fi
    python3 /tmp/rb/resolve-changelog.py
    git add CHANGELOG.md
    GIT_EDITOR=true git rebase --continue >/tmp/rb/rebase-$$.log 2>&1 || { grep -q "CONFLICT" /tmp/rb/rebase-$$.log || { cat /tmp/rb/rebase-$$.log | tail -5; exit 3; }; }
  done
fi
after_stat=$(git diff --stat origin/main HEAD -- . ':!CHANGELOG.md' | tail -1)
echo "$b rebased -> $(git rev-parse --short=9 HEAD); non-changelog diffstat before: [$before_stat] after: [$after_stat]"
echo "--- new [Unreleased] section (first 12 lines):"; awk '/^## \[Unreleased\]/{f=1} /^## \[0\.8\.33\]/{f=0} f' CHANGELOG.md | head -12
grep -c "gemini-3.8-flash" CHANGELOG.md | sed 's/^/gemini mentions in file: /'
