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
PYTHON="$("$(dirname "${BASH_SOURCE[0]}")/require-python" 3.11 "$(basename "${BASH_SOURCE[0]}")")" || exit 1
CHECKER="${REPO_ROOT}/scripts/check_release_workflow_contract.py"
DOCTOR="${REPO_ROOT}/scripts/release-doctor"
WORKFLOW="${REPO_ROOT}/.github/workflows/release.yml"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-release-doctor-contract.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

SEMVER_PASS="PASS Tag releases reuse exact-tree pre-tag semver evidence"
SEMVER_FAIL="FAIL Tag releases must reuse exact-tree pre-tag semver evidence"
SLO_PASS="PASS Rust registry publication reads crates back before the SDKs and enforces the 30 minute publication SLO"
SLO_FAIL="FAIL Rust registry publication does not read crates back before the SDKs or enforce the 30 minute publication SLO"
DISPATCH_PASS="PASS Publishing dispatches are bound to their version's tag"
DISPATCH_FAIL="FAIL A publishing dispatch without release_tag could publish a branch tip as the tagged version"
TAG_VALIDATION_PASS="PASS Tag releases skip Cargo validation only on an exact-tree CI attestation that proves its lanes ran"
TAG_VALIDATION_FAIL="FAIL Tag releases may skip Cargo validation only on an exact-tree CI attestation that proves its lanes ran"
PREBUILD_PASS="PASS Pre-tag prebuilds publish nothing, and a tag publishes its own commit's prebuild only through the attested-archive verification"
PREBUILD_FAIL="FAIL A pre-tag prebuild must publish nothing, and a tag may publish its commit's prebuild only through the attested-archive verification"
BASELINE_PASS="PASS Semver baseline rustdoc builds alongside the release and gates nothing; it attaches only to a published release, never replacing an attached one"
BASELINE_FAIL="FAIL The semver baseline rustdoc must stay off every path a user waits on"
TRAILING_PASS="PASS Linux and macOS arm64 assets publish without waiting for the trailing Intel macOS and Windows lanes; no upload replaces a published asset"
TRAILING_FAIL="FAIL The trailing Intel macOS and Windows lanes must not gate the critical assets, and no upload may replace a published asset"
WINDOWS_PASS="PASS Release workflow cross-compiles Windows binaries on Linux, verifies them on the pinned windows-2025-vs2026 runner, and gates on both"
WINDOWS_FAIL="FAIL Release workflow should cross-compile Windows binaries on Linux (build_binaries_windows_cross using build-windows-cross from the workflow-ref tooling checkout), verify them on the pinned windows-2025-vs2026 runner (verify_windows_binaries), keep the artifact name meerkat-x86_64-pc-windows-msvc-windows-latest, and gate on both jobs"
WEB_TOOLS_PASS="PASS The Web SDK release build checks every download's sha256 and wasm-pack installs nothing: wasm-bindgen (Cargo.lock's version) and wasm-opt are pinned"
TOKEN_PASS="PASS Release workflow token is read-only except for the jobs that attest or publish, which never persist it in a checkout; no workflow passes every secret to a reusable workflow"
CACHES_PASS="PASS GitHub Actions caches in release.yml jobs, the local actions they call and, on a tag, the reusable workflows they call are never saved on a dispatch and never cache ~/.cargo/bin; token jobs restore none, no release job or semver readiness run restores a cache on any event, OIDC or attesting jobs restore none that a dispatch-like run of any workflow saves, and no dispatch-like run saves the Bazel repository cache (reusable workflows never run on a dispatch); a workflow that produces what a tag trusts or checks out a ref its inputs choose saves no cache on a dispatch-like run; only push and schedule runs set CI's sccache client to write its bucket (a client-side setting), and CI installs a sha256-pinned sccache release without sccache-action, which would export the Actions cache token to its run steps"
CACHES_FAIL="FAIL A release cache can be saved on a dispatch, cache ~/.cargo/bin, reach a token, OIDC or attesting job, or be restored by a release job, or a workflow a tag trusts saves a cache on a dispatch-like run, or a run other than a push or schedule sets CI's sccache client to write, or CI installs sccache through sccache-action or without a pinned sha256"

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
)
SLO_LINE = (
    "            --slo-seconds ${{ github.event_name == 'workflow_dispatch'"
    " && '2147483647' || '1800' }}\n"
)
SLO_PREVIOUS_LINE = (
    '            ${tag_pushed_at:+--tag-pushed-at "${tag_pushed_at}"} \\\n'
)

STEP_BODY = r"(?:        .*\n|          .*\n|\n)*?(?=      - name: |\Z|  [a-z_]+:\n)"
READBACK_STEP = re.compile(r"      - name: Verify all Rust crates are public\n" + STEP_BODY)
SLO_STEP = re.compile(r"      - name: Enforce the 30 minute Rust publication SLO\n" + STEP_BODY)
PYTHON_SDK_STEP = re.compile(r"      - name: Publish Python SDK\n")

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
        "      - name: Verify every reported break is named and the notes are stamped\n",
    )
elif name == "slo-relaxed":
    replace_once(SLO_LINE, SLO_LINE.replace("'1800'", "'3600'"))
elif name == "slo-flag-removed":
    replace_once(
        SLO_PREVIOUS_LINE + SLO_LINE,
        '            ${tag_pushed_at:+--tag-pushed-at "${tag_pushed_at}"}\n',
    )
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
elif name == "readback-step-removed":
    match = READBACK_STEP.search(text)
    if match is None or len(READBACK_STEP.findall(text)) != 1:
        raise SystemExit("fixture expects exactly one crate readback step")
    text = text[: match.start()] + text[match.end() :]
elif name == "slo-before-sdk":
    slo = SLO_STEP.search(text)
    sdk = PYTHON_SDK_STEP.search(text)
    if slo is None or sdk is None:
        raise SystemExit("fixture expects the SLO step and the Python SDK step")
    slo_text = slo.group(0)
    text = text[: slo.start()] + text[slo.end() :]
    sdk = PYTHON_SDK_STEP.search(text)
    text = text[: sdk.start()] + slo_text + text[sdk.start() :]
elif name == "dispatch-unbound":
    # The pre-fix condition: a publishing dispatch with no release_tag
    # skipped the version check and published the branch tip.
    replace_once(
        "          (github.event_name == 'workflow_dispatch' &&\n"
        "           (github.event.inputs.release_tag != '' ||\n"
        "            github.event.inputs.publish_release_packages == 'true' ||\n"
        "            github.event.inputs.publish_web_sdk_only == 'true' ||\n"
        "            github.event.inputs.publish_release_assets_only == 'true'))\n",
        "          (github.event_name == 'workflow_dispatch' &&\n"
        "           github.event.inputs.release_tag != '')\n",
    )
elif name == "dispatch-branch-name":
    # The pre-round-2 comparison: the last path segment of the ref name,
    # so a branch named release/v<version> passed and could publish.
    binding = re.compile(
        r'          expected="v\$\{version\}"\n(?:.*\n)*?'
        r'          echo "Version \$\{version\} matches release tag \$\{release_tag\}"\n'
    )
    text, count = binding.subn(
        '          ref_name="${RELEASE_TAG_INPUT:-${GITHUB_REF_NAME}}"\n'
        '          tag="${ref_name##*/}"\n'
        '          tag="${tag#v}"\n'
        '          if [[ "${version}" != "${tag}" ]]; then\n'
        '            exit 1\n'
        '          fi\n',
        text,
    )
    if count != 1:
        raise SystemExit(f"fixture `{name}` expects one release-version script, found {count}")
elif name == "dispatch-head-unchecked":
    # Drop the guard that the named release_tag is the checked-out commit.
    replace_once(
        '            if [[ "$(git rev-parse HEAD)" != "${tag_commit}" ]]; then\n'
        '              echo "The checked-out commit is not the ${RELEASE_TAG_INPUT} tag." >&2\n'
        "              exit 1\n"
        "            fi\n",
        "",
    )
elif name == "dispatch-alpha-unscoped":
    # Allow the alpha tag namespace outside the alpha crates-only lane.
    replace_once(
        '          if [[ "${ALPHA_CRATES_ONLY}" == "true" ]]; then\n'
        '            allowed+=("alpha/${expected}")\n'
        "          fi\n",
        '          allowed+=("alpha/${expected}")\n',
    )
elif name == "dispatch-checkout-github-ref":
    # require_ci_green checks out github.ref instead of the named release_tag.
    job = text.index("  require_ci_green:\n")
    needle = (
        "          ref: ${{ github.event_name == 'workflow_dispatch' && "
        "github.event.inputs.release_tag != '' && github.event.inputs.release_tag || github.ref }}\n"
    )
    at = text.index(needle, job)
    text = text[:at] + "          ref: ${{ github.ref }}\n" + text[at + len(needle):]
elif name == "dispatch-prefers-github-ref":
    # Prefer a tag github.ref over the named release_tag: a dispatch on the
    # version's tag that names a branch checks out and publishes the branch.
    replace_once(
        '          if [[ -n "${RELEASE_TAG_INPUT}" ]]; then\n',
        '          if [[ "${GITHUB_REF}" == refs/tags/* ]]; then\n'
        '            release_tag="${GITHUB_REF#refs/tags/}"\n'
        '          elif [[ -n "${RELEASE_TAG_INPUT}" ]]; then\n',
    )
elif name == "dispatch-alpha-any-version":
    # Accept any alpha/* tag in the alpha lane, not only this version's.
    replace_once(
        '            if [[ "${release_tag}" == "${tag}" ]]; then\n',
        '            if [[ "${release_tag}" == "${tag}" || ( "${ALPHA_CRATES_ONLY}" == "true" && "${release_tag}" == alpha/* ) ]]; then\n',
    )
elif name == "dispatch-ref-name":
    # Equivalent: read a tag push's tag from GITHUB_REF_NAME, which GitHub
    # sets to the ref without refs/tags/ (alpha/v<version> stays whole).
    replace_once(
        '            release_tag="${GITHUB_REF#refs/tags/}"\n',
        '            release_tag="${GITHUB_REF_NAME}"\n',
    )
