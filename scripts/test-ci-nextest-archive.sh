#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# The source scans below read this checkout through Git. A hook inherits
# repository-local Git variables (GIT_DIR, GIT_INDEX_FILE, ...) that would
# point them elsewhere, so drop them, as the pre-push dispatcher does.
while IFS= read -r git_local_env; do
  unset "$git_local_env"
done < <(git rev-parse --local-env-vars)

TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-ci-nextest-archive.XXXXXX")"
trap 'rm -rf -- "$TEST_ROOT"' EXIT

LOG="${TEST_ROOT}/commands.log"
FAKE_CARGO="${TEST_ROOT}/repo-cargo"
FAKE_NEXTEST="${TEST_ROOT}/cargo-nextest"
FAKE_METADATA="${TEST_ROOT}/metadata.json"
FAKE_FIXTURE_BINARY="${TEST_ROOT}/mcp-test-server"

# The archive build asks scripts/mcp-test-server-fixture whether a selected
# package dev-depends on mcp-test-server, and builds the fixture first if so.
# The fake answers that `cargo metadata` call and the fixture build, so the
# fixture's real selection runs and any fixture error fails this test.
cat >"$FAKE_CARGO" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'cargo' >>"$COMMAND_LOG"
printf ' <%s>' "$@" >>"$COMMAND_LOG"
printf '\n' >>"$COMMAND_LOG"
case "${1:-}" in
  metadata)
    [[ "$*" == 'metadata --format-version=1 --no-deps' ]] || {
      echo "fake cargo: unexpected metadata call: $*" >&2
      exit 93
    }
    case "${FAKE_CARGO_METADATA:-ok}" in
      ok) cat "$FAKE_METADATA" ;;
      invalid) echo 'not cargo metadata' ;;
      *)
        echo "fake cargo: metadata failed" >&2
        exit 94
        ;;
    esac
    exit 0
    ;;
  build)
    [[ "$*" == 'build -p mcp-test-server --bin mcp-test-server --locked --message-format=json-render-diagnostics' ]] || {
      echo "fake cargo: unexpected build call: $*" >&2
      exit 95
    }
    printf '{"reason":"compiler-artifact","target":{"name":"mcp-test-server","kind":["bin"]},"executable":"%s"}\n' \
      "$FAKE_FIXTURE_BINARY"
    printf '{"reason":"build-finished","success":true}\n'
    exit 0
    ;;
esac
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
# The workspace's mcp-test-server dev-dependents (meerkat, meerkat-mcp,
# meerkat-rpc, meerkat-integration-tests), two Core packages without one, and
# one package of another product (under ci/products.json's meta root), which
# the families over Core's whole workspace exclude
# (scripts/core-workspace-args.mjs).
cat >"$FAKE_METADATA" <<'EOF'
{"workspace_root": "/ws",
 "workspace_members": ["meerkat", "meerkat-mcp", "meerkat-rpc", "meerkat-integration-tests", "meerkat-mob", "mcp-test-server", "meerkat-meta"],
 "workspace_default_members": ["meerkat", "meerkat-mcp", "meerkat-rpc", "meerkat-integration-tests", "meerkat-mob", "mcp-test-server"],
 "packages": [
  {"id": "meerkat", "name": "meerkat", "manifest_path": "/ws/crates/meerkat/Cargo.toml", "dependencies": [{"name": "mcp-test-server", "kind": "dev"}]},
  {"id": "meerkat-mcp", "name": "meerkat-mcp", "manifest_path": "/ws/crates/meerkat-mcp/Cargo.toml", "dependencies": [{"name": "mcp-test-server", "kind": "dev"}]},
  {"id": "meerkat-rpc", "name": "meerkat-rpc", "manifest_path": "/ws/crates/meerkat-rpc/Cargo.toml", "dependencies": [{"name": "mcp-test-server", "kind": "dev"}]},
  {"id": "meerkat-integration-tests", "name": "meerkat-integration-tests", "manifest_path": "/ws/tests/integration/Cargo.toml", "dependencies": [{"name": "mcp-test-server", "kind": "dev"}]},
  {"id": "meerkat-mob", "name": "meerkat-mob", "manifest_path": "/ws/crates/meerkat-mob/Cargo.toml", "dependencies": []},
  {"id": "mcp-test-server", "name": "mcp-test-server", "manifest_path": "/ws/tests/fixtures/mcp-test-server/Cargo.toml", "dependencies": []},
  {"id": "meerkat-meta", "name": "meerkat-meta", "manifest_path": "/ws/meta/crates/meerkat-meta/Cargo.toml", "dependencies": []}
]}
EOF
printf '#!/bin/sh\nexit 0\n' >"$FAKE_FIXTURE_BINARY"
chmod +x "$FAKE_FIXTURE_BINARY"
export COMMAND_LOG="$LOG"
export EXPECTED_WORKSPACE_ROOT="$ROOT"
export ROOT
export CARGO="$FAKE_CARGO"
export NEXTEST_BIN="$FAKE_NEXTEST"
export FAKE_METADATA FAKE_FIXTURE_BINARY
# An exported fixture path would skip the fixture build this test observes.
unset MEERKAT_MCP_TEST_SERVER FAKE_CARGO_METADATA

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

