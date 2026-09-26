#!/usr/bin/env bash
# Contract test for the semver-breaks baseline selection
# (scripts/semver-baseline.sh).
#
# After v0.8.44 was tagged and before crates.io published it, every tree whose
# `## [Unreleased]` notes were not empty failed Release semver readiness: the
# gate measured the workspace version, 0.8.44, against crates.io's newest
# release, 0.8.43, re-reported the 66 breaks the stamped 0.8.44 section
# already declared, and demanded them again under `## [Unreleased]`. Those
# notes follow 0.8.44, so v0.8.44 is their baseline. This test builds scratch
# repositories for each state and asserts which baseline is chosen, or that
# the selection refuses, without touching the network.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON="${PYTHON:-$(command -v python3.11 2>/dev/null || command -v python3)}"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-semver-baseline.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

UNRELEASED_EMPTY=$'## [Unreleased]\n\n'
UNRELEASED_NOTES=$'## [Unreleased]\n\n### Fixed\n\n- A fix after the release.\n\n'
STAMPED_44=$'## [0.8.44] - 2026-09-26\n\n### Breaking\n\n- Something broke.\n\n'
STAMPED_43=$'## [0.8.43] - 2026-09-26\n\n### Added\n\n- A thing.\n\n'

failures=0
fail() {
  echo "FAIL: $*" >&2
  failures=$((failures + 1))
}

# new_repo DIR VERSION CHANGELOG_BODY: a repository whose first commit is the
# given release tree.
new_repo() {
  local dir="$1" version="$2" body="$3"
  mkdir -p "$dir"
  git -C "$dir" init -q -b main
  git -C "$dir" config user.email semver-baseline@example.invalid
  git -C "$dir" config user.name "semver baseline test"
  git -C "$dir" config commit.gpgsign false
  git -C "$dir" config tag.gpgsign false
  printf '[workspace.package]\nversion = "%s"\n' "$version" >"$dir/Cargo.toml"
  printf '# Changelog\n\n%s' "$body" >"$dir/CHANGELOG.md"
  git -C "$dir" add Cargo.toml CHANGELOG.md
  git -C "$dir" commit -q -m "release tree"
}

# commit_changelog DIR BODY: a later commit that only rewrites the notes.
commit_changelog() {
  local dir="$1" body="$2"
  printf '# Changelog\n\n%s' "$body" >"$dir/CHANGELOG.md"
  git -C "$dir" commit -q -am "notes"
}

# select_in DIR PUBLISHED [OVERRIDE]: run the selection in DIR with crates.io
# answering PUBLISHED. Prints "<baseline_version> <post_release>" on success;
# on refusal prints "refused" and the explanation.
select_in() {
  local dir="$1" published="$2" override="${3:-}"
  (
    cd "$dir"
    # shellcheck source=scripts/semver-baseline.sh
    source "$REPO_ROOT/scripts/semver-baseline.sh"
    semver_published_version() { printf '%s\n' "$published"; }
    semver_analyser="$REPO_ROOT/scripts/check_semver_breaks.py"
    semver_changelog="$dir/CHANGELOG.md"
    workspace_version="$(
      "$PYTHON" -c 'import pathlib,tomllib; print(tomllib.loads(pathlib.Path("Cargo.toml").read_text())["workspace"]["package"]["version"])'
    )"
    MEERKAT_SEMVER_BASELINE_VERSION="$override"
    if resolve_semver_baseline 2>"$dir/.selection-stderr"; then
      printf '%s %s\n' "$baseline_version" "$post_release"
    else
      printf 'refused %s\n' "$(tr '\n' ' ' <"$dir/.selection-stderr")"
    fi
  )
}

expect() {
  local name="$1" expected="$2" actual="$3"
  if [[ "$actual" != "$expected" ]]; then
    fail "${name}: expected '${expected}', got '${actual}'"
  fi
}

expect_refusal() {
  local name="$1" needle="$2" actual="$3"
  if [[ "$actual" != refused* || "$actual" != *"$needle"* ]]; then
    fail "${name}: expected a refusal mentioning '${needle}', got '${actual}'"
  fi
}