elif name == "dispatch-branch-ref-name":
    # Read a non-tag ref's name without requiring refs/tags/: a branch named
    # exactly like an allowed tag (refs/heads/v<version>) then binds.
    replace_once(
        '          elif [[ "${GITHUB_REF}" == refs/tags/* ]]; then\n'
        '            release_tag="${GITHUB_REF#refs/tags/}"\n'
        "          else\n"
        '            echo "${GITHUB_REF} is not a tag. A publishing run must run on an allowed tag (${allowed[*]}) or name release_tag." >&2\n'
        "            exit 1\n"
        "          fi\n",
        "          else\n"
        '            release_tag="${GITHUB_REF_NAME}"\n'
        "          fi\n",
    )
elif name == "dispatch-branch-ref-name-tag-exists":
    # Read a non-tag ref's name as the tag once a tag of that name exists: a
    # branch named like the version's tag then publishes its tip as that tag.
    replace_once(
        '          elif [[ "${GITHUB_REF}" == refs/tags/* ]]; then\n'
        '            release_tag="${GITHUB_REF#refs/tags/}"\n'
        "          else\n"
        '            echo "${GITHUB_REF} is not a tag. A publishing run must run on an allowed tag (${allowed[*]}) or name release_tag." >&2\n'
        "            exit 1\n"
        "          fi\n",
        "          else\n"
        '            release_tag="${GITHUB_REF_NAME}"\n'
        '            if ! git rev-parse -q --verify "refs/tags/${release_tag}^{commit}" >/dev/null; then\n'
        "              exit 1\n"
        "            fi\n"
        "          fi\n",
    )
elif name == "dispatch-ref-name-expression":
    # Equivalent: a tag push's tag read from the ${{ github.ref_name }}
    # expression (the spelling release.yml uses elsewhere).
    replace_once(
        '            release_tag="${GITHUB_REF#refs/tags/}"\n',
        '            release_tag="${{ github.ref_name }}"\n',
    )
elif name == "dispatch-prefers-github-ref-other-tag":
    # Prefer a tag github.ref over the named release_tag, with no check of
    # the named tag at all.
    binding = re.compile(
        r'          if \[\[ -n "\$\{RELEASE_TAG_INPUT\}" \]\]; then\n(?:.*\n)*?'
        r'          fi\n(?=          bound=false\n)'
    )
    text, count = binding.subn(
        '          if [[ "${GITHUB_REF}" == refs/tags/* ]]; then\n'
        '            release_tag="${GITHUB_REF#refs/tags/}"\n'
        "          else\n"
        '            release_tag="${RELEASE_TAG_INPUT}"\n'
        "          fi\n",
        text,
    )
    if count != 1:
        raise SystemExit(f"fixture `{name}` expects one release-tag selection, found {count}")
elif name == "dispatch-prefers-github-ref-head-checked":
    # Prefer a tag github.ref, and check that it is the checked-out commit:
    # a named alpha tag on the same commit then publishes outside its lane.
    replace_once(
        '          if [[ -n "${RELEASE_TAG_INPUT}" ]]; then\n',
        '          if [[ "${GITHUB_REF}" == refs/tags/* ]]; then\n'
        '            release_tag="${GITHUB_REF#refs/tags/}"\n'
        '            if [[ "$(git rev-parse HEAD)" != "$(git rev-parse -q --verify "refs/tags/${release_tag}^{commit}")" ]]; then\n'
        "              exit 1\n"
        "            fi\n"
        '          elif [[ -n "${RELEASE_TAG_INPUT}" ]]; then\n',
    )
elif name == "dispatch-prefix-glob":
    # Compare the tag by prefix: v<version>1 and alpha/v<version>-rc.1 bind.
    replace_once(
        '            if [[ "${release_tag}" == "${tag}" ]]; then\n',
        '            if [[ "${release_tag}" == "${tag}"* ]]; then\n',
    )
elif name == "dispatch-alpha-named-any-version":
    # Accept any alpha/* tag the alpha lane names as release_tag.
    replace_once(
        '            release_tag="${RELEASE_TAG_INPUT}"\n',
        '            release_tag="${RELEASE_TAG_INPUT}"\n'
        '            if [[ "${ALPHA_CRATES_ONLY}" == "true" && "${RELEASE_TAG_INPUT}" == alpha/* ]]; then\n'
        '              allowed+=("${RELEASE_TAG_INPUT}")\n'
        "            fi\n",
    )
elif name == "dispatch-skips-named-tag-run":
    # Skip the binding step when a dispatch on a v* tag also names a
    # release_tag, which is then checked out and published unbound.
    replace_once(
        "          startsWith(github.ref, 'refs/tags/v') ||\n"
        "          (github.event_name == 'workflow_dispatch' &&\n",
        "          (startsWith(github.ref, 'refs/tags/v') && github.event.inputs.release_tag == '') ||\n"
        "          (github.event_name == 'workflow_dispatch' && !startsWith(github.ref, 'refs/tags/v') &&\n",
    )
elif name == "dispatch-reflowed":
    # Equivalent: the same publishing modes, reordered onto one line.
    replace_once(
        "          (github.event_name == 'workflow_dispatch' &&\n"
        "           (github.event.inputs.release_tag != '' ||\n"
        "            github.event.inputs.publish_release_packages == 'true' ||\n"
        "            github.event.inputs.publish_web_sdk_only == 'true' ||\n"
        "            github.event.inputs.publish_release_assets_only == 'true'))\n",
        "          (github.event_name == 'workflow_dispatch' && (github.event.inputs.publish_release_assets_only == 'true' || github.event.inputs.publish_web_sdk_only == 'true' || github.event.inputs.publish_release_packages == 'true' || github.event.inputs.release_tag != ''))\n",
    )
elif name == "tag-validation-cargo-on-attested-tags":
    # The pre-change route: a tag re-runs Cargo validation whenever the
    # self-hosted graph is off, whatever its exact-tree CI already covered.
    replace_once(
        " &&\n       needs.require_ci_green.outputs.ci_covers_release_validation != 'true') ||\n",
        ") ||\n",
    )
elif name == "tag-validation-gate-any-schema":
    # The gate trusts any verified attestation, so one that does not prove
    # the Cargo lanes ran skips Cargo validation too.
    replace_once(
        '            if [[ "${ci_covers}" == "true" ]]; then\n',
        '            if [[ -n "${ci_attestation_schema}" ]]; then\n',
    )
elif name == "tag-validation-schema-unwritten":
    # The verifier no longer exposes whether the attestation covers the lanes.
    replace_once('          echo "covers_release_validation=${covers}" >> "${GITHUB_OUTPUT}"\n', "")
elif name == "tag-validation-ignores-sdk-host":
    # The pre-review coverage: any schema-4 attestation, SDK suites or not.
    pattern = re.compile(
        r"              \(\.component_results\.sdk_host == \"success\" or\n(?:.*\n)*?"
        r"                \(\(\.reused_test_evidence\.workflow_run_attempt // \"\"\) \| test\(\"\^\[1-9\]\[0-9\]\*\$\"\)\)\)\)' \\\n"
    )
    text, count = pattern.subn("              true' \\\n", text)
    if count != 1:
        raise SystemExit(f"fixture `{name}` expects one SDK-lane clause, found {count}")
elif name == "tag-validation-reflowed":
    # Equivalent: the tag disjunct on one line, its operands reordered.
    replace_once(
        "      (github.event_name != 'workflow_dispatch' &&\n"
        "       vars.MEERKAT_GCP_BUILDBUDDY_ENABLED != 'true' &&\n"
        "       needs.require_ci_green.outputs.ci_covers_release_validation != 'true') ||\n",
        "      (needs.require_ci_green.outputs.ci_covers_release_validation != 'true' && "
        "vars.MEERKAT_GCP_BUILDBUDDY_ENABLED != 'true' && "
        "github.event_name != 'workflow_dispatch') ||\n",
    )
elif name in ("attestation-main-only", "attestation-any-release-branch", "attestation-ref-reordered"):
    # The CI attestation's ref rule: the pre-maintenance-release rule (main
    # only), any release branch instead of the tag's version's, and an
    # equivalent spelling with the operands reordered.
    rules = {
        "attestation-main-only": '.ref == "refs/heads/main"',
        "attestation-any-release-branch": '(.ref == "refs/heads/main" or (.ref | startswith("refs/heads/release/")))',
        "attestation-ref-reordered": '(.ref == $release_branch_ref or .ref == "refs/heads/main")',
    }
    replace_once(
        '             (.ref == "refs/heads/main" or .ref == $release_branch_ref) and\n'
        "             ((.schema_version == 1",
        f"             {rules[name]} and\n"
        "             ((.schema_version == 1",
    )
elif name in ("ci-run-main-only", "ci-run-any-release-branch", "ci-run-branches-reordered"):
    # The exact-commit CI lookup's branches: the pre-maintenance-release
    # rule (main only), any release branch instead of the tag's version's (no
    # branch in the query, any release/ branch in the filter), and an
    # equivalent spelling that queries the release branch first.
    branches = '            const branches = ["main", releaseBranch];\n'
    if name == "ci-run-main-only":
        replace_once(branches, '            const branches = ["main"];\n')
    elif name == "ci-run-any-release-branch":
        replace_once(
            '                  workflow_id: "ci.yml",\n                  branch,\n',
            '                  workflow_id: "ci.yml",\n',
        )
        replace_once(
            "                      run.head_branch === branch &&\n",
            '                      (run.head_branch === "main" || run.head_branch.startsWith("release/")) &&\n',
        )
    else:
        replace_once(branches, '            const branches = [releaseBranch, "main"];\n')
elif name in (
    "evidence-main-only",
    "evidence-any-release-branch",
    "evidence-run-any-branch",
    "evidence-pick-any-branch",
    "evidence-branch-reordered",
):
    # The readiness evidence's ref and run-branch rules: the
    # pre-maintenance-release rule (main only), any release branch, a run
    # branch checked neither where the artifact is picked nor where its run
    # is read, a pick of the tree's newest artifact from any branch, and an
    # equivalent spelling.
    attested = (
        '             (.ref == "refs/heads/main" or .ref == $release_branch_ref) and\n'
        "             .result == \"success\"'"
    )
    run_branch = '             (.head_branch == "main" or .head_branch == $release_branch) and\n'
    pick_branch = (
        "            '[.artifacts[] | select(.expired == false and\n"
        '               (.workflow_run.head_branch == "main" or .workflow_run.head_branch == $release_branch))]\n'
    )
    pick_any = "            '[.artifacts[] | select(.expired == false)]\n"
    if name == "evidence-main-only":
        replace_once(attested, '             .ref == "refs/heads/main" and\n             .result == "success"\'')
        replace_once(run_branch, '             .head_branch == "main" and\n')
    elif name == "evidence-any-release-branch":
        replace_once(
            attested,
            '             (.ref == "refs/heads/main" or (.ref | startswith("refs/heads/release/"))) and\n'
            "             .result == \"success\"'",
        )
    elif name == "evidence-run-any-branch":
        replace_once(pick_branch, pick_any)
        replace_once(run_branch, "")
    elif name == "evidence-pick-any-branch":
        replace_once(pick_branch, pick_any)
    else:
        replace_once(run_branch, '             (.head_branch == $release_branch or .head_branch == "main") and\n')