assert_line_contains 'cargo <nextest> <archive> <--workspace> <--exclude> <meerkat-meta> <--lib>'
assert_line_contains 'cargo <nextest> <archive> <-p> <meerkat-integration-tests> <--tests> <--profile> <fast>'
assert_line_contains '<-p> <meerkat-mob> <-p> <meerkat-mob-adaptive>'
assert_line_contains '<--workspace> <--exclude> <meerkat-meta> <--exclude> <meerkat-integration-tests> <--exclude> <meerkat-mob>'

# Per family, in order: Core's package selection for the families over
# Core's whole workspace (unit, int-everything-else), the fixture check, then
# the fixture build when a selected package dev-depends on mcp-test-server (so
# the binary rides in the archive), then the archive. unit-mob and int-mob
# select no such package.
cargo_sequence="$(sed -n 's/^cargo <\([^>]*\)>.*/\1/p' "$LOG" | tr '\n' ' ')"
expected_cargo_sequence='metadata metadata build nextest metadata nextest metadata build nextest metadata nextest metadata metadata build nextest '
[[ "$cargo_sequence" == "$expected_cargo_sequence" ]] || {
  echo "archive builds did not check and build the mcp-test-server fixture before archiving" >&2
  echo "  expected cargo calls: ${expected_cargo_sequence}" >&2
  echo "  actual cargo calls:   ${cargo_sequence}" >&2
  exit 1
}
# A fixture check that cannot read the package graph stops the archive build;
# it must never be taken for "no package needs the fixture". (int-heavy: its
# fixture check is the build's first package-graph read.)
for metadata_mode in fail invalid; do
  metadata_archive="${TEST_ROOT}/metadata-${metadata_mode}.tar.zst"
  if COMMAND_LOG="${TEST_ROOT}/metadata-${metadata_mode}.log" \
    FAKE_CARGO_METADATA="$metadata_mode" \
    "$ROOT/scripts/ci-nextest-archive.sh" build int-heavy "$metadata_archive" \
    >"${TEST_ROOT}/metadata-${metadata_mode}.out" 2>&1; then
    echo "archive build ignored a failed mcp-test-server fixture check (cargo metadata: ${metadata_mode})" >&2
    cat "${TEST_ROOT}/metadata-${metadata_mode}.out" >&2
    exit 1
  fi
  if ! grep -Fq 'mcp-test-server-fixture: ' "${TEST_ROOT}/metadata-${metadata_mode}.out" ||
    [[ -e "$metadata_archive" ]]; then
    echo "archive build did not stop at the failed fixture check (cargo metadata: ${metadata_mode})" >&2
    cat "${TEST_ROOT}/metadata-${metadata_mode}.out" >&2
    exit 1
  fi
done

