#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-ci-nextest-archive.XXXXXX")"
trap 'rm -rf -- "$TEST_ROOT"' EXIT

LOG="${TEST_ROOT}/commands.log"
FAKE_CARGO="${TEST_ROOT}/repo-cargo"
FAKE_NEXTEST="${TEST_ROOT}/cargo-nextest"

cat >"$FAKE_CARGO" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'cargo' >>"$COMMAND_LOG"
printf ' <%s>' "$@" >>"$COMMAND_LOG"
printf '\n' >>"$COMMAND_LOG"
archive_file=""
while (($#)); do
  if [[ "$1" == "--archive-file" ]]; then
    archive_file="$2"
    break
  fi
  shift
done
[[ -n "$archive_file" ]] || exit 91
printf 'archive\n' >"$archive_file"
EOF

cat >"$FAKE_NEXTEST" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "${MEERKAT_WORKSPACE_ROOT:-}" == "${EXPECTED_WORKSPACE_ROOT:?}" ]] || {
  echo "archived run omitted MEERKAT_WORKSPACE_ROOT" >&2
  exit 92
}
printf 'nextest' >>"$COMMAND_LOG"
printf ' <%s>' "$@" >>"$COMMAND_LOG"
printf '\n' >>"$COMMAND_LOG"
EOF

chmod +x "$FAKE_CARGO" "$FAKE_NEXTEST"
export COMMAND_LOG="$LOG"
export EXPECTED_WORKSPACE_ROOT="$ROOT"
export ROOT
export CARGO="$FAKE_CARGO"
export NEXTEST_BIN="$FAKE_NEXTEST"

families=(unit unit-mob int-heavy int-mob int-everything-else)
for family in "${families[@]}"; do
  archive="${TEST_ROOT}/${family}.tar.zst"
  "$ROOT/scripts/ci-nextest-archive.sh" build "$family" "$archive"
  "$ROOT/scripts/ci-nextest-archive.sh" run "$family" "$archive" hash:1/1
done
NEXTEST_PROFILE_OVERRIDE=mob-dense-topology \
  NEXTEST_RUN_IGNORED=all \
  "$ROOT/scripts/ci-nextest-archive.sh" run \
  unit-mob \
  "${TEST_ROOT}/unit-mob.tar.zst" \
  hash:1/1

assert_line_contains() {
  local pattern="$1"
  if ! grep -F -- "$pattern" "$LOG" >/dev/null; then
    echo "missing command fragment: ${pattern}" >&2
    cat "$LOG" >&2
    exit 1
  fi
}

assert_line_contains 'cargo <nextest> <archive> <--workspace> <--lib>'
assert_line_contains 'cargo <nextest> <archive> <-p> <meerkat-integration-tests> <--tests> <--profile> <fast>'
assert_line_contains '<-p> <meerkat-mob> <-p> <meerkat-mob-adaptive>'
assert_line_contains '<--workspace> <--exclude> <meerkat-integration-tests> <--exclude> <meerkat-mob>'

run_count="$(grep -c '^nextest ' "$LOG")"
[[ "$run_count" == 6 ]] || {
  echo "expected six archived runs, got ${run_count}" >&2
  exit 1
}
while IFS= read -r command; do
  [[ "$command" == *' <--no-tests=fail> '* ]] || {
    echo "archived run omitted --no-tests=fail: ${command}" >&2
    exit 1
  }
  # This proves that every archived execution passes Nextest's relocation
  # flag. Compile-time CARGO_MANIFEST_DIR reads can still embed the builder
  # checkout, so the count ratchet below prevents that known gap from growing
  # until the dedicated portability cleanup removes it.
  [[ "$command" == *' <--workspace-remap> '* ]] || {
    echo "archived run omitted --workspace-remap: ${command}" >&2
    exit 1
  }
  [[ "$command" == *' <--partition> <hash:1/1>'* ]] || {
    echo "archived run omitted the requested partition: ${command}" >&2
    exit 1
  }
