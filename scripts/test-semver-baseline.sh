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
# repositories (with a real `origin` where the case needs one) for each state
# and asserts which baseline is chosen, or that the selection refuses and
# says why, without touching the network. The end-to-end cases then feed the
# analyser the report a measurement against the SELECTED baseline would
# produce, so they fail if selection regresses, not only if the analyser does.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON="${PYTHON:-$(command -v python3.11 2>/dev/null || command -v python3)}"
ANALYSER="$REPO_ROOT/scripts/check_semver_breaks.py"
FIXTURES="$REPO_ROOT/scripts/fixtures/semver-breaks"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-semver-baseline.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

UNRELEASED_EMPTY=$'## [Unreleased]\n\n'
UNRELEASED_NOTES=$'## [Unreleased]\n\n### Fixed\n\n- A fix after the release.\n\n'
STAMPED_44=$'## [0.8.44] - 2026-09-26\n\n### Breaking\n\n- Something broke.\n\n'
STAMPED_43=$'## [0.8.43] - 2026-09-26\n\n### Added\n\n- A thing.\n\n'
RELEASE_TREE="${UNRELEASED_EMPTY}${STAMPED_44}${STAMPED_43}"
POST_RELEASE="${UNRELEASED_NOTES}${STAMPED_44}${STAMPED_43}"

failures=0
fail() {
  echo "FAIL: $*" >&2
  failures=$((failures + 1))
}

git_quiet() {
  git -c init.defaultBranch=main -c advice.detachedHead=false "$@"
}

configure_repo() {
  local dir="$1"
  git -C "$dir" config user.email semver-baseline@example.invalid
  git -C "$dir" config user.name "semver baseline test"
  git -C "$dir" config commit.gpgsign false
  git -C "$dir" config tag.gpgsign false
}