# Neither may Core's package selection: an unreadable package graph stops a
# build over Core's whole workspace instead of selecting every product.
for metadata_mode in fail invalid; do
  selection_archive="${TEST_ROOT}/selection-${metadata_mode}.tar.zst"
  if COMMAND_LOG="${TEST_ROOT}/selection-${metadata_mode}.log" \
    FAKE_CARGO_METADATA="$metadata_mode" \
    "$ROOT/scripts/ci-nextest-archive.sh" build unit "$selection_archive" \
    >"${TEST_ROOT}/selection-${metadata_mode}.out" 2>&1; then
    echo "archive build ignored an unreadable package graph for Core's selection (cargo metadata: ${metadata_mode})" >&2
    cat "${TEST_ROOT}/selection-${metadata_mode}.out" >&2
    exit 1
  fi
  if ! grep -Fq "cannot derive Core's package selection" "${TEST_ROOT}/selection-${metadata_mode}.out" ||
    [[ -e "$selection_archive" ]]; then
    echo "archive build did not stop at Core's package selection (cargo metadata: ${metadata_mode})" >&2
    cat "${TEST_ROOT}/selection-${metadata_mode}.out" >&2
    exit 1
  fi
done

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

unit_run="$(grep -m 1 '^nextest ' "$LOG")"
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
NIGHTLY_WORKFLOW="$ROOT/.github/workflows/nightly.yml"
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
assert_file_contains "$BUILD_ACTION" 'python3 scripts/restore-ci-unit-mob-archive.py'
assert_file_contains "$BUILD_ACTION" 'if [[ "${ARCHIVE_REUSED}" != "true" ]]; then'
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
# The reservation must sit under a default-profile override: nextest's
# `inherits` does not carry another profile's overrides, so the same stanza
# under [[profile.fast.overrides]] would silently not apply to ci-unit/ci-pr.
workflow_isolation_header="$(awk '
  /^\[/ { header = $0 }
  pending && $0 == "threads-required = \"num-cpus\"" { print header }
  { pending = ($0 == "filter = '\''test(=machines::tests::machine_workflow_red_ok_detects_missing_and_stale_generated_artifacts)'\''") }
' "$NEXTEST_CONFIG")"
[[ "$workflow_isolation_header" == '[[profile.default.overrides]]' ]] || {
  echo "the machine workflow test must reserve the whole lane (threads-required = \"num-cpus\") under [[profile.default.overrides]], found: ${workflow_isolation_header:-none}" >&2
  exit 1
}
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
assert_file_contains "$DENSE_WORKFLOW" '          reuse_exact_ci: "true"'
assert_file_contains "$DENSE_WORKFLOW" '    timeout-minutes: 30'
assert_file_contains "$DENSE_WORKFLOW" '    timeout-minutes: 10'
assert_file_contains "$DENSE_WORKFLOW" '          cache-on-failure: true'
assert_file_contains "$DENSE_WORKFLOW" '          cache-workspace-crates: true'
assert_file_contains "$DENSE_WORKFLOW" '          profile: mob-dense-topology'
assert_file_contains "$DENSE_WORKFLOW" '          run_ignored: all'
# The dense topology stress moved off the PR hot path: nightly calls the
# reusable workflow, PR CI does not.
assert_file_contains "$NIGHTLY_WORKFLOW" '  dense-topology:'
assert_file_contains "$NIGHTLY_WORKFLOW" '    uses: ./.github/workflows/mob-dense-topology.yml'
if grep -F 'mob-dense-topology.yml' "$TOP_LEVEL_WORKFLOW" | grep -Fq 'uses:'; then
  echo "PR CI must not run the dense Mob topology stress" >&2
  exit 1
fi
assert_file_contains "$TOP_LEVEL_WORKFLOW" 'schema_version: 4'
assert_file_contains "$TOP_LEVEL_WORKFLOW" 'validation_backend: "github-hosted-cargo"'
# A partition-only re-run (attempt 2+) does not re-run the archive build, so
# the archive artifact must be keyed by run, not attempt, and a full re-run
# must be able to replace it.
archive_name='          name: nextest-${{ matrix.archive }}-${{ github.run_id }}'
for step in 'Upload nextest archive' 'Download nextest archive'; do
  if ! grep -F -A8 -- "- name: ${step}" "$TOP_LEVEL_WORKFLOW" | grep -qxF -- "${archive_name}"; then
    echo "${step} does not use the run-keyed archive name" >&2
    exit 1
  fi
done
if ! grep -F -A10 -- '- name: Upload nextest archive' "$TOP_LEVEL_WORKFLOW" | grep -qxF -- '          overwrite: true'; then
  echo "Upload nextest archive must overwrite so a full re-run can replace the archive" >&2
  exit 1
fi
if grep -Fq 'name: nextest-${{ matrix.archive }}-${{ github.run_id }}-${{ github.run_attempt }}' "$TOP_LEVEL_WORKFLOW"; then
  echo "nextest archive artifact is keyed by run attempt; partition re-runs cannot find it" >&2
  exit 1
fi
# Archive consumers must use Nextest's direct runner; these paths execute the
# supplied binaries without resolving or rebuilding the source Cargo graph.
assert_file_contains "$TOP_LEVEL_WORKFLOW" 'cargo-nextest nextest list --archive-file "${archive}"'
assert_file_contains "$TOP_LEVEL_WORKFLOW" 'cargo-nextest nextest run --archive-file "${RUNNER_TEMP}/nextest-archive/nextest-${ARCHIVE}.tar.zst"'

# Execute the doctor's actual workflow check against temporary source fixtures.
# This keeps its archive-only exception covered without copying the detector.
WORKFLOW_PROBE_ROOT="${TEST_ROOT}/workflow-entrypoints"
mkdir -p "${WORKFLOW_PROBE_ROOT}/.github/workflows"
WORKFLOW_CHECK="${TEST_ROOT}/doctor-workflow-check.sh"
WORKFLOW_BAD='GitHub workflow Rust entrypoints bypass scripts/repo-cargo'
cat >"${WORKFLOW_CHECK}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
pass() { :; }
bad() { printf '%s\n' "$1" >&2; return 1; }
EOF
# Exactly the workflow check: from its first scan through the `fi` that ends
# its verdict. Later doctor checks (such as the env bash version) are not
# this contract and must not decide the probes below.
awk '
  /^raw_cargo_workflow_hits=/ { selected = 1 }
  selected { print }
  selected && /^fi$/ { exit }
' "$ROOT/scripts/rust-lane-doctor" >>"${WORKFLOW_CHECK}"
if ! grep -Fq "bad \"${WORKFLOW_BAD}\"" "${WORKFLOW_CHECK}" ||
  ! grep -Fq 'pass "GitHub workflow Rust entrypoints use the repo Cargo wrapper"' "${WORKFLOW_CHECK}" ||
  [[ "$(tail -n 1 "${WORKFLOW_CHECK}")" != 'fi' ]]; then
  echo "doctor workflow check fixture no longer selects its actual owner" >&2
  exit 1
fi
check_workflow_entrypoints() {
  local description="$1" expected="$2" source="$3" actual
  printf '%s\n' "${source}" >"${WORKFLOW_PROBE_ROOT}/.github/workflows/probe.yml"
  if (cd "${WORKFLOW_PROBE_ROOT}" && bash "${WORKFLOW_CHECK}") \
    >"${TEST_ROOT}/workflow-check.out" 2>&1; then
    actual=pass
  elif grep -Fxq -- "${WORKFLOW_BAD}" "${TEST_ROOT}/workflow-check.out"; then
    actual=fail
  else
    # Exiting without the check's own verdict is a crash, not a detection.
    actual=error
  fi
  if [[ "${actual}" != "${expected}" ]]; then
    echo "doctor workflow check ${description}: expected ${expected}, got ${actual}" >&2
    cat "${TEST_ROOT}/workflow-check.out" >&2
    exit 1
  fi
}
check_workflow_entrypoints "actual archive consumers" pass "$(cat "$TOP_LEVEL_WORKFLOW")"
check_workflow_entrypoints "alias run" fail $'run: |\n  cargo-nextest nextest r --archive-file fixture.tar.zst'
check_workflow_entrypoints "archive subcommand" fail $'run: |\n  cargo-nextest nextest archive --archive-file fixture.tar.zst'
check_workflow_entrypoints "path-qualified binary" fail $'run: |\n  /usr/local/bin/cargo-nextest nextest run --archive-file fixture.tar.zst'
check_workflow_entrypoints "global option before run" fail $'run: |\n  cargo-nextest nextest --color always run --archive-file fixture.tar.zst'
check_workflow_entrypoints "global option before nextest" fail $'run: |\n  cargo-nextest --color always nextest list --archive-file fixture.tar.zst'
check_workflow_entrypoints "continued archive flag" pass $'run: |\n  cargo-nextest nextest list \\\n    --archive-file "fixture.tar.zst" --profile ci-pr'
check_workflow_entrypoints "equals archive flag" pass $'run: |\n  cargo-nextest nextest run --archive-file=fixture.tar.zst'
check_workflow_entrypoints "nonarchive run" fail $'run: |\n  cargo-nextest nextest run -p meerkat-core'
check_workflow_entrypoints "nonarchive list" fail $'run: |\n  cargo-nextest nextest list \\\n    --profile ci-pr'
check_workflow_entrypoints "following command flag" fail $'run: |\n  cargo-nextest nextest run; echo --archive-file fixture.tar.zst'
check_workflow_entrypoints "commented flag" fail $'run: |\n  cargo-nextest nextest list # --archive-file fixture.tar.zst'
check_workflow_entrypoints "lookalike flag" fail $'run: |\n  cargo-nextest nextest run --archive-file-other fixture.tar.zst'
check_workflow_entrypoints "test-binary argument flag" fail $'run: |\n  cargo-nextest nextest run -p meerkat-core -- --archive-file fixture.tar.zst'
check_workflow_entrypoints "comment cannot continue over next command" fail $'run: |\n  cargo-nextest nextest list --archive-file fixture.tar.zst # \\\n  cargo-nextest nextest run -p meerkat-core'
check_workflow_entrypoints "raw Cargo archive consumer" fail 'run: cargo nextest list --archive-file fixture.tar.zst'
# The raw Cargo scan: `run: cargo` anywhere, or a line of whitespace and then
# `cargo <build or test subcommand>` as a whole word.
check_workflow_entrypoints "inline raw Cargo command" fail '    run: cargo fmt --all -- --check'
check_workflow_entrypoints "indented raw Cargo command" fail $'run: |\n  cargo build -p meerkat-core'
check_workflow_entrypoints "tab-indented raw Cargo command" fail $'run: |\n\tcargo clippy --workspace'
check_workflow_entrypoints "CRLF raw Cargo command" fail $'run: |\r\n  cargo test -p meerkat-core\r'
check_workflow_entrypoints "raw Cargo subcommand at line end" fail $'run: |\n  cargo machete'
check_workflow_entrypoints "Cargo subcommand prefix of a word" pass $'run: |\n  cargo testing --help'
check_workflow_entrypoints "Cargo after another command" pass $'run: |\n  echo cargo build'
check_workflow_entrypoints "unlisted Cargo subcommand" pass $'run: |\n  cargo metadata --format-version=1'
check_workflow_entrypoints "quoted diagnostic" pass 'run: echo "cargo-nextest nextest run"'

# The doctor's shell entrypoint scan. ripgrep exits 0 when it finds a raw
# Cargo entrypoint and 1 when it finds none; any other status (127 for a
# missing rg, 2 for an unreadable scan root) means the scan did not run, which
# must fail the doctor instead of reading as "no bypass".
SHELL_SCAN_ROOT="${TEST_ROOT}/shell-entrypoints"
SHELL_SCAN_CHECK="${TEST_ROOT}/doctor-shell-scan-check.sh"
SHELL_SCAN_PASS='PASS shell build/test entrypoints use the repo Cargo wrapper'
SHELL_SCAN_BYPASS='FAIL shell build/test entrypoints bypass scripts/repo-cargo'
SHELL_SCAN_FAILED='FAIL shell build/test entrypoint scan failed'
cat >"${SHELL_SCAN_CHECK}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
pass() { printf 'PASS %s\n' "$1"; }
bad() { printf 'FAIL %s\n' "$1"; }
EOF
# Exactly the shell scan: from its first assignment through the `fi` that ends
# its verdict.
awk '
  /^raw_cargo_shell_(scan_status|hits)=/ { selected = 1 }
  selected { print }
  selected && /^fi$/ { exit }
' "$ROOT/scripts/rust-lane-doctor" >>"${SHELL_SCAN_CHECK}"
if ! grep -Fq "pass \"${SHELL_SCAN_PASS#PASS }\"" "${SHELL_SCAN_CHECK}" ||
  ! grep -Fq "bad \"${SHELL_SCAN_BYPASS#FAIL }\"" "${SHELL_SCAN_CHECK}" ||
  [[ "$(tail -n 1 "${SHELL_SCAN_CHECK}")" != 'fi' ]]; then
  echo "doctor shell scan fixture no longer selects its actual owner" >&2
  exit 1
fi
SHELL_SCAN_NO_RG_BIN="${TEST_ROOT}/shell-scan-no-rg-bin"
mkdir -p "${SHELL_SCAN_NO_RG_BIN}"
check_shell_entrypoints() {
  local description="$1" expected="$2" layout="$3" path_mode="$4" actual output
  rm -rf -- "${SHELL_SCAN_ROOT}"
  mkdir -p "${SHELL_SCAN_ROOT}/scripts" "${SHELL_SCAN_ROOT}/specs"
  printf '#!/usr/bin/env bash\n./scripts/repo-cargo test -p meerkat-core\n' \
    >"${SHELL_SCAN_ROOT}/scripts/wrapped.sh"
  case "${layout}" in
    clean) ;;
    bypass) printf '#!/usr/bin/env bash\ncargo test -p meerkat-core\n' >"${SHELL_SCAN_ROOT}/specs/raw.sh" ;;
    missing-root) rm -rf -- "${SHELL_SCAN_ROOT}/specs" ;;
  esac
  output="${TEST_ROOT}/shell-scan-check.out"
  if [[ "${path_mode}" == no-rg ]]; then
    (cd "${SHELL_SCAN_ROOT}" && PATH="${SHELL_SCAN_NO_RG_BIN}" "${BASH}" "${SHELL_SCAN_CHECK}") \
      >"${output}" 2>&1 || true
  else
    (cd "${SHELL_SCAN_ROOT}" && "${BASH}" "${SHELL_SCAN_CHECK}") >"${output}" 2>&1 || true
  fi
  if grep -Fxq -- "${SHELL_SCAN_PASS}" "${output}"; then
    actual=pass
  elif grep -Fxq -- "${SHELL_SCAN_BYPASS}" "${output}"; then
    actual=bypass
  elif grep -Fq -- "${SHELL_SCAN_FAILED}" "${output}"; then
    actual=scan-failed
  else
    # Exiting without the scan's own verdict is a crash, not a detection.
    actual=error
  fi
  if [[ "${actual}" != "${expected}" ]]; then
    echo "doctor shell scan ${description}: expected ${expected}, got ${actual}" >&2
    cat "${output}" >&2
    exit 1
  fi
}
if command -v rg >/dev/null 2>&1; then
  check_shell_entrypoints "wrapped entrypoints" pass clean rg
  check_shell_entrypoints "raw Cargo entrypoint" bypass bypass rg
  check_shell_entrypoints "unreadable scan root" scan-failed missing-root rg
