#!/usr/bin/env bash
# Contract test for the release doctor's release-workflow assertions.
#
# Reproduces the #1091 drift shape: `.github/workflows/release.yml` was
# reworded after v0.8.32 (a folded `if: >-` condition and a `--slo-seconds
# ${{ ... }}` expression) without changing behaviour, and two doctor greps
# that matched literal lines failed on a healthy main. The fix asserts what
# the workflow does under concrete events. This test proves both halves of
# that contract on fixtures derived from the committed workflow: equivalent
# spellings pass, and removing the behaviour still fails and names the defect.
#
# Every fixture mutation asserts its needle occurs exactly once in the real
# workflow, so a rewording that invalidates a fixture fails here loudly
# instead of leaving the doctor unverified.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON="${PYTHON:-$(command -v python3.11 2>/dev/null || command -v python3)}"
CHECKER="${REPO_ROOT}/scripts/check_release_workflow_contract.py"
DOCTOR="${REPO_ROOT}/scripts/release-doctor"
WORKFLOW="${REPO_ROOT}/.github/workflows/release.yml"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-release-doctor-contract.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

SEMVER_PASS="PASS Tag releases reuse exact-tree pre-tag semver evidence"
SEMVER_FAIL="FAIL Tag releases must reuse exact-tree pre-tag semver evidence"
SLO_PASS="PASS Rust registry publication enforces the 30 minute tag-to-public SLO"
SLO_FAIL="FAIL Rust registry publication does not enforce the 30 minute tag-to-public SLO"

fail() {
  echo "release doctor workflow contract violated: $1" >&2
  shift
  for extra in "$@"; do
    echo "  ${extra}" >&2
  done
  exit 1
}

# mutate <name> <destination>: write a mutated copy of the committed workflow.
mutate() {
  local name="$1"
  local destination="$2"
  "$PYTHON" - "$WORKFLOW" "$destination" "$name" <<'PYEOF'
import pathlib
import re
import sys

source, destination, name = sys.argv[1:4]
text = pathlib.Path(source).read_text()


def replace_once(needle: str, replacement: str) -> None:
    global text
    count = text.count(needle)
    if count != 1:
        raise SystemExit(
            f"fixture `{name}` expects exactly one occurrence of {needle!r} "
            f"in the committed workflow, found {count}; update the fixture"
        )
    text = text.replace(needle, replacement)


EVIDENCE_STEP = re.compile(
    r"      - name: Verify exact-tree pre-tag semver evidence\n"
    r"(?:        .*\n|          .*\n|\n)*?"
    r"(?=      - name: )"
)
EVIDENCE_IF = (
    "            github.event_name != 'workflow_dispatch' ||\n"
    "            (github.event.inputs.release_tag != '' &&\n"
    "             github.event.inputs.semver_evidence_job_id == '')\n"
)
MEASUREMENT_STEP_IF = (
    "      - name: Verify every reported break is named and the notes are stamped\n"
    "        if: >-\n"
    "          ${{\n"
    "            github.event_name == 'workflow_dispatch' &&\n"
    "            github.event.inputs.semver_evidence_job_id == '' &&\n"
    "            github.event.inputs.release_tag == ''\n"
    "          }}\n"
    "        run: make semver-breaks\n"
)
SLO_LINE = (
    "            --slo-seconds ${{ github.event_name == 'workflow_dispatch'"
    " && '2147483647' || '1800' }}\n"
)
SLO_PREVIOUS_LINE = "            --deadline-seconds 900 \\\n"

if name == "evidence-step-removed":
    match = EVIDENCE_STEP.search(text)
    if match is None or len(EVIDENCE_STEP.findall(text)) != 1:
        raise SystemExit("fixture expects exactly one exact-tree evidence step")
    text = text[: match.start()] + text[match.end() :]
elif name == "evidence-step-off-tags":
    replace_once(
        EVIDENCE_IF,
        "            github.event_name == 'workflow_dispatch' &&\n"
        "            (github.event.inputs.release_tag != '' &&\n"
        "             github.event.inputs.semver_evidence_job_id == '')\n",
    )
elif name == "evidence-if-single-line":
    # Same condition, collapsed onto one line with a bare (unwrapped)
    # expression: the spelling GitHub also accepts.
    condition = " ".join(EVIDENCE_IF.split())
    replace_once(
        "        if: >-\n          ${{\n" + EVIDENCE_IF + "          }}\n        shell: bash\n"
        "        env:\n          GH_TOKEN: ${{ github.token }}\n        run: |\n"
        "          set -euo pipefail\n          tree_sha=",
        f"        if: {condition}\n        shell: bash\n"
        "        env:\n          GH_TOKEN: ${{ github.token }}\n        run: |\n"
        "          set -euo pipefail\n          tree_sha=",
    )
elif name == "measurement-on-tags":
    replace_once(
        MEASUREMENT_STEP_IF,
        "      - name: Verify every reported break is named and the notes are stamped\n"
        "        run: make semver-breaks\n",
    )
elif name == "slo-relaxed":
    replace_once(SLO_LINE, SLO_LINE.replace("'1800'", "'3600'"))
elif name == "slo-flag-removed":
    replace_once(SLO_PREVIOUS_LINE + SLO_LINE, "            --deadline-seconds 900\n")
elif name == "slo-literal":
    replace_once(SLO_LINE, "            --slo-seconds 1800\n")
elif name == "slo-reflowed":
    # Equivalent expression, operands reordered and split across lines.
    replace_once(
        SLO_LINE,
        "            --slo-seconds ${{\n"
        "              github.event_name != 'workflow_dispatch' && '1800'\n"
        "              || '2147483647'\n"
        "            }}\n",
    )
elif name == "both-defects":
    replace_once(
        MEASUREMENT_STEP_IF,
        "      - name: Verify every reported break is named and the notes are stamped\n"
        "        run: make semver-breaks\n",
    )
    replace_once(SLO_LINE, SLO_LINE.replace("'1800'", "'3600'"))
else:
    raise SystemExit(f"unknown fixture `{name}`")

pathlib.Path(destination).write_text(text)
PYEOF
}