elif name == "prebuild-publishes":
    # A prebuild dispatch reaches the GitHub release publisher.
    replace_once(
        "  publish_github_release:\n    name: Publish GitHub release assets\n    if: >-\n",
        "  publish_github_release:\n    name: Publish GitHub release assets\n    if: >-\n"
        "      github.event.inputs.prebuild_release_artifacts == 'true' ||\n",
    )
elif name == "prebuild-moving-checkout":
    # The Web SDK prebuild checks out main's moving tip, not the dispatched commit.
    job = text.index("  build_web_sdk_package:\n")
    needle = (
        "          ref: ${{ github.event_name == 'workflow_dispatch' && "
        "github.event.inputs.prebuild_release_artifacts == 'true' && github.sha || "
    )
    at = text.index(needle, job)
    text = text[:at] + "          ref: ${{ " + text[at + len(needle):]
elif name == "prebuild-builds-anyway":
    # A tag whose commit was prebuilt rebuilds every binary anyway.
    needle = "      needs.release_prebuild.outputs.run_id == '' &&\n"
    if text.count(needle) != 4:
        raise SystemExit(f"fixture `{name}` expects four binary build guards, found {text.count(needle)}")
    text = text.replace(needle, "")
elif name == "prebuild-reuse-unverified":
    # The publisher takes the prebuild's archives through the exact-tag
    # verification, which a main-attested archive cannot pass honestly, and
    # drops the --source-run binding.
    pattern = re.compile(
        r'          if \[\[ -n "\$\{PREBUILD_RUN_ID\}" \]\]; then\n'
        r"            # The tag commit's pre-tag prebuild(?:.*\n)*?          elif "
    )
    text, count = pattern.subn("          if ", text)
    if count != 1:
        raise SystemExit(f"fixture `{name}` expects one prebuild verification branch, found {count}")
elif name == "prebuild-web-unverified":
    match = re.search(
        r"      - name: Verify the prebuilt Web SDK package\n" + STEP_BODY, text
    )
    if match is None:
        raise SystemExit(f"fixture `{name}` expects the Web SDK verification step")
    text = text[: match.start()] + text[match.end() :]
elif name == "prebuild-any-backend":
    # The request no longer checks the backend, so a GitHub-hosted prebuild
    # can race a BuildBuddy tag run (v0.9.3).
    replace_once(
        '          if [[ "${RELEASE_BACKEND}" != "${TAG_RUN_BACKEND}" ]]; then\n'
        '            echo "A release prebuild builds with the backend its tag run selects (${TAG_RUN_BACKEND}), '
        'got release_backend=${RELEASE_BACKEND}: dispatch it with make release-prebuild or '
        'release_backend=${TAG_RUN_BACKEND}." >&2\n'
        '            exit 1\n'
        '          fi\n',
        "",
    )
elif name == "prebuild-backend-fixed":
    # The request pins BuildBuddy instead of the tag run's own selection.
    replace_once(
        "          TAG_RUN_BACKEND: ${{ github.actor == 'lukacf' && (vars.MEERKAT_RELEASE_BUILDBUDDY == 'true' || "
        "vars.MEERKAT_RELEASE_BUILDBUDDY == '1') && 'buildbuddy' || 'github-hosted' }}\n",
        "          TAG_RUN_BACKEND: buildbuddy\n",
    )
elif name == "prebuild-gate-renamed":
    replace_once("    name: Release prebuild gate\n", "    name: Prebuild complete\n")
elif name == "prebuild-lookup-if-reflowed":
    # Equivalent: the lookup's condition folded, its operands reordered.
    replace_once(
        "    if: ${{ startsWith(github.ref, 'refs/tags/v') && github.event_name != 'workflow_dispatch' }}\n",
        "    if: >-\n      github.event_name != 'workflow_dispatch' &&\n      startsWith(github.ref, 'refs/tags/v')\n",
    )
elif name == "semver-baseline-gates-homebrew":
    # The Homebrew update waits for the 17-minute baseline rustdoc build.
    job = text.index("  update_homebrew:\n")
    needle = "    needs: [publish_github_release]\n"
    at = text.index(needle, job)
    text = text[:at] + "    needs: [publish_github_release, publish_semver_baseline]\n" + text[at + len(needle):]
elif name == "semver-baseline-after-release":
    # The pre-change shape: the rustdoc build starts only after publication.
    replace_once(
        "    name: Generate semver baseline rustdoc\n    needs: [require_ci_green]\n",
        "    name: Generate semver baseline rustdoc\n    needs: [publish_github_release]\n",
    )
elif name == "semver-baseline-needs-block-list":
    # Equivalent: the attach job's needs as a block sequence.
    replace_once(
        "    needs: [publish_github_release, publish_semver_baseline]\n",
        "    needs:\n      - publish_github_release\n      - publish_semver_baseline\n",
    )
elif name == "trailing-gates-critical":
    # The critical publisher waits for every lane again.
    replace_once(
        "    needs: [require_ci_green, release_validate_gate, release_semver_gate, build_binaries_critical_gate, release_prebuild]\n",
        "    needs: [require_ci_green, release_validate_gate, release_semver_gate, build_binaries_gate, release_prebuild]\n",
    )
elif name == "trailing-overwrites":
    # The critical upload replaces archives already on the release.
    replace_once(
        "            critical-release-artifacts/*.tar.gz\n"
        "          draft: false\n"
        "          fail_on_unmatched_files: true\n"
        "          generate_release_notes: true\n"
        "          overwrite_files: false\n",
        "            critical-release-artifacts/*.tar.gz\n"
        "          draft: false\n"
        "          fail_on_unmatched_files: true\n"
        "          generate_release_notes: true\n"
        "          overwrite_files: true\n",
    )
elif name == "trailing-selects-intel":
    # The critical set grows an Intel macOS archive.
    replace_once(
        '              "aarch64-apple-darwin",\n          )\n          wanted = sorted(\n',
        '              "aarch64-apple-darwin",\n              "x86_64-apple-darwin",\n          )\n'
        "          wanted = sorted(\n",
    )
elif name == "trailing-critical-if-reflowed":
    # Equivalent: the critical publisher's conditions in another order.
    replace_once(
        "      needs.release_semver_gate.result == 'success' &&\n"
        "      needs.build_binaries_critical_gate.result == 'success'\n",
        "      needs.build_binaries_critical_gate.result == 'success' &&\n"
        "      needs.release_semver_gate.result == 'success'\n",
    )
elif name == "prebuild-gate-waits-trailing":
    # The prebuild is reusable only once its trailing lanes finished too.
    replace_once(
        "    needs: [require_ci_green, build_binaries_critical_gate, build_web_sdk_package]\n",
        "    needs: [require_ci_green, build_binaries_gate, build_web_sdk_package]\n",
    )
elif name == "prebuild-reuse-no-wait":
    # The trailing publisher downloads before the prebuild's trailing lanes finished.
    match = re.search(
        r"      - name: Wait for the prebuild's trailing lanes\n"
        r"(?:        .*\n|          .*\n|\n)*?(?=      - name: |      # |\Z)",
        text,
    )
    if match is None:
        raise SystemExit(f"fixture `{name}` expects the trailing-lane wait step")
    text = text[: match.start()] + text[match.end() :]
elif name == "prebuild-critical-full-gate":
    # The critical publisher waits on the prebuild's full gate, which its
    # still-running trailing lanes hold back.
    replace_once('              --source-gate "Release critical binary build gate" \\\n', "")
elif name == "prebuild-web-ungated":
    # The pre-fix publish_web_sdk: a reused prebuild's package publishes
    # whatever release validation and the semver gate concluded.
    replace_once(
        "      (needs.release_validate_gate.result == 'success' ||\n"
        "       (github.event_name == 'workflow_dispatch' &&\n"
        "        github.event.inputs.publish_web_sdk_only == 'true' &&\n"
        "        needs.release_validate_gate.result == 'skipped')) &&\n"
        "      (needs.release_semver_gate.result == 'success' ||\n"
        "       (github.event_name == 'workflow_dispatch' &&\n"
        "        github.event.inputs.publish_web_sdk_only == 'true' &&\n"
        "        needs.release_semver_gate.result == 'skipped')) &&\n",
        "",
    )
elif name == "trailing-manifest-replaced":
    # The pre-review manifest upload: every run re-uploads it, replacing it.
    replace_once(
        "      - name: Publish complete release manifest\n"
        "        if: ${{ steps.manifest.outputs.upload == 'true' }}\n",
        "      - name: Publish complete release manifest\n",
    )
elif name == "token-write-default":
    # The pre-review default: every job gets a write token and OIDC.
    replace_once(
        "permissions:\n  actions: read\n  checks: read\n  contents: read\n",
        "permissions:\n  actions: read\n  checks: read\n  contents: write\n  id-token: write\n",
    )
elif name == "token-publisher-persisted":
    # The pre-review publishers kept the contents: write token in .git/config.
    job = text.index("  publish_github_release:\n")
    needle = "          persist-credentials: false\n"
    at = text.index(needle, job)
    text = text[:at] + text[at + len(needle):]
elif name == "secrets-inherit":
    replace_once(
        "    secrets:\n      MEERKAT_GCP_BUILDBUDDY_API_KEY: ${{ secrets.MEERKAT_GCP_BUILDBUDDY_API_KEY }}\n",
        "    secrets: inherit\n",
    )