else
  echo "note: ripgrep is not installed; skipped the doctor shell scan probes that need it" >&2
fi
check_shell_entrypoints "missing ripgrep" scan-failed clean no-rg
assert_file_contains "$RELEASE_WORKFLOW" '.schema_version == 4'
assert_file_contains "$RELEASE_WORKFLOW" '.validation_backend == "github-hosted-cargo"'
assert_file_contains "$RELEASE_WORKFLOW" '.schema_version == 3'
assert_file_contains "$RELEASE_WORKFLOW" '.validation_backend == "gcp-buildbuddy+github-hosted-dense-mob"'
assert_file_contains "$RELEASE_WORKFLOW" '.component_results.gcp_buildbuddy == "success"'
assert_file_contains "$RELEASE_WORKFLOW" '.component_results.github_hosted_dense_mob == "success"'

# The restore helper runs on the release scripts' Python >= 3.10 floor.
restore_python="$("$ROOT/scripts/require-python" 3.10 "the nextest archive restore test")" || exit 1
"$restore_python" "$ROOT/scripts/test_restore_ci_unit_mob_archive.py"
# Also on the oldest supported interpreter, when one is installed and runs: a
# version manager's python3.10 shim (pyenv, asdf) without a selected 3.10
# exits 127 instead of running Python.
if oldest_python="$(command -v python3.10 2>/dev/null)"; then
  if "$oldest_python" -c 'import sys; raise SystemExit(sys.version_info[:2] != (3, 10))' >/dev/null 2>&1; then
    "$oldest_python" "$ROOT/scripts/test_restore_ci_unit_mob_archive.py"
  else
    echo "note: ${oldest_python} does not run Python 3.10; skipped the restore test on the 3.10 floor" >&2
  fi