# 1. The normal release path: the release commit's notes are stamped for the
#    workspace version with an empty stub above, measured against the newest
#    published release. Unchanged by the post-release rule.
repo="$TEST_ROOT/release-tree"
new_repo "$repo" 0.8.44 "${UNRELEASED_EMPTY}${STAMPED_44}${STAMPED_43}"
expect "release tree before publication" "0.8.43 false" "$(select_in "$repo" 0.8.43)"
expect_refusal "release tree after publication" "already the published baseline" \
  "$(select_in "$repo" 0.8.44)"

# 2. The v0.8.44 failure: the release is tagged, HEAD has moved past it, and
#    new notes sit under `## [Unreleased]`. The tag is the baseline whether or
#    not crates.io has caught up.
repo="$TEST_ROOT/post-release"
new_repo "$repo" 0.8.44 "${UNRELEASED_EMPTY}${STAMPED_44}${STAMPED_43}"
git -C "$repo" tag v0.8.44
commit_changelog "$repo" "${UNRELEASED_NOTES}${STAMPED_44}${STAMPED_43}"
expect "post-release tree before publication" "0.8.44 true" "$(select_in "$repo" 0.8.43)"
expect "post-release tree after publication" "0.8.44 true" "$(select_in "$repo" 0.8.44)"
expect "explicit override still wins" "0.8.42 false" "$(select_in "$repo" 0.8.43 0.8.42)"

# 3. A post-release tree with no new notes is still measured as the release:
#    its pending section is the stamped one.
repo="$TEST_ROOT/post-release-no-notes"
new_repo "$repo" 0.8.44 "${UNRELEASED_EMPTY}${STAMPED_44}${STAMPED_43}"
git -C "$repo" tag v0.8.44
printf 'unrelated\n' >"$repo/README.md"
git -C "$repo" add README.md
git -C "$repo" commit -q -m "unrelated"
expect "post-release tree without new notes" "0.8.43 false" "$(select_in "$repo" 0.8.43)"

# 4. At the tagged commit itself nothing has moved past the release, so a
#    malformed release tree with notes left under `## [Unreleased]` keeps the
#    published baseline rather than measuring the tag against itself.
repo="$TEST_ROOT/tagged-with-notes"
new_repo "$repo" 0.8.44 "${UNRELEASED_NOTES}${STAMPED_44}${STAMPED_43}"
git -C "$repo" tag v0.8.44
expect "notes at the tagged commit" "0.8.43 false" "$(select_in "$repo" 0.8.43)"

# 5. Between the release commit and its tag there is no release to measure the
#    new notes against yet: refuse and say so instead of misreporting breaks.
repo="$TEST_ROOT/untagged"
new_repo "$repo" 0.8.44 "${UNRELEASED_EMPTY}${STAMPED_44}${STAMPED_43}"
commit_changelog "$repo" "${UNRELEASED_NOTES}${STAMPED_44}${STAMPED_43}"
expect_refusal "post-release notes before the tag" "is not tagged yet" "$(select_in "$repo" 0.8.43)"

# 6. A tag HEAD does not descend from is not the release these notes follow.
repo="$TEST_ROOT/foreign-tag"
new_repo "$repo" 0.8.44 "${UNRELEASED_EMPTY}${STAMPED_44}${STAMPED_43}"
git -C "$repo" checkout -q -b elsewhere
commit_changelog "$repo" "${UNRELEASED_EMPTY}${STAMPED_44}${STAMPED_43}- elsewhere"$'\n'
git -C "$repo" tag v0.8.44
git -C "$repo" checkout -q main
commit_changelog "$repo" "${UNRELEASED_NOTES}${STAMPED_44}${STAMPED_43}"
expect_refusal "tag off HEAD's history" "is not an ancestor of HEAD" "$(select_in "$repo" 0.8.43)"

# 7. The version bump landed but the notes were never stamped: that is still a
#    release candidate measured against the published release (check_stamped
#    then rejects the unstamped notes).
repo="$TEST_ROOT/bumped-unstamped"
new_repo "$repo" 0.8.44 "${UNRELEASED_NOTES}${STAMPED_43}"
expect "bumped but unstamped notes" "0.8.43 false" "$(select_in "$repo" 0.8.43)"

if [[ "$failures" -ne 0 ]]; then
  echo "semver baseline selection contract: ${failures} failure(s)" >&2
  exit 1
fi
echo "semver baseline selection contract holds"