elif name == "release-job-cache":
    # The pre-review build job restored a rust-cache, which on a dispatch
    # (a prebuild) reads main's scope, where no release run saves it.
    job = text.index("  build_binaries:\n")
    needle = "      - name: Add release target\n"
    at = text.index(needle, job)
    text = (
        text[:at]
        + "      - name: Cache cargo registry\n        uses: Swatinem/rust-cache@v2\n        with:\n"
        + "          save-if: ${{ github.event_name != 'workflow_dispatch' }}\n          cache-bin: \"false\"\n\n"
        + text[at:]
    )
elif name == "cache-registry-restore":
    # The pre-review registry publisher restored a cache with its tokens.
    job = text.index("  publish_registries:\n")
    needle = "      - name: Install Python + Node tooling\n"
    at = text.index(needle, job)
    text = text[:at] + "      - name: Cache cargo registry\n        uses: Swatinem/rust-cache@v2\n\n" + text[at:]
elif name == "cache-dispatch-save":
    # A build cache saved by dispatch runs, which tag runs restore.
    job = text.index("  build_binaries_windows_cross:\n")
    needle = "      - name: Resolve release version\n"
    at = text.index(needle, job)
    text = (
        text[:at]
        + "      - name: Cache cargo registry\n        uses: Swatinem/rust-cache@v2\n        with:\n"
        + "          cache-bin: \"false\"\n\n"
        + text[at:]
    )
elif name == "semver-baseline-clobber":
    # The pre-review attach replaced a baseline already on the release.
    replace_once(
        '          gh release upload "${RELEASE_TAG}" \\\n'
        '            --repo "${GITHUB_REPOSITORY}" \\\n'
        '            "semver-baseline/${asset}"\n',
        '          gh release upload "${RELEASE_TAG}" \\\n'
        '            --repo "${GITHUB_REPOSITORY}" \\\n'
        '            --clobber \\\n'
        '            "semver-baseline/${asset}"\n',
    )
elif name == "web-sdk-pack-mode-dropped":
    # wasm-pack downloads its own wasm-bindgen and wasm-opt again.
    replace_once("          MEERKAT_WEB_WASM_PACK_MODE: no-install\n", "")
elif name == "web-sdk-bindgen-mismatch":
    # A Cargo.lock bump without the pin: wasm-pack would refuse the binary.
    text, count = re.subn(r'(?m)^          bindgen_version="[^"]+"$', '          bindgen_version="0.0.0-stale"', text)
    if count != 1:
        raise SystemExit(f"fixture `{name}` expects one wasm-bindgen pin, found {count}")
elif name == "web-sdk-unchecked-download":
    replace_once(
        '          echo "${binaryen_sha256}  ${tools}/${binaryen_archive}" | sha256sum --check --strict -\n',
        "",
    )
elif name == "cache-xwin-dispatch-save":
    # The Windows SDK cache saved by dispatch runs too.
    replace_once(
        "          # No release job restores or saves a cache.\n          save-sdk-cache: \"false\"\n",
        "          save-sdk-cache: \"true\"\n",
    )
elif name == "cache-buildbuddy-repository":
    # The pre-review release binary build restored and saved the Bazel
    # repository cache.
    job = text.index("  build_binaries_buildbuddy:\n")
    needle = '          repository-cache: "false"\n'
    at = text.index(needle, job)
    text = text[:at] + text[at + len(needle):]
elif name == "reusable-tag-repository-restore":
    # The full-graph call without its repository_cache input: on a tag every
    # BuildBuddy lane of buildbuddy.yml restores the Bazel repository cache.
    replace_once(
        "      # This job mints OIDC tokens: no lane restores a Bazel repository\n"
        "      # cache entry another run saved.\n"
        '      repository_cache: "false"\n',
        "",
    )
elif name == "reusable-tag-unreadable":
    # A reusable workflow the check cannot read, called on a tag.
    replace_once(
        "    uses: ./.github/workflows/buildbuddy.yml\n",
        "    uses: example/workflows/.github/workflows/graph.yml@0123456789abcdef0123456789abcdef01234567 # v1\n",
    )
elif name == "reusable-on-dispatch":
    replace_once(
        "    if: ${{ github.event_name != 'workflow_dispatch' && vars.MEERKAT_GCP_BUILDBUDDY_ENABLED == 'true' }}\n",
        "    if: ${{ vars.MEERKAT_GCP_BUILDBUDDY_ENABLED == 'true' }}\n",
    )
elif name == "windows-verify-moving-image":
    # The verify job back on the label whose image GitHub moves.
    replace_once("    runs-on: windows-2025-vs2026\n", "    runs-on: windows-latest\n")
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

