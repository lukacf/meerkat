#!/usr/bin/env bash
# Fixture needles are literal workflow text, so `${...}` in them must not expand.
# shellcheck disable=SC2016
# Contract test for the post-release guards in the release workflows
# (scripts/check_release_workflow_contract.py):
#
#   - release-semver-readiness.yml attests only a release tree: a post-release
#     tree is measured against its own tag, and `release_semver_gate` trusts
#     the main-push attestation by tree and version alone;
#   - release.yml's own `make semver-breaks` refuses a post-release tree, which
#     would otherwise pass and publish main's tip as the tagged version.
#
# Fixtures are mutations of the committed workflows. Equivalent spellings must
# pass; dropping a guard, or misspelling the output it is keyed on, must fail
# and name the defect. Every mutation asserts its needle occurs the expected
# number of times, so a rewording that invalidates a fixture fails here loudly.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON="${PYTHON:-$(command -v python3.11 2>/dev/null || command -v python3)}"
CHECKER="${REPO_ROOT}/scripts/check_release_workflow_contract.py"
READINESS="${REPO_ROOT}/.github/workflows/release-semver-readiness.yml"
RELEASE="${REPO_ROOT}/.github/workflows/release.yml"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-semver-readiness-contract.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

failures=0
fail() {
  echo "FAIL: $*" >&2
  failures=$((failures + 1))
}

# mutate SOURCE DEST OCCURRENCES NEEDLE REPLACEMENT [INDEX]: replace NEEDLE
# (which must occur exactly OCCURRENCES times) everywhere, or only its
# INDEX-th occurrence (0-based) when INDEX is given.
mutate() {
  "$PYTHON" - "$@" <<'PY'
import sys
source, dest, occurrences, needle, replacement = sys.argv[1:6]
index = int(sys.argv[6]) if len(sys.argv) > 6 else None
text = open(source, encoding="utf-8").read()
count = text.count(needle)
if count != int(occurrences):
    sys.exit(f"fixture needle occurs {count} times, expected {occurrences}: {needle!r}")
if index is None:
    text = text.replace(needle, replacement)
else:
    start = -1
    for _ in range(index + 1):
        start = text.index(needle, start + 1)
    text = text[:start] + replacement + text[start + len(needle):]
open(dest, "w", encoding="utf-8").write(text)
PY
}

# check WORKFLOW CHECK: the checker's violations, or "ok".
check() {
  local output
  if output="$("$PYTHON" "$CHECKER" --workflow "$1" "$2" 2>&1)"; then
    echo ok
  else
    echo "$output"
  fi
}

expect_ok() {
  local name="$1" result="$2"
  [[ "$result" == ok ]] || fail "${name}: expected the guard to hold, got: ${result}"
}

expect_violation() {
  local name="$1" needle="$2" result="$3"
  if [[ "$result" == ok || "$result" != *"$needle"* ]]; then
    fail "${name}: expected a violation mentioning '${needle}', got: ${result}"
  fi
}

CONDITION="steps.unpublished.outputs.needed == 'true' && steps.notes.outputs.baseline == 'published'"
WRITER='echo "baseline=${baseline}" >> "${GITHUB_OUTPUT}"'

# --- release-semver-readiness.yml ------------------------------------------
expect_ok "committed readiness workflow" "$(check "$READINESS" readiness-attestation)"

fixture="$TEST_ROOT/equivalent.yml"
mutate "$READINESS" "$fixture" 2 \
  "if: \${{ ${CONDITION} }}" \
  $'if: >-\n          ${{\n            \'published\' == steps.notes.outputs.baseline &&\n            steps.unpublished.outputs.needed == \'true\'\n          }}'
expect_ok "an equivalent folded, reordered condition" "$(check "$fixture" readiness-attestation)"