done < <(grep '^nextest ' "$LOG")

unit_run="$(grep '^nextest ' "$LOG" | head -n 1)"
[[ "$unit_run" == *' <--profile> <ci-unit>'* ]] || {
  echo "unit archive run omitted the bounded ci-unit profile" >&2
  exit 1
}
[[ "$unit_run" == *' <--final-status-level> <fail>'* ]] || {
  echo "unit archive run changed its failure-only final status" >&2
  exit 1
}
[[ "$unit_run" == *' <--status-level> <none>'* ]] || {
  echo "unit archive run changed its quiet streaming status" >&2
  exit 1
}
ci_unit_run_count="$(grep '^nextest ' "$LOG" | grep -c -- '<--profile> <ci-unit>')"
[[ "$ci_unit_run_count" == 2 ]] || {
  echo "expected two ci-unit archive runs, got ${ci_unit_run_count}" >&2
  exit 1
}
dense_run="$(grep '^nextest ' "$LOG" | tail -n 1)"
[[ "$dense_run" == *' <--profile> <mob-dense-topology>'* ]] || {
  echo "dense archive run omitted its isolated profile" >&2
  exit 1
}
[[ "$dense_run" == *' <--run-ignored> <all>'* ]] || {
  echo "dense archive run did not opt the Linux test back in" >&2
  exit 1
}
[[ "$dense_run" == *' <--status-level> <slow>'* ]] || {
  echo "dense archive run suppressed named slow-test reports" >&2
  exit 1
}
[[ "$dense_run" == *' <--final-status-level> <slow>'* ]] || {
  echo "dense archive run suppressed its slow-test summary" >&2
  exit 1
}
fast_build_count="$(grep '^cargo ' "$LOG" | grep -c -- '<--profile> <fast>')"
[[ "$fast_build_count" == 3 ]] || {
  echo "expected three fast-profile integration archive builds, got ${fast_build_count}" >&2
  exit 1
}
fast_run_count="$(grep '^nextest ' "$LOG" | grep -c -- '<--profile> <fast>')"
[[ "$fast_run_count" == 3 ]] || {
  echo "expected three fast-profile integration archive runs, got ${fast_run_count}" >&2
  exit 1
}
while IFS= read -r command; do
  [[ "$command" == *' <--status-level> <slow>'* ]] || {
    echo "integration archive run suppressed streaming slow-test markers: ${command}" >&2
    exit 1
  }
  [[ "$command" == *' <--final-status-level> <slow>'* ]] || {
    echo "integration archive run suppressed its slow-test summary: ${command}" >&2
    exit 1
  }
done < <(grep '^nextest ' "$LOG" | grep -- '<--profile> <fast>')

if "$ROOT/scripts/ci-nextest-archive.sh" build unknown "${TEST_ROOT}/unknown.tar.zst" >/dev/null 2>&1; then
  echo "unknown archive family was accepted" >&2
  exit 1
fi
if "$ROOT/scripts/ci-nextest-archive.sh" run unit "${TEST_ROOT}/unit.tar.zst" >/dev/null 2>&1; then
  echo "archive run without a partition was accepted" >&2
  exit 1
fi

BUILD_ACTION="$ROOT/.github/actions/build-nextest-archive/action.yml"
RUN_ACTION="$ROOT/.github/actions/run-nextest-archive/action.yml"
WORKFLOW="$ROOT/.github/workflows/cargo.yml"
TOP_LEVEL_WORKFLOW="$ROOT/.github/workflows/ci.yml"
DENSE_WORKFLOW="$ROOT/.github/workflows/mob-dense-topology.yml"
RELEASE_WORKFLOW="$ROOT/.github/workflows/release.yml"
NEXTEST_CONFIG="$ROOT/.config/nextest.toml"