# run_check <workflow> <log> <check...>: prints the checker's exit status.
run_check() {
  local workflow="$1"
  local log="$2"
  shift 2
  set +e
  "$PYTHON" "$CHECKER" --workflow "$workflow" "$@" >"$log" 2>&1
  local status=$?
  set -e
  printf '%s' "$status"
}

expect_pass() {
  local label="$1"
  local fixture="$2"
  shift 2
  local log="${TEST_ROOT}/${label}.log"
  local status
  status="$(run_check "$fixture" "$log" "$@")"
  if [[ "$status" -ne 0 ]]; then
    fail "${label}: an equivalent workflow spelling was rejected" "$(cat "$log")"
  fi
}

expect_fail_named() {
  local label="$1"
  local fixture="$2"
  local needle="$3"
  shift 3
  local log="${TEST_ROOT}/${label}.log"
  local status
  status="$(run_check "$fixture" "$log" "$@")"
  if [[ "$status" -eq 0 ]]; then
    fail "${label}: a workflow that dropped the behaviour was accepted"
  fi
  if ! grep -Fq "$needle" "$log"; then
    fail "${label}: failure does not name the defect (expected: ${needle})" "$(cat "$log")"
  fi
}

# 1. The committed workflow satisfies both contracts.
expect_pass committed "$WORKFLOW" all

# 2. Equivalent spellings pass: this is the #1091 false positive.
mutate evidence-if-single-line "${TEST_ROOT}/evidence-if-single-line.yml"
expect_pass evidence-if-single-line "${TEST_ROOT}/evidence-if-single-line.yml" semver-evidence
mutate slo-literal "${TEST_ROOT}/slo-literal.yml"
expect_pass slo-literal "${TEST_ROOT}/slo-literal.yml" registry-slo
mutate slo-reflowed "${TEST_ROOT}/slo-reflowed.yml"
expect_pass slo-reflowed "${TEST_ROOT}/slo-reflowed.yml" registry-slo

# 3. Dropping the behaviour fails and names the defect.
mutate evidence-step-removed "${TEST_ROOT}/evidence-step-removed.yml"
expect_fail_named evidence-step-removed "${TEST_ROOT}/evidence-step-removed.yml" \
  "no step that resolves the exact-tree" semver-evidence
mutate evidence-step-off-tags "${TEST_ROOT}/evidence-step-off-tags.yml"
expect_fail_named evidence-step-off-tags "${TEST_ROOT}/evidence-step-off-tags.yml" \
  "does not run on a tag push" semver-evidence
mutate measurement-on-tags "${TEST_ROOT}/measurement-on-tags.yml"
expect_fail_named measurement-on-tags "${TEST_ROOT}/measurement-on-tags.yml" \
  "reruns \`make semver-breaks\` on a tag push" semver-evidence
mutate slo-relaxed "${TEST_ROOT}/slo-relaxed.yml"
expect_fail_named slo-relaxed "${TEST_ROOT}/slo-relaxed.yml" \
  "passes \`--slo-seconds 3600\` on a tag push" registry-slo
mutate slo-flag-removed "${TEST_ROOT}/slo-flag-removed.yml"
expect_fail_named slo-flag-removed "${TEST_ROOT}/slo-flag-removed.yml" \
  "without \`--slo-seconds\`" registry-slo

# 4. The doctor itself binds to the checker: its PASS/FAIL lines follow the
#    workflow it is pointed at. Environment checks (gh, npm) are ignored; only
#    the two workflow-contract lines are asserted.
run_doctor() {
  local workflow="$1"
  local log="$2"
  set +e
  RELEASE_WORKFLOW_FILE="$workflow" "$DOCTOR" >"$log" 2>&1
  set -e
}

run_doctor "$WORKFLOW" "${TEST_ROOT}/doctor-committed.log"
for line in "$SEMVER_PASS" "$SLO_PASS"; do
  if ! grep -Fxq "$line" "${TEST_ROOT}/doctor-committed.log"; then
    fail "doctor does not report \`${line}\` for the committed workflow" \
      "$(cat "${TEST_ROOT}/doctor-committed.log")"
  fi
done

mutate both-defects "${TEST_ROOT}/both-defects.yml"
run_doctor "${TEST_ROOT}/both-defects.yml" "${TEST_ROOT}/doctor-defects.log"
for line in "$SEMVER_FAIL" "$SLO_FAIL"; do
  if ! grep -Fq "$line" "${TEST_ROOT}/doctor-defects.log"; then
    fail "doctor does not report \`${line}\` for a workflow that dropped the behaviour" \
      "$(cat "${TEST_ROOT}/doctor-defects.log")"
  fi
done

echo "release doctor workflow contract holds"