fixture="$TEST_ROOT/attestation-unguarded.yml"
mutate "$READINESS" "$fixture" 2 "$CONDITION" "steps.unpublished.outputs.needed == 'true'" 0
expect_violation "attestation without the notes guard" \
  "step \`Build exact-tree semver attestation\` also runs on a main push with needed='true' and notes baseline 'workspace-version'" \
  "$(check "$fixture" readiness-attestation)"

fixture="$TEST_ROOT/upload-unguarded.yml"
mutate "$READINESS" "$fixture" 2 "$CONDITION" "steps.unpublished.outputs.needed == 'true'" 1
expect_violation "upload without the notes guard" \
  "step \`Upload exact-tree semver evidence\` also runs on a main push" \
  "$(check "$fixture" readiness-attestation)"

fixture="$TEST_ROOT/condition-misspelled.yml"
mutate "$READINESS" "$fixture" 2 "steps.notes.outputs.baseline ==" "steps.notes.outputs.baselin =="
expect_violation "condition keyed on a misspelled output" \
  "context \`steps.notes.outputs.baselin\` is not modelled" \
  "$(check "$fixture" readiness-attestation)"

fixture="$TEST_ROOT/writer-misspelled.yml"
mutate "$READINESS" "$fixture" 1 "$WRITER" 'echo "baselin=${baseline}" >> "${GITHUB_OUTPUT}"'
expect_violation "classifier writing a misspelled output" \
  "not \`baseline\`, which the attestation is gated on" \
  "$(check "$fixture" readiness-attestation)"

fixture="$TEST_ROOT/no-notes-id.yml"
mutate "$READINESS" "$fixture" 1 $'        id: notes\n' ""
expect_violation "classifier without its step id" "0 steps with \`id: notes\`" \
  "$(check "$fixture" readiness-attestation)"

# --- release.yml -----------------------------------------------------------
expect_ok "committed release workflow" "$(check "$RELEASE" semver-evidence)"
REQUIRE='MEERKAT_SEMVER_REQUIRE_RELEASE_TREE: "1"'

fixture="$TEST_ROOT/release-unquoted.yml"
mutate "$RELEASE" "$fixture" 1 "$REQUIRE" "MEERKAT_SEMVER_REQUIRE_RELEASE_TREE: 1"
expect_ok "an unquoted release-tree requirement" "$(check "$fixture" semver-evidence)"

fixture="$TEST_ROOT/release-no-requirement.yml"
mutate "$RELEASE" "$fixture" 1 "          ${REQUIRE}"$'\n' ""
expect_violation "release measurement without the release-tree requirement" \
  "without \`MEERKAT_SEMVER_REQUIRE_RELEASE_TREE: \"1\"\`" "$(check "$fixture" semver-evidence)"

# An evidence step gated on a step output no context models must fail the
# check closed, not read the output as an empty string and pass.
fixture="$TEST_ROOT/release-unmodelled-output.yml"
mutate "$RELEASE" "$fixture" 1 \
  $'      - name: Verify exact-tree pre-tag semver evidence\n        if: >-\n          ${{\n' \
  $'      - name: Verify exact-tree pre-tag semver evidence\n        if: >-\n          ${{\n            steps.mode.outputs.skip_evidence != \'true\' &&\n'
expect_violation "evidence gated on an unmodelled step output" \
  "context \`steps.mode.outputs.skip_evidence\` is not modelled" "$(check "$fixture" semver-evidence)"

fixture="$TEST_ROOT/release-requirement-off.yml"
mutate "$RELEASE" "$fixture" 1 "$REQUIRE" 'MEERKAT_SEMVER_REQUIRE_RELEASE_TREE: "0"'
expect_violation "release measurement with the requirement switched off" \
  "without \`MEERKAT_SEMVER_REQUIRE_RELEASE_TREE: \"1\"\`" "$(check "$fixture" semver-evidence)"

if [[ "$failures" -ne 0 ]]; then
  echo "semver readiness workflow contract: ${failures} failure(s)" >&2
  exit 1
fi
echo "semver readiness workflow contract holds"