fi

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

# The archived lanes run Core's tests only (every family over Core's whole
# workspace excludes the other products' packages), so the scans below read
# Core's tree: each product root of ci/products.json is left out.
product_roots="$(cd "$ROOT" && node --input-type=module -e '
import { loadProducts, productRootPrefixes } from "./scripts/ci-products.mjs";
for (const prefix of productRootPrefixes(loadProducts())) console.log(prefix.replace(/\/$/, ""));
')" || {
  echo "error: cannot read the product roots of ci/products.json" >&2
  exit 1
}
product_root_pathspecs=()
while IFS= read -r product_root; do
  [[ -n "$product_root" ]] && product_root_pathspecs+=(":(exclude)${product_root}")
done <<<"$product_roots"

# Prints every line of a tracked or untracked (not ignored) *.rs file of
# Core's tree that contains the literal $1. Git, not ripgrep: Git is on every
# machine that runs this hook, and a user ripgrep config cannot change the
# result.
rust_source_lines() {
  local matches status=0
  matches="$(git -C "$ROOT" grep --untracked -n -F -e "$1" -- '*.rs' "${product_root_pathspecs[@]}")" || status=$?
  case "$status" in
    0) printf '%s\n' "$matches" ;;
    1) ;;
    *)
      echo "error: git grep failed (exit ${status}) scanning Rust sources for $1" >&2
      return 1
      ;;
  esac
}

bin_exe_matches="$(rust_source_lines 'env!("CARGO_BIN_EXE_')" || exit 1
if [[ -n "$bin_exe_matches" ]]; then
  printf '%s\n' "$bin_exe_matches"
  echo "archived tests must resolve Cargo binary paths from the runtime environment" >&2
  exit 1
fi

manifest_dir_env_matches="$(rust_source_lines 'env!("CARGO_MANIFEST_DIR")')" || exit 1
manifest_dir_env_count="$(
  printf '%s\n' "$manifest_dir_env_matches" | sed '/^$/d' | wc -l | tr -d '[:space:]'
)"
if ((manifest_dir_env_count > 71)); then
  printf '%s\n' "$manifest_dir_env_matches" >&2
  echo "compile-time CARGO_MANIFEST_DIR archive paths grew from the pinned budget of 71" >&2
  exit 1
fi

echo "CI nextest archive family and fail-closed contracts hold"