assert_file_contains() {
  local file="$1"
  local pattern="$2"
  if ! grep -F -- "$pattern" "$file" >/dev/null; then
    echo "${file} is missing required contract: ${pattern}" >&2
    exit 1
  fi
}

stable_artifact_name="nextest-\${{ inputs.family }}-\${{ github.sha }}"
assert_file_contains "$BUILD_ACTION" "name: ${stable_artifact_name}"
assert_file_contains "$BUILD_ACTION" 'overwrite: true'
assert_file_contains "$BUILD_ACTION" "if: \${{ inputs.publish == 'true' }}"
assert_file_contains "$BUILD_ACTION" 'value: ${{ steps.archive.outputs.path }}'
assert_file_contains "$RUN_ACTION" "name: ${stable_artifact_name}"
assert_file_contains "$RUN_ACTION" 'uses: ./.github/actions/setup-rust-ci'
assert_file_contains "$RUN_ACTION" 'components: rustfmt'
assert_file_contains "$RUN_ACTION" 'scripts/ci-nextest-archive.sh run'
assert_file_contains "$RUN_ACTION" 'NEXTEST_PROFILE_OVERRIDE: ${{ inputs.profile }}'
assert_file_contains "$RUN_ACTION" 'NEXTEST_RUN_IGNORED: ${{ inputs.run_ignored }}'
assert_file_contains "$NEXTEST_CONFIG" '[profile.ci-unit]'
assert_file_contains "$NEXTEST_CONFIG" 'inherits = "default"'
assert_file_contains "$NEXTEST_CONFIG" 'slow-timeout = { period = "60s", terminate-after = 4 }'
assert_file_contains "$NEXTEST_CONFIG" 'filter = '\''test(=machines::tests::machine_workflow_red_ok_detects_missing_and_stale_generated_artifacts)'\'''
assert_file_contains "$NEXTEST_CONFIG" 'slow-timeout = { period = "60s", terminate-after = 8 }'
assert_file_contains "$NEXTEST_CONFIG" '[profile.mob-dense-topology]'
assert_file_contains "$NEXTEST_CONFIG" 'default-filter = '\''package(meerkat-mob) and test(=runtime::tests::test_wire_members_batch_materializes_300_by_150_dense_topology_in_seconds)'\'''
assert_file_contains "$NEXTEST_CONFIG" 'slow-timeout = { period = "60s", terminate-after = 8 }'
fast_profile="$(sed -n '/^\[profile.fast\]$/,/^\[/p' "$NEXTEST_CONFIG")"
[[ "$fast_profile" == *'slow-timeout = { period = "60s" }'* ]] || {
  echo "fast profile must report slow tests every 60 seconds" >&2
  exit 1
}
[[ "$fast_profile" != *'terminate-after'* ]] || {
  echo "fast observation profile must not terminate tests" >&2
  exit 1
}

for dependency in unit-archive int-archives; do
  assert_file_contains "$WORKFLOW" "      - ${dependency}"
  assert_file_contains "$WORKFLOW" "            \${{ needs.${dependency}.result }}"
done
for execution_job in unit int-mob int-else int-rest; do
  assert_file_contains "$WORKFLOW" "      - ${execution_job}"
  assert_file_contains "$WORKFLOW" "            \${{ needs.${execution_job}.result }}"
done

dense_job="$(sed -n '/^  dense-topology:$/,/^  int-archives:$/p' "$WORKFLOW")"
for contract in \
  '      - unit-archive' \
  '          profile: mob-dense-topology' \
  '          run_ignored: all'; do
  [[ "$dense_job" == *"$contract"* ]] || {
    echo "Cargo dense-topology job is missing contract: ${contract}" >&2
    exit 1
  }
