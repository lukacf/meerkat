#!/usr/bin/env bash
# One row of the Nightly's integration-fast tests (nightly.yml, workspace-int;
# the rows come from scripts/ci-nightly-int-split.mjs). The same steps as a
# closure-tests row of ci.yml: build and list the row's selection, check that
# nextest lists exactly the row's binaries, run it; then write the row's
# report for the aggregate, whatever happened.
#
# Inputs (environment, from the job matrix): PACKAGES, TESTS, PROFILE, FILTER,
# BINARIES, NO_TESTS, MIN_STACK, PARTITION, ROW_ID, ROW_NAME, ROW_KIND,
# ROW_CHECK; and REPORT_DIR (where the report goes).
set -euo pipefail

for name in PACKAGES TESTS PROFILE BINARIES NO_TESTS ROW_ID ROW_NAME ROW_KIND ROW_CHECK REPORT_DIR; do
  if [[ -z "${!name:-}" ]]; then
    echo "::error::incomplete nightly integration-fast row: ${name} is empty"
    exit 1
  fi
done

mkdir -p "${REPORT_DIR}"
list="${REPORT_DIR}/nextest-list.json"
log="${REPORT_DIR}/nextest-run.log"

# The report is written on every way out of this script, with the status it
# leaves with, so a row that failed to build still says so to the aggregate.
# A row whose runner is lost writes nothing, and the aggregate names it as
# missing.
write_report() {
  local code=$?
  node scripts/ci-nightly-int-split.mjs report --row "${ROW_ID}" --name "${ROW_NAME}" --kind "${ROW_KIND}" \
    --binaries "${BINARIES}" --list "${list}" --log "${log}" --exit "${code}" --out "${REPORT_DIR}/report.json" ||
    echo "::error::the row's report could not be written"
  exit "${code}"
}
trap write_report EXIT

read -r -a test_flags <<<"${TESTS}"
filter_args=()
if [[ -n "${FILTER:-}" ]]; then filter_args=(-E "${FILTER}"); fi
partition_args=()
if [[ -n "${PARTITION:-}" ]]; then partition_args=(--partition "${PARTITION}"); fi
# One selection for the list and the run, as in ci.yml's closure-tests step.
# shellcheck disable=SC2206
selection=(${PACKAGES} "${test_flags[@]}" --profile "${PROFILE}" "${filter_args[@]}" "${partition_args[@]}")
# `make test-int` ran on the 8 MiB test-thread stack; the row carries it.
if [[ -n "${MIN_STACK:-}" ]]; then export RUST_MIN_STACK="${MIN_STACK}"; fi
# Tests of crates that dev-depend on mcp-test-server spawn this exact fixture
# and fail without it (mcp_test_server::fixture_binary).
# shellcheck disable=SC2086
fixture="$(scripts/mcp-test-server-fixture --if-needed ${PACKAGES})"
if [[ -n "${fixture}" ]]; then
  export MEERKAT_MCP_TEST_SERVER="${fixture}"
  echo "mcp-test-server fixture: ${MEERKAT_MCP_TEST_SERVER}"
fi

./scripts/repo-cargo nextest list "${selection[@]}" --message-format json >"${list}"
if [[ "${ROW_CHECK}" == "binaries" ]]; then
  # Exactly the row's binaries, none of their tests dropped by the profile.
  # shellcheck disable=SC2086
  node scripts/ci-verify-row-binaries.mjs "${list}" "${partition_args[@]}" ${BINARIES}
fi
# A row that selects tests of a binary by name (HeadCanonical's kill-window filter)
# has no such check: `--no-tests=fail` below fails it when nothing is selected.

set +e
./scripts/repo-cargo nextest run "${selection[@]}" --no-tests="${NO_TESTS}" --no-fail-fast \
  --status-level slow --final-status-level slow 2>&1 | tee "${log}"
status="${PIPESTATUS[0]}"
set -e
exit "${status}"