# new_repo DIR VERSION CHANGELOG_BODY: a repository whose first commit is the
# given release tree.
new_repo() {
  local dir="$1" version="$2" body="$3"
  mkdir -p "$dir"
  git_quiet -C "$dir" init -q
  configure_repo "$dir"
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

# select_in DIR PUBLISHED [NAME=VALUE...]: run the selection in DIR with
# crates.io answering PUBLISHED (`__unreachable__` makes the lookup fail) and
# the given environment. Prints "<baseline_version> <post_release>", or
# "refused <explanation>". Its stderr is left in DIR/.selection-stderr.
select_in() {
  local dir="$1" crates_io_answer="$2"
  shift 2
  (
    cd "$dir"
    unset MEERKAT_SEMVER_BASELINE_VERSION MEERKAT_SEMVER_REQUIRE_RELEASE_TREE
    for assignment in "$@"; do
      export "${assignment?}"
    done
    # shellcheck source=scripts/semver-baseline.sh
    source "$REPO_ROOT/scripts/semver-baseline.sh"
    semver_published_version() {
      [[ "$crates_io_answer" != "__unreachable__" ]] || return 1
      printf '%s\n' "$crates_io_answer"
    }
    semver_analyser="$ANALYSER"
    semver_changelog="$dir/CHANGELOG.md"
    workspace_version="$(
      "$PYTHON" -c 'import pathlib,tomllib; print(tomllib.loads(pathlib.Path("Cargo.toml").read_text())["workspace"]["package"]["version"])'
    )"
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

expect_stderr() {
  local name="$1" needle="$2" dir="$3"
  if ! grep -Fq -- "$needle" "$dir/.selection-stderr"; then
    fail "${name}: expected stderr to mention '${needle}', got '$(tr '\n' ' ' <"$dir/.selection-stderr")'"
  fi
}

# with_origin DIR: give DIR a bare `origin` holding its history and tags.
with_origin() {
  local dir="$1"
  git_quiet clone -q --bare "$dir" "$dir.origin.git"
  git -C "$dir" remote add origin "$dir.origin.git"
}

# 1. The normal release path: the release commit's notes are stamped for the
#    workspace version with an empty stub above, measured against the newest
#    published release. Unchanged by the post-release rule, including under
#    the release workflow's release-tree requirement.
repo="$TEST_ROOT/release-tree"
new_repo "$repo" 0.8.44 "$RELEASE_TREE"
expect "release tree before publication" "0.8.43 false" "$(select_in "$repo" 0.8.43)"
expect "release tree under the release-tree requirement" "0.8.43 false" \
  "$(select_in "$repo" 0.8.43 MEERKAT_SEMVER_REQUIRE_RELEASE_TREE=1)"
expect_refusal "release tree after publication" "already the published baseline" \
  "$(select_in "$repo" 0.8.44)"
expect_refusal "an empty crates.io answer" "could not resolve the published baseline" \
  "$(select_in "$repo" "")"
expect_refusal "crates.io unreachable" "could not resolve the published baseline" \
  "$(select_in "$repo" __unreachable__)"

# 2. The v0.8.44 failure: the release is tagged, HEAD has moved past it, and
#    new notes sit under `## [Unreleased]`. The tag is the baseline whether or
#    not crates.io has caught up.
repo="$TEST_ROOT/post-release"
new_repo "$repo" 0.8.44 "$RELEASE_TREE"
git -C "$repo" tag v0.8.44
commit_changelog "$repo" "$POST_RELEASE"
expect "post-release tree before publication" "0.8.44 true" "$(select_in "$repo" 0.8.43)"
expect_stderr "post-release tree warns that crates.io is behind" \
  "crates.io's newest meerkat-core is 0.8.43, not 0.8.44" "$repo"
expect "post-release tree after publication" "0.8.44 true" "$(select_in "$repo" 0.8.44)"
if [[ -s "$repo/.selection-stderr" ]]; then
  fail "post-release tree after publication warned: $(cat "$repo/.selection-stderr")"
fi
expect "post-release tree with crates.io unreachable" "0.8.44 true" \
  "$(select_in "$repo" __unreachable__)"
expect_stderr "post-release tree says it could not compare with crates.io" \
  "could not read crates.io's newest meerkat-core" "$repo"
expect "explicit override still wins" "0.8.42 false" \
  "$(select_in "$repo" 0.8.43 MEERKAT_SEMVER_BASELINE_VERSION=0.8.42)"
# The release workflow's own measurement must never publish main's tip as the
# tagged version: it refuses the post-release classification outright.
expect_refusal "post-release tree under the release-tree requirement" "this is a post-release tree" \
  "$(select_in "$repo" 0.8.43 MEERKAT_SEMVER_REQUIRE_RELEASE_TREE=1)"
expect_refusal "the release-tree requirement beats an override" "this is a post-release tree" \
  "$(select_in "$repo" 0.8.43 MEERKAT_SEMVER_REQUIRE_RELEASE_TREE=1 MEERKAT_SEMVER_BASELINE_VERSION=0.8.43)"

# 3. The tag exists only on origin (a clone that has not fetched it): the
#    selection fetches it and proceeds.
repo="$TEST_ROOT/tag-on-origin"
new_repo "$repo" 0.8.44 "$RELEASE_TREE"
git -C "$repo" tag v0.8.44
commit_changelog "$repo" "$POST_RELEASE"
with_origin "$repo"
git -C "$repo" tag -d v0.8.44 >/dev/null
expect "tag fetched from origin" "0.8.44 true" "$(select_in "$repo" 0.8.44)"
if ! git -C "$repo" rev-parse -q --verify refs/tags/v0.8.44 >/dev/null; then
  fail "tag fetched from origin: v0.8.44 was not made local"
fi

# 4. A post-release tree with no new notes is still measured as the release:
#    its pending section is the stamped one.
repo="$TEST_ROOT/post-release-no-notes"
new_repo "$repo" 0.8.44 "$RELEASE_TREE"
git -C "$repo" tag v0.8.44
printf 'unrelated\n' >"$repo/README.md"
git -C "$repo" add README.md
git -C "$repo" commit -q -m "unrelated"
expect "post-release tree without new notes" "0.8.43 false" "$(select_in "$repo" 0.8.43)"

# 5. At the tagged commit itself nothing has moved past the release, so a
#    malformed release tree with notes left under `## [Unreleased]` keeps the
#    published baseline rather than measuring the tag against itself.
repo="$TEST_ROOT/tagged-with-notes"
new_repo "$repo" 0.8.44 "$POST_RELEASE"
git -C "$repo" tag v0.8.44
expect "notes at the tagged commit" "0.8.43 false" "$(select_in "$repo" 0.8.43)"

# 6. The release commit with notes above its stamped section and no tag
#    anywhere: tell the operator where the notes go, not to set an override.
repo="$TEST_ROOT/untagged"
new_repo "$repo" 0.8.44 "$RELEASE_TREE"
commit_changelog "$repo" "$POST_RELEASE"
with_origin "$repo"
result="$(select_in "$repo" 0.8.43)"
expect_refusal "post-release notes before the tag" "is not tagged, locally or on origin" "$result"
expect_refusal "post-release notes before the tag" "move them into its stamped" "$result"
if [[ "$result" == *MEERKAT_SEMVER_BASELINE_VERSION* ]]; then
  fail "post-release notes before the tag: suggests an override no value satisfies: $result"
fi

# 7. No reachable origin is not evidence the tag is missing: say origin could
#    not be asked.
repo="$TEST_ROOT/no-origin"
new_repo "$repo" 0.8.44 "$RELEASE_TREE"
commit_changelog "$repo" "$POST_RELEASE"
expect_refusal "no origin to ask" "could not ask origin whether v0.8.44 exists" \
  "$(select_in "$repo" 0.8.43)"
git -C "$repo" remote add origin "$TEST_ROOT/does-not-exist.git"
expect_refusal "unreachable origin" "could not ask origin whether v0.8.44 exists" \
  "$(select_in "$repo" 0.8.43)"

# 8. A tag HEAD does not descend from is not the release these notes follow.
repo="$TEST_ROOT/foreign-tag"
new_repo "$repo" 0.8.44 "$RELEASE_TREE"
git -C "$repo" checkout -q -b elsewhere
commit_changelog "$repo" "${RELEASE_TREE}- elsewhere"$'\n'
git -C "$repo" tag v0.8.44
git -C "$repo" checkout -q main
commit_changelog "$repo" "$POST_RELEASE"
expect_refusal "tag off HEAD's history" "is not an ancestor of HEAD" "$(select_in "$repo" 0.8.43)"

# 9. A shallow clone cannot see the tag's ancestry: say so instead of
#    claiming the tag is not an ancestor.
source_repo="$TEST_ROOT/shallow-source"
new_repo "$source_repo" 0.8.44 "$RELEASE_TREE"
git -C "$source_repo" tag v0.8.44
commit_changelog "$source_repo" "$POST_RELEASE"
repo="$TEST_ROOT/shallow"
git_quiet clone -q --depth 1 --no-tags "file://$source_repo" "$repo"
configure_repo "$repo"
result="$(select_in "$repo" 0.8.43)"
expect_refusal "shallow clone" "this clone is shallow" "$result"
if [[ "$result" == *"is not an ancestor of HEAD"* ]]; then
  fail "shallow clone: claims the tag is not an ancestor: $result"
fi

# 10. The version bump landed but the notes were never stamped: that is still
#     a release candidate measured against the published release
#     (check_stamped then rejects the unstamped notes).
repo="$TEST_ROOT/bumped-unstamped"
new_repo "$repo" 0.8.44 "${UNRELEASED_NOTES}${STAMPED_43}"
expect "bumped but unstamped notes" "0.8.43 false" "$(select_in "$repo" 0.8.43)"

# 11. End to end: selection, then the analyser on the report a measurement
#     against the SELECTED baseline yields. Against 0.8.43 the report holds
#     the 0.8.44 release's own breaks (already declared under `## [0.8.44]`);
#     against v0.8.44 it holds only what changed since.
# analyse_selected DIR PUBLISHED SINCE_RELEASE_REPORT SINCE_RELEASE_EXIT
analyse_selected() {
  local dir="$1" published="$2" since_report="$3" since_exit="$4"
  local selected report exit_code
  selected="$(select_in "$dir" "$published")"
  case "$selected" in
    "0.8.44 true")
      report="$since_report"
      exit_code="$since_exit"
      ;;
    "0.8.43 false")
      report="$FIXTURES/report-meerkat-sqlite-0.8.22.txt"
      exit_code=1
      ;;
    *)
      echo "unexpected selection: $selected"
      return 0
      ;;
  esac
  if "$PYTHON" "$ANALYSER" --report "$report" --changelog "$dir/CHANGELOG.md" \
    --version 0.8.44 --tool-exit-code "$exit_code" >"$dir/.analysis" 2>&1; then
    echo "green"
  else
    echo "red $(tr '\n' ' ' <"$dir/.analysis")"
  fi
}

repo="$TEST_ROOT/end-to-end"
new_repo "$repo" 0.8.44 "$RELEASE_TREE"
git -C "$repo" tag v0.8.44
commit_changelog "$repo" "$POST_RELEASE"
# No new breaks since v0.8.44: green. The pre-fix selection (0.8.43) makes
# this red with "`## [Unreleased]` has no `### Breaking` heading".
expect "post-release notes without new breaks" "green" \
  "$(analyse_selected "$repo" 0.8.43 "$FIXTURES/report-clean-two-crates.txt" 0)"
# A new break since v0.8.44 must still be declared under `## [Unreleased]`.
result="$(analyse_selected "$repo" 0.8.43 "$FIXTURES/report-meerkat-sqlite-0.8.22.txt" 1)"
# shellcheck disable=SC2016 # the backticks are literal Markdown
if [[ "$result" != red* || "$result" != *'`## [Unreleased]` has no `### Breaking` heading'* ]]; then
  fail "a new break after the release: expected red naming ## [Unreleased], got '$result'"
fi

if [[ "$failures" -ne 0 ]]; then
  echo "semver baseline selection contract: ${failures} failure(s)" >&2
  exit 1
fi
echo "semver baseline selection contract holds"