done
assert_file_contains "$WORKFLOW" '      - dense-topology'
assert_file_contains "$WORKFLOW" '${{ needs.dense-topology.result }}'
assert_file_contains "$DENSE_WORKFLOW" '  workflow_call:'
assert_file_contains "$DENSE_WORKFLOW" '          family: unit-mob'
assert_file_contains "$DENSE_WORKFLOW" '    timeout-minutes: 10'
assert_file_contains "$DENSE_WORKFLOW" '          profile: mob-dense-topology'
assert_file_contains "$DENSE_WORKFLOW" '          run_ignored: all'
assert_file_contains "$TOP_LEVEL_WORKFLOW" '  github-hosted-dense-topology:'
assert_file_contains "$TOP_LEVEL_WORKFLOW" '    uses: ./.github/workflows/mob-dense-topology.yml'
assert_file_contains "$TOP_LEVEL_WORKFLOW" '      - github-hosted-dense-topology'
assert_file_contains "$TOP_LEVEL_WORKFLOW" '${{ needs.github-hosted-dense-topology.result }}'
assert_file_contains "$TOP_LEVEL_WORKFLOW" 'schema_version: 3'
assert_file_contains "$RELEASE_WORKFLOW" '.schema_version == 3'
assert_file_contains "$RELEASE_WORKFLOW" '.validation_backend == "gcp-buildbuddy+github-hosted-dense-mob"'
assert_file_contains "$RELEASE_WORKFLOW" '.component_results.gcp_buildbuddy == "success"'
assert_file_contains "$RELEASE_WORKFLOW" '.component_results.github_hosted_dense_mob == "success"'

unit_job="$(sed -n '/^  unit:$/,/^  int-archives:$/p' "$WORKFLOW")"
[[ "$unit_job" == *'      - unit-archive'* ]] || {
  echo "unit execution does not depend on unit-archive" >&2
  exit 1
}
archive_job="$(sed -n '/^  int-archives:$/,/^  int-mob:$/p' "$WORKFLOW")"
for family in int-heavy int-mob int-everything-else; do
  [[ "$archive_job" == *"          family: ${family}"* ]] || {
    echo "integration archive builder omits ${family}" >&2
    exit 1
  }
done
[[ "$archive_job" == *'          publish: "false"'* ]] || {
  echo "int-heavy archive must not be published to redundant execution runners" >&2
  exit 1
}
[[ "$archive_job" == *'scripts/ci-nextest-archive.sh run'* ]] || {
  echo "int-heavy archive is not executed on its build runner" >&2
  exit 1
}
[[ "$archive_job" == *'          hash:1/1'* ]] || {
  echo "int-heavy local archive execution is not fail-closed over the whole family" >&2
  exit 1
}
mob_job="$(sed -n '/^  int-mob:$/,/^  int-else:$/p' "$WORKFLOW")"
[[ "$mob_job" == *'      - int-archives'* ]] || {
  echo "int-mob execution does not depend on int-archives" >&2
  exit 1
}
else_job="$(sed -n '/^  int-else:$/,/^  int-rest:$/p' "$WORKFLOW")"
[[ "$else_job" == *'      - int-archives'* ]] || {
  echo "int-else execution does not depend on int-archives" >&2
  exit 1
}

if rg -n 'env!\("CARGO_BIN_EXE_' "$ROOT" --glob '*.rs'; then
  echo "archived tests must resolve Cargo binary paths from the runtime environment" >&2
  exit 1
fi

manifest_dir_env_matches="$(
  rg -n 'env!\("CARGO_MANIFEST_DIR"\)' "$ROOT" --glob '*.rs' || true
)"
manifest_dir_env_count="$(
  printf '%s\n' "$manifest_dir_env_matches" | sed '/^$/d' | wc -l | tr -d '[:space:]'
)"
if ((manifest_dir_env_count > 71)); then
  printf '%s\n' "$manifest_dir_env_matches" >&2
  echo "compile-time CARGO_MANIFEST_DIR archive paths grew from the pinned budget of 71" >&2
  exit 1
fi

echo "CI nextest archive family and fail-closed contracts hold"
