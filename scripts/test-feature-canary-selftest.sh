#!/usr/bin/env bash
# The test-feature canary (each Core crate's lib.rs, armed by the
# repository's .cargo/config.toml [env]) must stop a test binary built
# without the crate's test features, and nothing else. This builds a fixture
# crate with the same canary (scripts/tests/fixtures/test-feature-canary;
# scripts/test_published_crates_standalone.py holds it to the Core crates'
# shape) on the pinned toolchain, in a scratch directory outside the
# repository so its Cargo configuration does not arm it unasked:
#   armed, without the feature: check and clippy of every target pass, and a
#     test build (debug or release) fails with E0080 and the canary's message;
#   armed, with the feature: the tests build and pass;
#   unarmed: the tests build and pass.
# A toolchain that evaluated the canary in check builds, or stopped
# evaluating it in code generation, fails here. A few seconds.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE="${ROOT}/scripts/tests/fixtures/test-feature-canary"
TOOLCHAIN="$(sed -n 's/^channel = "\(.*\)"$/\1/p' "${ROOT}/rust-toolchain.toml")"
if [[ -z "${TOOLCHAIN}" ]]; then
  echo "test-feature-canary-selftest: no channel in rust-toolchain.toml" >&2
  exit 1
fi
if ! grep -qx 'MEERKAT_WORKSPACE_TEST_FEATURES = "required"' "${ROOT}/.cargo/config.toml"; then
  echo "test-feature-canary-selftest: .cargo/config.toml [env] does not arm the canary" >&2
  exit 1
fi

TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-test-feature-canary.XXXXXX")"
trap 'rm -rf -- "$TEST_ROOT"' EXIT
mkdir -p "${TEST_ROOT}/crate/src"
cp "${FIXTURE}/Cargo.toml.txt" "${TEST_ROOT}/crate/Cargo.toml"
cp "${FIXTURE}/lib.rs.txt" "${TEST_ROOT}/crate/src/lib.rs"
cd "${TEST_ROOT}/crate"
export RUSTUP_TOOLCHAIN="${TOOLCHAIN}" CARGO_TARGET_DIR="${TEST_ROOT}/target" CARGO_TERM_COLOR=never
unset MEERKAT_WORKSPACE_TEST_FEATURES
LOG="${TEST_ROOT}/cargo.log"
MESSAGE="this repository's test builds need the crate's test features"

armed() {
  MEERKAT_WORKSPACE_TEST_FEATURES=required "$@"
}

expect_pass() {
  local label="$1"
  shift
  if ! "$@" >"${LOG}" 2>&1; then
    echo "FAIL ${label}: the build failed" >&2
    cat "${LOG}" >&2
    exit 1
  fi
  if grep -q "${MESSAGE}" "${LOG}"; then
    echo "FAIL ${label}: the canary fired" >&2
    cat "${LOG}" >&2
    exit 1
  fi
  echo "ok   ${label}"
}

expect_canary() {
  local label="$1"
  shift
  if "$@" >"${LOG}" 2>&1; then
    echo "FAIL ${label}: the build passed; the canary did not fire" >&2
    exit 1
  fi
  if ! grep -q '^error\[E0080\]' "${LOG}" || ! grep -q "${MESSAGE}" "${LOG}"; then
    echo "FAIL ${label}: the build failed, but not on the canary" >&2
    cat "${LOG}" >&2
    exit 1
  fi
  echo "ok   ${label}"
}

expect_pass "armed: cargo check --all-targets" armed cargo check --all-targets
expect_pass "armed: cargo clippy --all-targets -D warnings" armed cargo clippy --all-targets -- -D warnings
expect_canary "armed: cargo test --no-run" armed cargo test --no-run
expect_canary "armed: cargo test --release --no-run" armed cargo test --release --no-run
expect_pass "armed: cargo test --features canary-test-feature" armed cargo test --features canary-test-feature
expect_pass "unarmed: cargo test" cargo test
echo "test-feature canary: only test code generation trips it (toolchain ${TOOLCHAIN})"