# mutate_tree <name> <root>: copy the committed .github tree under <root>
# and mutate one of its workflows or local actions; the checker reads local
# actions and the other workflows from there (--repo-root).
mutate_tree() {
  local name="$1"
  local root="$2"
  mkdir -p "$root"
  cp -R "${REPO_ROOT}/.github" "${root}/.github"
  "$PYTHON" - "$root" "$name" <<'PYEOF'
import pathlib
import sys

root, name = pathlib.Path(sys.argv[1]), sys.argv[2]


def replace_once(relative: str, needle: str, replacement: str) -> None:
    path = root / relative
    text = path.read_text()
    count = text.count(needle)
    if count != 1:
        raise SystemExit(
            f"fixture `{name}` expects exactly one occurrence of {needle!r} "
            f"in {relative}, found {count}; update the fixture"
        )
    path.write_text(text.replace(needle, replacement))


if name == "cache-xwin-save-ungated":
    # The pre-review action saved the Windows SDK cache from any run.
    replace_once(
        ".github/actions/build-windows-cross/action.yml",
        "      if: ${{ inputs.save-sdk-cache == 'true' && !startsWith(github.ref, 'refs/tags/') && steps.sdk-cache.outputs.cache-hit != 'true' }}\n",
        "      if: ${{ steps.sdk-cache.outputs.cache-hit != 'true' }}\n",
    )
elif name == "cache-xwin-validate-dispatch-save":
    # Another workflow's dispatch saves the cache that the release build
    # restores once the action restores it whatever its caller saves.
    replace_once(
        ".github/workflows/windows-cross-validate.yml",
        "          save-sdk-cache: ${{ github.event_name != 'workflow_dispatch' }}\n",
        '          save-sdk-cache: "true"\n',
    )
    replace_once(
        ".github/actions/build-windows-cross/action.yml",
        "      if: ${{ inputs.save-sdk-cache == 'true' && !startsWith(github.ref, 'refs/tags/') }}\n",
        "      if: ${{ !startsWith(github.ref, 'refs/tags/') }}\n",
    )
elif name == "cache-dispatch-shared-registry":
    # An attesting job shares a rust-cache key with a workflow whose trigger
    # is written with a value (`workflow_dispatch: {}`, as in ci.yml), which
    # saves that key on a dispatch.
    release = root / ".github/workflows/release.yml"
    text = release.read_text()
    job = text.index("  build_web_sdk_package:\n")
    needle = "      - uses: ./.release-tooling/.github/actions/setup-rust-ci\n\n"
    at = text.index(needle, job) + len(needle)
    release.write_text(
        text[:at]
        + "      - name: Cache cargo registry\n        uses: Swatinem/rust-cache@v2\n        with:\n"
        + "          shared-key: ci-registry\n"
        + "          save-if: ${{ github.event_name != 'workflow_dispatch' }}\n          cache-bin: \"false\"\n\n"
        + text[at:]
    )
    (root / ".github/workflows/registry-dispatch.yml").write_text(
        "name: Registry cache on dispatch\n"
        "\n"
        "on:\n"
        "  push:\n"
        "    branches:\n"
        "      - main\n"
        "  pull_request:\n"
        "    types:\n"
        "      - opened\n"
        "  workflow_dispatch: {}\n"
        "\n"
        "permissions:\n"
        "  contents: read\n"
        "\n"
        "jobs:\n"
        "  lint:\n"
        "    runs-on: ubuntu-24.04\n"
        "    steps:\n"
        "      - name: Cache cargo registry\n"
        "        uses: Swatinem/rust-cache@v2\n"
        "        with:\n"
        "          shared-key: ci-registry\n"
        "          cache-targets: \"false\"\n"
        "\n"
        "      - name: Lint\n"
        "        run: make lint\n"
    )
elif name == "dispatch-saves-repository-cache":
    # A dispatch-only workflow that saves the Bazel repository cache again.
    replace_once(
        ".github/workflows/release-binaries-check.yml",
        "          # A dispatch restores and never saves: a save lands in the default\n"
        "          # branch's cache scope, which every tag run would restore.\n"
        "          repository-cache: restore\n",
        "",
    )
elif name == "xwin-tag-restore":
    # The pre-review action restored the Windows SDK cache on every run.
    replace_once(
        ".github/actions/build-windows-cross/action.yml",
        "      id: sdk-cache\n      if: ${{ inputs.save-sdk-cache == 'true' && !startsWith(github.ref, 'refs/tags/') }}\n",
        "      id: sdk-cache\n",
    )
elif name in (
    "sccache-pull-request-writes",
    "sccache-dispatch-writes-any-ref",
    "sccache-dispatch-ref-name-prefix",
    "sccache-dispatch-ref-name-slash",
    "sccache-release-dispatch-reads",
):
    # The select step's RW_MODE replaced by a wrong rule: the pre-review
    # setup wrote the bucket from pull requests; a dispatch on any ref
    # writing it; a prefix on the ref's short name, which also takes
    # rel-next and released; the release prefixes on the short name, which
    # also take a tag named rel/next; and the rule before a release-branch
    # dispatch wrote it.
    rules = {
        "sccache-pull-request-writes": "github.event_name != 'workflow_dispatch'",
        "sccache-dispatch-writes-any-ref": "(github.event_name == 'push' || github.event_name == 'schedule' || github.event_name == 'workflow_dispatch')",
        "sccache-dispatch-ref-name-prefix": "(github.event_name == 'push' || github.event_name == 'schedule' || (github.event_name == 'workflow_dispatch' && startsWith(github.ref_name, 'rel')))",
        "sccache-dispatch-ref-name-slash": "(github.event_name == 'push' || github.event_name == 'schedule' || (github.event_name == 'workflow_dispatch' && (startsWith(github.ref_name, 'rel/') || startsWith(github.ref_name, 'release/'))))",
        "sccache-release-dispatch-reads": "(github.event_name == 'push' || github.event_name == 'schedule')",
    }
    path = root / ".github/actions/setup-sccache-ci/action.yml"
    text = path.read_text()
    step = text.index("    - name: Select the sccache backend\n")
    needle = "        RW_MODE: ${{ (github.event_name == 'push' || github.event_name == 'schedule' || (github.event_name == 'workflow_dispatch' && (startsWith(github.ref, 'refs/heads/rel/') || startsWith(github.ref, 'refs/heads/release/')))) && 'READ_WRITE' || 'READ_ONLY' }}\n"
    at = text.index(needle, step)
    path.write_text(
        text[:at]
        + "        RW_MODE: ${{ " + rules[name] + " && 'READ_WRITE' || 'READ_ONLY' }}\n"
        + text[at + len(needle):]
    )
elif name == "sccache-action-install":
    # The pre-review sccache setup installed through sccache-action, which
    # exports the job's Actions cache token to the run steps after it.
    replace_once(
        ".github/actions/setup-sccache-ci/action.yml",
        "  steps:\n",
        "  steps:\n"
        "    - uses: mozilla-actions/sccache-action@fc920bf0ec8de6ee65d409111f7ec508035751ba # v0.0.11\n"
        "      with:\n"
        "        version: v0.18.0\n",
    )
elif name == "sccache-unchecked-install":
    # An install that never checks the tarball against the pinned sha256.
    replace_once(
        ".github/actions/setup-sccache-ci/action.yml",
        '        echo "${SCCACHE_SHA256}  ${archive}" | sha256sum --check --strict -\n',
        "",
    )
elif name == "semver-readiness-dispatch-save":
    # The pre-review semver measurement saved its rust-cache on a dispatch.
    replace_once(
        ".github/workflows/release-semver-readiness.yml",
        "      - name: Install cargo-semver-checks\n",
        "      - name: Cache Cargo\n        uses: Swatinem/rust-cache@v2\n        with:\n"
        "          cache-bin: \"false\"\n\n      - name: Install cargo-semver-checks\n",
    )
elif name == "semver-gate-cache":
    # The pre-0.9.4 semver gate restored a rust-cache on its dispatch
    # measurement, whose target directory the build never used.
    replace_once(
        ".github/workflows/release.yml",
        "      - name: Install cargo-semver-checks\n",
        "      - name: Cache cargo\n        if: ${{ github.event_name == 'workflow_dispatch' }}\n"
        "        uses: Swatinem/rust-cache@v2\n        with:\n"
        "          save-if: ${{ github.event_name != 'workflow_dispatch' }}\n          cache-bin: \"false\"\n\n"
        "      - name: Install cargo-semver-checks\n",
    )
elif name == "semver-readiness-restore":
    # A restore-only rust-cache back in the readiness measurement: it saves
    # on no run, and its build never reads a target cache.
    replace_once(
        ".github/workflows/release-semver-readiness.yml",
        "      - name: Install cargo-semver-checks\n",
        "      - name: Cache Cargo\n        uses: Swatinem/rust-cache@v2\n        with:\n"
        "          save-if: \"false\"\n          cache-bin: \"false\"\n\n      - name: Install cargo-semver-checks\n",
    )
elif name == "ci-dispatch-save":
    # A CI lane that saves its rust-cache on a dispatch.
    path = root / ".github/workflows/ci.yml"
    text = path.read_text()
    job = text.index("\n  fmt-governance:\n")
    needle = "          save-if: ${{ github.event_name != 'workflow_dispatch' }}\n"
    at = text.index(needle, job)
    path.write_text(text[:at] + text[at + len(needle):])
elif name == "input-ref-dispatch-save":
    # A dispatch that checks out a ref its input chooses and saves a cache.
    (root / ".github/workflows/measure-ref.yml").write_text(
        "name: Measure a ref\n"
        "\n"
        "on:\n"
        "  workflow_dispatch:\n"
        "    inputs:\n"
        "      ref:\n"
        "        required: true\n"
        "        type: string\n"
        "\n"
        "permissions:\n"
        "  contents: read\n"
        "\n"
        "jobs:\n"
        "  measure:\n"
        "    runs-on: ubuntu-24.04\n"
        "    steps:\n"
        "      - name: Checkout\n"
        "        uses: actions/checkout@v6\n"
        "        with:\n"
        "          ref: ${{ inputs.ref }}\n"
        "\n"
        "      - name: Cache cargo\n"
        "        uses: Swatinem/rust-cache@v2\n"
        "        with:\n"
        "          shared-key: measure-ref\n"
    )
elif name == "trusted-workflow-missing":
    # A renamed semver readiness workflow leaves the trusted list stale.
    (root / ".github/workflows/release-semver-readiness.yml").unlink()
elif name == "nightly-secrets-inherit":
    # The pre-review nightly full graph passed buildbuddy.yml every
    # repository secret.
    path = root / ".github/workflows/nightly.yml"
    lines = path.read_text().splitlines(keepends=True)
    at = lines.index("    secrets:\n", lines.index("  gcp-buildbuddy:\n"))
    end = at + 1
    while end < len(lines) and lines[end].startswith("      "):
        end += 1
    if end == at + 1:
        raise SystemExit(f"fixture `{name}` found no secrets mapping in nightly.yml; update the fixture")
    path.write_text("".join(lines[:at] + ["    secrets: inherit\n"] + lines[end:]))
elif name == "cache-token-job-action":
    # A cache inside the local action a token job calls.
    replace_once(
        ".github/actions/update-homebrew/action.yml",
        "    - name: Checkout tap\n",
        "    - name: Cache tap\n"
        "      uses: actions/cache@v4\n"
        "      with:\n"
        "        path: homebrew-meerkat\n"
        "        key: homebrew-tap\n"
        "\n"
        "    - name: Checkout tap\n",
    )
else:
    raise SystemExit(f"unknown tree fixture `{name}`")
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

# expect_tree_fail_named <label> <root> <needle> <check...>: the mutated
# tree's release workflow fails the named check and names the defect.
expect_tree_fail_named() {
  local label="$1"
  local root="$2"
  local needle="$3"
  shift 3
  local log="${TEST_ROOT}/${label}.log"
  local status
  set +e
  "$PYTHON" "$CHECKER" --repo-root "$root" --workflow "${root}/.github/workflows/release.yml" "$@" >"$log" 2>&1
  status=$?
  set -e
  if [[ "$status" -eq 0 ]]; then
    fail "${label}: a tree that dropped the behaviour was accepted"
  fi
  if ! grep -Fq "$needle" "$log"; then
    fail "${label}: failure does not name the defect (expected: ${needle})" "$(cat "$log")"
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
mutate dispatch-reflowed "${TEST_ROOT}/dispatch-reflowed.yml"
expect_pass dispatch-reflowed "${TEST_ROOT}/dispatch-reflowed.yml" dispatch-binding
mutate dispatch-ref-name "${TEST_ROOT}/dispatch-ref-name.yml"
expect_pass dispatch-ref-name "${TEST_ROOT}/dispatch-ref-name.yml" dispatch-binding
mutate dispatch-ref-name-expression "${TEST_ROOT}/dispatch-ref-name-expression.yml"
expect_pass dispatch-ref-name-expression "${TEST_ROOT}/dispatch-ref-name-expression.yml" dispatch-binding
mutate tag-validation-reflowed "${TEST_ROOT}/tag-validation-reflowed.yml"
expect_pass tag-validation-reflowed "${TEST_ROOT}/tag-validation-reflowed.yml" tag-validation
mutate attestation-ref-reordered "${TEST_ROOT}/attestation-ref-reordered.yml"
expect_pass attestation-ref-reordered "${TEST_ROOT}/attestation-ref-reordered.yml" tag-validation
mutate ci-run-branches-reordered "${TEST_ROOT}/ci-run-branches-reordered.yml"
expect_pass ci-run-branches-reordered "${TEST_ROOT}/ci-run-branches-reordered.yml" tag-validation
mutate evidence-branch-reordered "${TEST_ROOT}/evidence-branch-reordered.yml"
expect_pass evidence-branch-reordered "${TEST_ROOT}/evidence-branch-reordered.yml" semver-evidence
mutate prebuild-lookup-if-reflowed "${TEST_ROOT}/prebuild-lookup-if-reflowed.yml"
expect_pass prebuild-lookup-if-reflowed "${TEST_ROOT}/prebuild-lookup-if-reflowed.yml" prebuild
mutate semver-baseline-needs-block-list "${TEST_ROOT}/semver-baseline-needs-block-list.yml"
expect_pass semver-baseline-needs-block-list "${TEST_ROOT}/semver-baseline-needs-block-list.yml" semver-baseline
mutate trailing-critical-if-reflowed "${TEST_ROOT}/trailing-critical-if-reflowed.yml"
expect_pass trailing-critical-if-reflowed "${TEST_ROOT}/trailing-critical-if-reflowed.yml" trailing-platforms

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

mutate dispatch-unbound "${TEST_ROOT}/dispatch-unbound.yml"
expect_fail_named dispatch-unbound "${TEST_ROOT}/dispatch-unbound.yml" \
  "does not run on a package dispatch from refs/heads/main without release_tag" dispatch-binding
mutate dispatch-head-unchecked "${TEST_ROOT}/dispatch-head-unchecked.yml"
expect_fail_named dispatch-head-unchecked "${TEST_ROOT}/dispatch-head-unchecked.yml" \
  "(the tag is not the checked-out commit)" dispatch-binding
mutate dispatch-alpha-unscoped "${TEST_ROOT}/dispatch-alpha-unscoped.yml"
expect_fail_named dispatch-alpha-unscoped "${TEST_ROOT}/dispatch-alpha-unscoped.yml" \
  "accepts a package dispatch on the alpha/v0.0.0 tag without alpha_crates_only" dispatch-binding
mutate dispatch-prefers-github-ref "${TEST_ROOT}/dispatch-prefers-github-ref.yml"
expect_fail_named dispatch-prefers-github-ref "${TEST_ROOT}/dispatch-prefers-github-ref.yml" \
  "accepts a package dispatch on the v0.0.0 tag naming release_tag main (a branch)" dispatch-binding
mutate dispatch-alpha-any-version "${TEST_ROOT}/dispatch-alpha-any-version.yml"
expect_fail_named dispatch-alpha-any-version "${TEST_ROOT}/dispatch-alpha-any-version.yml" \
  "accepts an alpha crate dispatch on the alpha/v9.9.9 tag (another version)" dispatch-binding
mutate dispatch-branch-ref-name "${TEST_ROOT}/dispatch-branch-ref-name.yml"
expect_fail_named dispatch-branch-ref-name "${TEST_ROOT}/dispatch-branch-ref-name.yml" \
  "accepts a package dispatch from refs/heads/v0.0.0 without release_tag" dispatch-binding
mutate dispatch-branch-ref-name-tag-exists "${TEST_ROOT}/dispatch-branch-ref-name-tag-exists.yml"
expect_fail_named dispatch-branch-ref-name-tag-exists "${TEST_ROOT}/dispatch-branch-ref-name-tag-exists.yml" \
  "accepts a package dispatch from refs/heads/v0.0.0 without release_tag" dispatch-binding
mutate dispatch-prefers-github-ref-other-tag "${TEST_ROOT}/dispatch-prefers-github-ref-other-tag.yml"
expect_fail_named dispatch-prefers-github-ref-other-tag "${TEST_ROOT}/dispatch-prefers-github-ref-other-tag.yml" \
  "(a tag on another commit)" dispatch-binding
mutate dispatch-prefers-github-ref-head-checked "${TEST_ROOT}/dispatch-prefers-github-ref-head-checked.yml"
expect_fail_named dispatch-prefers-github-ref-head-checked "${TEST_ROOT}/dispatch-prefers-github-ref-head-checked.yml" \
  "(both tags on one commit, no alpha lane)" dispatch-binding
mutate dispatch-prefix-glob "${TEST_ROOT}/dispatch-prefix-glob.yml"
expect_fail_named dispatch-prefix-glob "${TEST_ROOT}/dispatch-prefix-glob.yml" \
  "(a near miss of v0.0.0)" dispatch-binding
mutate dispatch-alpha-named-any-version "${TEST_ROOT}/dispatch-alpha-named-any-version.yml"
expect_fail_named dispatch-alpha-named-any-version "${TEST_ROOT}/dispatch-alpha-named-any-version.yml" \
  "accepts an alpha crate dispatch naming release_tag alpha/v9.9.9 (another version)" dispatch-binding
mutate dispatch-skips-named-tag-run "${TEST_ROOT}/dispatch-skips-named-tag-run.yml"
expect_fail_named dispatch-skips-named-tag-run "${TEST_ROOT}/dispatch-skips-named-tag-run.yml" \
  "does not run on a package dispatch on the v0.0.0 tag naming release_tag main (a branch)" dispatch-binding
mutate dispatch-checkout-github-ref "${TEST_ROOT}/dispatch-checkout-github-ref.yml"
expect_fail_named dispatch-checkout-github-ref "${TEST_ROOT}/dispatch-checkout-github-ref.yml" \
  "refuses a package dispatch naming release_tag v0.0.0" dispatch-binding
mutate dispatch-branch-name "${TEST_ROOT}/dispatch-branch-name.yml"
expect_fail_named dispatch-branch-name "${TEST_ROOT}/dispatch-branch-name.yml" \
  "accepts a package dispatch from refs/heads/release/v0.0.0 without release_tag" dispatch-binding

mutate tag-validation-cargo-on-attested-tags "${TEST_ROOT}/tag-validation-cargo-on-attested-tags.yml"
expect_fail_named tag-validation-cargo-on-attested-tags "${TEST_ROOT}/tag-validation-cargo-on-attested-tags.yml" \
  "runs on a tag push whose attestation covers the Cargo lanes (true)" tag-validation
mutate tag-validation-gate-any-schema "${TEST_ROOT}/tag-validation-gate-any-schema.yml"
expect_fail_named tag-validation-gate-any-schema "${TEST_ROOT}/tag-validation-gate-any-schema.yml" \
  "accepts a tag push whose attestation does not cover the Cargo lanes and Cargo validation skipped" tag-validation
mutate tag-validation-schema-unwritten "${TEST_ROOT}/tag-validation-schema-unwritten.yml"
expect_fail_named tag-validation-schema-unwritten "${TEST_ROOT}/tag-validation-schema-unwritten.yml" \
  "writes covers_release_validation=<nothing> where every lane ran" tag-validation
mutate tag-validation-ignores-sdk-host "${TEST_ROOT}/tag-validation-ignores-sdk-host.yml"
expect_fail_named tag-validation-ignores-sdk-host "${TEST_ROOT}/tag-validation-ignores-sdk-host.yml" \
  "writes covers_release_validation=true where the SDK suites were skipped" tag-validation

# A maintenance release is tagged on release/<version>: CI and semver
# evidence from that branch qualifies next to main's, and from no other.
mutate ci-run-main-only "${TEST_ROOT}/ci-run-main-only.yml"
expect_fail_named ci-run-main-only "${TEST_ROOT}/ci-run-main-only.yml" \
  "refuses the successful CI run of a push to release/0.0.0 (a maintenance release of the tag's version)" tag-validation
mutate ci-run-any-release-branch "${TEST_ROOT}/ci-run-any-release-branch.yml"
expect_fail_named ci-run-any-release-branch "${TEST_ROOT}/ci-run-any-release-branch.yml" \
  "accepts the successful CI run of a push to release/9.9.9 (another version's release branch)" tag-validation
mutate attestation-main-only "${TEST_ROOT}/attestation-main-only.yml"
expect_fail_named attestation-main-only "${TEST_ROOT}/attestation-main-only.yml" \
  "refuses the CI attestation of a push to release/0.0.0 (a maintenance release of the tag's version)" tag-validation
mutate attestation-any-release-branch "${TEST_ROOT}/attestation-any-release-branch.yml"
expect_fail_named attestation-any-release-branch "${TEST_ROOT}/attestation-any-release-branch.yml" \
  "accepts the CI attestation of a push to release/9.9.9 (another version's release branch)" tag-validation
mutate evidence-main-only "${TEST_ROOT}/evidence-main-only.yml"
expect_fail_named evidence-main-only "${TEST_ROOT}/evidence-main-only.yml" \
  "refuses the readiness evidence of a push to release/0.0.0 (a maintenance release of the tag's version)" semver-evidence
mutate evidence-any-release-branch "${TEST_ROOT}/evidence-any-release-branch.yml"
expect_fail_named evidence-any-release-branch "${TEST_ROOT}/evidence-any-release-branch.yml" \
  "accepts the readiness evidence of a release/9.9.9 attestation whose run ran on main" semver-evidence
mutate evidence-run-any-branch "${TEST_ROOT}/evidence-run-any-branch.yml"
expect_fail_named evidence-run-any-branch "${TEST_ROOT}/evidence-run-any-branch.yml" \
  "accepts the readiness evidence of a main-push attestation whose run ran on release/9.9.9" semver-evidence
mutate evidence-pick-any-branch "${TEST_ROOT}/evidence-pick-any-branch.yml"
expect_fail_named evidence-pick-any-branch "${TEST_ROOT}/evidence-pick-any-branch.yml" \
  "refuses the readiness evidence of a push to main once a push of the same tree to release/9.9.9 (another version's release branch) uploaded newer evidence" semver-evidence

mutate prebuild-publishes "${TEST_ROOT}/prebuild-publishes.yml"
expect_fail_named prebuild-publishes "${TEST_ROOT}/prebuild-publishes.yml" \
  "job \`publish_github_release\` publishes on a prebuild dispatch on main" prebuild
mutate prebuild-moving-checkout "${TEST_ROOT}/prebuild-moving-checkout.yml"
expect_fail_named prebuild-moving-checkout "${TEST_ROOT}/prebuild-moving-checkout.yml" \
  "job \`build_web_sdk_package\` checks out \`refs/heads/main\` on a prebuild dispatch on main" prebuild
mutate prebuild-builds-anyway "${TEST_ROOT}/prebuild-builds-anyway.yml"
expect_fail_named prebuild-builds-anyway "${TEST_ROOT}/prebuild-builds-anyway.yml" \
  "job \`build_binaries_windows_cross\` builds on a tag push with a prebuild found" prebuild
mutate prebuild-reuse-unverified "${TEST_ROOT}/prebuild-reuse-unverified.yml"
expect_fail_named prebuild-reuse-unverified "${TEST_ROOT}/prebuild-reuse-unverified.yml" \
  "expected \`scripts/verify-release-asset-provenance --source-run 424242 --source-attempt 2 --prebuild --stamps ...\`" prebuild
mutate prebuild-web-unverified "${TEST_ROOT}/prebuild-web-unverified.yml"
expect_fail_named prebuild-web-unverified "${TEST_ROOT}/prebuild-web-unverified.yml" \
  "must verify the prebuilt package's attestation" prebuild
mutate prebuild-reuse-no-wait "${TEST_ROOT}/prebuild-reuse-no-wait.yml"
expect_fail_named prebuild-reuse-no-wait "${TEST_ROOT}/prebuild-reuse-no-wait.yml" \
  "must wait for the prebuild run's \`Release binary build gate\`" prebuild
mutate prebuild-gate-waits-trailing "${TEST_ROOT}/prebuild-gate-waits-trailing.yml"
expect_fail_named prebuild-gate-waits-trailing "${TEST_ROOT}/prebuild-gate-waits-trailing.yml" \
  "job \`release_prebuild_gate\` waits for \`build_binaries_gate\`" trailing-platforms
mutate prebuild-critical-full-gate "${TEST_ROOT}/prebuild-critical-full-gate.yml"
expect_fail_named prebuild-critical-full-gate "${TEST_ROOT}/prebuild-critical-full-gate.yml" \
  "expected \`Release critical binary build gate\`" prebuild
mutate prebuild-web-ungated "${TEST_ROOT}/prebuild-web-ungated.yml"
expect_fail_named prebuild-web-ungated "${TEST_ROOT}/prebuild-web-ungated.yml" \
  "job \`publish_web_sdk\` publishes on a tag push with a prebuild found and \`release_validate_gate\` failed" prebuild
mutate prebuild-any-backend "${TEST_ROOT}/prebuild-any-backend.yml"
expect_fail_named prebuild-any-backend "${TEST_ROOT}/prebuild-any-backend.yml" \
  "accepts a prebuild by lukacf with release_backend=github-hosted (MEERKAT_RELEASE_BUILDBUDDY true), whose tag run builds on buildbuddy" prebuild
mutate prebuild-backend-fixed "${TEST_ROOT}/prebuild-backend-fixed.yml"
expect_fail_named prebuild-backend-fixed "${TEST_ROOT}/prebuild-backend-fixed.yml" \
  "refuses a prebuild by lukacf with release_backend=github-hosted (MEERKAT_RELEASE_BUILDBUDDY unset), whose tag run builds on github-hosted" prebuild
mutate prebuild-gate-renamed "${TEST_ROOT}/prebuild-gate-renamed.yml"
expect_fail_named prebuild-gate-renamed "${TEST_ROOT}/prebuild-gate-renamed.yml" \
  "does not require the \`Prebuild complete\` job" prebuild

mutate semver-baseline-gates-homebrew "${TEST_ROOT}/semver-baseline-gates-homebrew.yml"
expect_fail_named semver-baseline-gates-homebrew "${TEST_ROOT}/semver-baseline-gates-homebrew.yml" \
  "job \`update_homebrew\` waits for the semver baseline rustdoc" semver-baseline
mutate semver-baseline-clobber "${TEST_ROOT}/semver-baseline-clobber.yml"
expect_fail_named semver-baseline-clobber "${TEST_ROOT}/semver-baseline-clobber.yml" \
  "job \`attach_semver_baseline\` attaches with \`--clobber\`" semver-baseline
expect_fail_named semver-baseline-clobber-add-only "${TEST_ROOT}/semver-baseline-clobber.yml" \
  "step \`Attach baseline rustdoc to the GitHub release\` uploads with \`gh release upload --clobber\`" trailing-platforms
mutate semver-baseline-after-release "${TEST_ROOT}/semver-baseline-after-release.yml"
expect_fail_named semver-baseline-after-release "${TEST_ROOT}/semver-baseline-after-release.yml" \
  "the rustdoc build must run in parallel with publication, not after it" semver-baseline

mutate trailing-gates-critical "${TEST_ROOT}/trailing-gates-critical.yml"
expect_fail_named trailing-gates-critical "${TEST_ROOT}/trailing-gates-critical.yml" \
  "waits for \`build_binaries_gate\`, \`build_binaries_macos_x86_cross\`" trailing-platforms
mutate trailing-overwrites "${TEST_ROOT}/trailing-overwrites.yml"
expect_fail_named trailing-overwrites "${TEST_ROOT}/trailing-overwrites.yml" \
  "release uploads are add-only" trailing-platforms
mutate trailing-manifest-replaced "${TEST_ROOT}/trailing-manifest-replaced.yml"
expect_fail_named trailing-manifest-replaced "${TEST_ROOT}/trailing-manifest-replaced.yml" \
  "writes the manifest unconditionally" trailing-platforms
mutate trailing-selects-intel "${TEST_ROOT}/trailing-selects-intel.yml"
expect_fail_named trailing-selects-intel "${TEST_ROOT}/trailing-selects-intel.yml" \
  "rkat-0.0.0-x86_64-apple-darwin.tar.gz" trailing-platforms

mutate token-write-default "${TEST_ROOT}/token-write-default.yml"
expect_fail_named token-write-default "${TEST_ROOT}/token-write-default.yml" \
  "the workflow default grants \`contents: write\` to every job" token-permissions
mutate token-publisher-persisted "${TEST_ROOT}/token-publisher-persisted.yml"
expect_fail_named token-publisher-persisted "${TEST_ROOT}/token-publisher-persisted.yml" \
  "job \`publish_github_release\` step \`Checkout workflow helpers\` persists the workflow token in its checkout" token-permissions
mutate secrets-inherit "${TEST_ROOT}/secrets-inherit.yml"
expect_fail_named secrets-inherit "${TEST_ROOT}/secrets-inherit.yml" \
  "job \`release_validate_buildbuddy_full\` passes every repository secret to a reusable workflow" token-permissions
mutate_tree nightly-secrets-inherit "${TEST_ROOT}/nightly-secrets-inherit"
expect_tree_fail_named nightly-secrets-inherit "${TEST_ROOT}/nightly-secrets-inherit" \
  "nightly.yml job \`gcp-buildbuddy\` passes every repository secret to a reusable workflow" token-permissions

mutate web-sdk-pack-mode-dropped "${TEST_ROOT}/web-sdk-pack-mode-dropped.yml"
expect_fail_named web-sdk-pack-mode-dropped "${TEST_ROOT}/web-sdk-pack-mode-dropped.yml" \
  "step \`Build and pack Web SDK\` lets wasm-pack download its own wasm-bindgen and wasm-opt" web-sdk-tools
mutate web-sdk-bindgen-mismatch "${TEST_ROOT}/web-sdk-bindgen-mismatch.yml"
expect_fail_named web-sdk-bindgen-mismatch "${TEST_ROOT}/web-sdk-bindgen-mismatch.yml" \
  "job \`build_web_sdk_package\` pins wasm-bindgen 0.0.0-stale, but Cargo.lock locks" web-sdk-tools
mutate web-sdk-unchecked-download "${TEST_ROOT}/web-sdk-unchecked-download.yml"
expect_fail_named web-sdk-unchecked-download "${TEST_ROOT}/web-sdk-unchecked-download.yml" \
  "step \`Install pinned wasm-bindgen and wasm-opt\` downloads 2 file(s) but checks 1 sha256" web-sdk-tools

mutate release-job-cache "${TEST_ROOT}/release-job-cache.yml"
expect_fail_named release-job-cache "${TEST_ROOT}/release-job-cache.yml" \
  "job \`build_binaries\` restores \`v0-rust-build_binaries\` on a dispatch; a release job restores no cache" release-caches
mutate cache-registry-restore "${TEST_ROOT}/cache-registry-restore.yml"
expect_fail_named cache-registry-restore "${TEST_ROOT}/cache-registry-restore.yml" \
  "job \`publish_registries\` holds a registry or publish token and restores a cache" release-caches
mutate cache-dispatch-save "${TEST_ROOT}/cache-dispatch-save.yml"
expect_fail_named cache-dispatch-save "${TEST_ROOT}/cache-dispatch-save.yml" \
  "job \`build_binaries_windows_cross\` saves its rust-cache on a dispatch" release-caches
mutate cache-xwin-dispatch-save "${TEST_ROOT}/cache-xwin-dispatch-save.yml"
expect_fail_named cache-xwin-dispatch-save "${TEST_ROOT}/cache-xwin-dispatch-save.yml" \
  "job \`build_binaries_windows_cross\` (in \`.github/actions/build-windows-cross\`) saves cache \`xwin-sdk-*-cargo-xwin-0.23.1\` on a dispatch" release-caches
mutate cache-buildbuddy-repository "${TEST_ROOT}/cache-buildbuddy-repository.yml"
expect_fail_named cache-buildbuddy-repository "${TEST_ROOT}/cache-buildbuddy-repository.yml" \
  "job \`build_binaries_buildbuddy\` (in \`.github/actions/setup-buildbuddy-ci\`) saves cache \`buildbuddy-bazel-repository-*-*\` on a dispatch" release-caches
expect_fail_named cache-buildbuddy-repository-restore "${TEST_ROOT}/cache-buildbuddy-repository.yml" \
  "job \`build_binaries_buildbuddy\` (in \`.github/actions/setup-buildbuddy-ci\`) restores \`buildbuddy-bazel-repository-*-*\` on a tag push; a release job restores no cache" release-caches
mutate reusable-tag-repository-restore "${TEST_ROOT}/reusable-tag-repository-restore.yml"
expect_fail_named reusable-tag-repository-restore "${TEST_ROOT}/reusable-tag-repository-restore.yml" \
  "job \`release_validate_buildbuddy_full\` (through buildbuddy.yml job \`prebuild-submit\` in \`.github/actions/run-buildbuddy-ci-lane-batch\` > \`.github/actions/setup-buildbuddy-ci\`) restores \`buildbuddy-bazel-repository-*-*\` on a tag push; a release job restores no cache" release-caches
mutate reusable-tag-unreadable "${TEST_ROOT}/reusable-tag-unreadable.yml"
expect_fail_named reusable-tag-unreadable "${TEST_ROOT}/reusable-tag-unreadable.yml" \
  "job \`release_validate_buildbuddy_full\` calls a reusable workflow on a tag that this check cannot read" release-caches
mutate reusable-on-dispatch "${TEST_ROOT}/reusable-on-dispatch.yml"
expect_fail_named reusable-on-dispatch "${TEST_ROOT}/reusable-on-dispatch.yml" \
  "job \`release_validate_buildbuddy_full\` calls a reusable workflow on a dispatch" release-caches
mutate_tree cache-xwin-save-ungated "${TEST_ROOT}/cache-xwin-save-ungated"
expect_tree_fail_named cache-xwin-save-ungated "${TEST_ROOT}/cache-xwin-save-ungated" \
  "job \`build_binaries_windows_cross\` (in \`.github/actions/build-windows-cross\`) saves cache \`xwin-sdk-*-cargo-xwin-0.23.1\` on a dispatch" release-caches
mutate_tree cache-xwin-validate-dispatch-save "${TEST_ROOT}/cache-xwin-validate-dispatch-save"
expect_tree_fail_named cache-xwin-validate-dispatch-save "${TEST_ROOT}/cache-xwin-validate-dispatch-save" \
  "restores \`xwin-sdk-*-cargo-xwin-0.23.1\`, which windows-cross-validate.yml job \`build_windows_cross\` (in \`.github/actions/build-windows-cross\`) saves on a workflow_dispatch run" release-caches
mutate_tree cache-dispatch-shared-registry "${TEST_ROOT}/cache-dispatch-shared-registry"
expect_tree_fail_named cache-dispatch-shared-registry "${TEST_ROOT}/cache-dispatch-shared-registry" \
  "job \`build_web_sdk_package\` restores \`v0-rust-ci-registry\`, which registry-dispatch.yml job \`lint\` saves on a workflow_dispatch run" release-caches
mutate_tree dispatch-saves-repository-cache "${TEST_ROOT}/dispatch-saves-repository-cache"
expect_tree_fail_named dispatch-saves-repository-cache "${TEST_ROOT}/dispatch-saves-repository-cache" \
  "release-binaries-check.yml job \`build\` (in \`.github/actions/setup-buildbuddy-ci\`) saves \`buildbuddy-bazel-repository-*-*\` on a workflow_dispatch run; such runs only restore the Bazel repository cache" release-caches
mutate_tree xwin-tag-restore "${TEST_ROOT}/xwin-tag-restore"
expect_tree_fail_named xwin-tag-restore "${TEST_ROOT}/xwin-tag-restore" \
  "job \`build_binaries_windows_cross\` (in \`.github/actions/build-windows-cross\`) restores \`xwin-sdk-*-cargo-xwin-0.23.1\` on a tag push; a release job restores no cache" release-caches
mutate_tree sccache-pull-request-writes "${TEST_ROOT}/sccache-pull-request-writes"
expect_tree_fail_named sccache-pull-request-writes "${TEST_ROOT}/sccache-pull-request-writes" \
  ".github/actions/setup-sccache-ci gives a pull_request run on refs/pull/1/merge sccache mode READ_WRITE, not READ_ONLY" release-caches
mutate_tree sccache-dispatch-writes-any-ref "${TEST_ROOT}/sccache-dispatch-writes-any-ref"
expect_tree_fail_named sccache-dispatch-writes-any-ref "${TEST_ROOT}/sccache-dispatch-writes-any-ref" \
  ".github/actions/setup-sccache-ci gives a workflow_dispatch run on refs/heads/main sccache mode READ_WRITE, not READ_ONLY" release-caches
mutate_tree sccache-dispatch-ref-name-prefix "${TEST_ROOT}/sccache-dispatch-ref-name-prefix"
expect_tree_fail_named sccache-dispatch-ref-name-prefix "${TEST_ROOT}/sccache-dispatch-ref-name-prefix" \
  ".github/actions/setup-sccache-ci gives a workflow_dispatch run on refs/heads/rel-next sccache mode READ_WRITE, not READ_ONLY" release-caches
mutate_tree sccache-release-dispatch-reads "${TEST_ROOT}/sccache-release-dispatch-reads"
expect_tree_fail_named sccache-release-dispatch-reads "${TEST_ROOT}/sccache-release-dispatch-reads" \
  ".github/actions/setup-sccache-ci gives a workflow_dispatch run on refs/heads/rel/next sccache mode READ_ONLY, not READ_WRITE" release-caches
mutate_tree sccache-dispatch-ref-name-slash "${TEST_ROOT}/sccache-dispatch-ref-name-slash"
expect_tree_fail_named sccache-dispatch-ref-name-slash "${TEST_ROOT}/sccache-dispatch-ref-name-slash" \
  ".github/actions/setup-sccache-ci gives a workflow_dispatch run on refs/tags/rel/next sccache mode READ_WRITE, not READ_ONLY" release-caches
mutate_tree sccache-action-install "${TEST_ROOT}/sccache-action-install"
expect_tree_fail_named sccache-action-install "${TEST_ROOT}/sccache-action-install" \
  ".github/actions/setup-sccache-ci installs sccache through sccache-action, which exports the job's Actions cache token to the run steps after it" release-caches
mutate_tree sccache-unchecked-install "${TEST_ROOT}/sccache-unchecked-install"
expect_tree_fail_named sccache-unchecked-install "${TEST_ROOT}/sccache-unchecked-install" \
  ".github/actions/setup-sccache-ci installs no sccache release checked against a pinned sha256 (SCCACHE_VERSION, SCCACHE_SHA256, sha256sum --check --strict -)" release-caches
mutate_tree semver-readiness-dispatch-save "${TEST_ROOT}/semver-readiness-dispatch-save"
expect_tree_fail_named semver-readiness-dispatch-save "${TEST_ROOT}/semver-readiness-dispatch-save" \
  "release-semver-readiness.yml job \`semver\` saves a cache on a workflow_dispatch run" release-caches
mutate_tree semver-gate-cache "${TEST_ROOT}/semver-gate-cache"
expect_tree_fail_named semver-gate-cache "${TEST_ROOT}/semver-gate-cache" \
  "job \`release_semver_gate\` restores \`v0-rust-release_semver_gate\` on a dispatch; a release job restores no cache" release-caches
mutate_tree semver-readiness-restore "${TEST_ROOT}/semver-readiness-restore"
expect_tree_fail_named semver-readiness-restore "${TEST_ROOT}/semver-readiness-restore" \
  "release-semver-readiness.yml job \`semver\` restores \`v0-rust-semver\` on a push run; the semver measurement restores no cache" release-caches
mutate_tree ci-dispatch-save "${TEST_ROOT}/ci-dispatch-save"
expect_tree_fail_named ci-dispatch-save "${TEST_ROOT}/ci-dispatch-save" \
  "ci.yml job \`fmt-governance\` saves a cache on a workflow_dispatch run" release-caches
mutate_tree input-ref-dispatch-save "${TEST_ROOT}/input-ref-dispatch-save"
expect_tree_fail_named input-ref-dispatch-save "${TEST_ROOT}/input-ref-dispatch-save" \
  "measure-ref.yml job \`measure\` saves a cache on a workflow_dispatch run (\`Swatinem/rust-cache@v2\`); a workflow that checks out a ref its inputs choose saves none on such a run" release-caches
mutate_tree trusted-workflow-missing "${TEST_ROOT}/trusted-workflow-missing"
expect_tree_fail_named trusted-workflow-missing "${TEST_ROOT}/trusted-workflow-missing" \
  "release-semver-readiness.yml, a workflow whose runs a tag trusts, is missing" release-caches
mutate_tree cache-token-job-action "${TEST_ROOT}/cache-token-job-action"
expect_tree_fail_named cache-token-job-action "${TEST_ROOT}/cache-token-job-action" \
  "job \`update_homebrew\` (in \`.github/actions/update-homebrew\`) holds a registry or publish token and restores a cache" release-caches

mutate readback-step-removed "${TEST_ROOT}/readback-step-removed.yml"
expect_fail_named readback-step-removed "${TEST_ROOT}/readback-step-removed.yml" \
  "no step that reads every crate back" registry-slo
mutate slo-before-sdk "${TEST_ROOT}/slo-before-sdk.yml"
expect_fail_named slo-before-sdk "${TEST_ROOT}/slo-before-sdk.yml" \
  "SDK publication must not wait on the publication SLO" registry-slo

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
for line in "$SEMVER_PASS" "$SLO_PASS" "$DISPATCH_PASS" "$TAG_VALIDATION_PASS" "$PREBUILD_PASS" "$BASELINE_PASS" "$TRAILING_PASS" "$WINDOWS_PASS" "$TOKEN_PASS" "$WEB_TOOLS_PASS" "$CACHES_PASS"; do
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

mutate dispatch-unbound "${TEST_ROOT}/doctor-dispatch-unbound.yml"
run_doctor "${TEST_ROOT}/doctor-dispatch-unbound.yml" "${TEST_ROOT}/doctor-dispatch.log"
if ! grep -Fq "$DISPATCH_FAIL" "${TEST_ROOT}/doctor-dispatch.log"; then
  fail "doctor does not report \`${DISPATCH_FAIL}\` for a workflow whose branch dispatch is unbound" \
    "$(cat "${TEST_ROOT}/doctor-dispatch.log")"
fi

mutate tag-validation-cargo-on-attested-tags "${TEST_ROOT}/doctor-tag-validation.yml"
run_doctor "${TEST_ROOT}/doctor-tag-validation.yml" "${TEST_ROOT}/doctor-tag-validation.log"
if ! grep -Fq "$TAG_VALIDATION_FAIL" "${TEST_ROOT}/doctor-tag-validation.log"; then
  fail "doctor does not report \`${TAG_VALIDATION_FAIL}\` for a workflow that re-runs Cargo validation on attested tags" \
    "$(cat "${TEST_ROOT}/doctor-tag-validation.log")"
fi

mutate prebuild-publishes "${TEST_ROOT}/doctor-prebuild.yml"
run_doctor "${TEST_ROOT}/doctor-prebuild.yml" "${TEST_ROOT}/doctor-prebuild.log"
if ! grep -Fq "$PREBUILD_FAIL" "${TEST_ROOT}/doctor-prebuild.log"; then
  fail "doctor does not report \`${PREBUILD_FAIL}\` for a workflow whose prebuild publishes" \
    "$(cat "${TEST_ROOT}/doctor-prebuild.log")"
fi

mutate semver-baseline-gates-homebrew "${TEST_ROOT}/doctor-baseline.yml"
run_doctor "${TEST_ROOT}/doctor-baseline.yml" "${TEST_ROOT}/doctor-baseline.log"
if ! grep -Fq "$BASELINE_FAIL" "${TEST_ROOT}/doctor-baseline.log"; then
  fail "doctor does not report \`${BASELINE_FAIL}\` for a workflow that waits for the baseline" \
    "$(cat "${TEST_ROOT}/doctor-baseline.log")"
fi

mutate trailing-gates-critical "${TEST_ROOT}/doctor-trailing.yml"
run_doctor "${TEST_ROOT}/doctor-trailing.yml" "${TEST_ROOT}/doctor-trailing.log"
if ! grep -Fq "$TRAILING_FAIL" "${TEST_ROOT}/doctor-trailing.log"; then
  fail "doctor does not report \`${TRAILING_FAIL}\` for a workflow whose critical assets wait for the trailing lanes" \
    "$(cat "${TEST_ROOT}/doctor-trailing.log")"
fi

mutate windows-verify-moving-image "${TEST_ROOT}/doctor-windows.yml"
run_doctor "${TEST_ROOT}/doctor-windows.yml" "${TEST_ROOT}/doctor-windows.log"
if ! grep -Fxq "$WINDOWS_FAIL" "${TEST_ROOT}/doctor-windows.log"; then
  fail "doctor does not report \`${WINDOWS_FAIL}\` for a workflow that verifies the Windows binaries on windows-latest" \
    "$(cat "${TEST_ROOT}/doctor-windows.log")"
fi

mutate cache-xwin-dispatch-save "${TEST_ROOT}/doctor-caches.yml"
run_doctor "${TEST_ROOT}/doctor-caches.yml" "${TEST_ROOT}/doctor-caches.log"
if ! grep -Fq "$CACHES_FAIL" "${TEST_ROOT}/doctor-caches.log"; then
  fail "doctor does not report \`${CACHES_FAIL}\` for a workflow whose Windows SDK cache is saved by a dispatch" \
    "$(cat "${TEST_ROOT}/doctor-caches.log")"
fi

echo "release doctor workflow contract holds"
