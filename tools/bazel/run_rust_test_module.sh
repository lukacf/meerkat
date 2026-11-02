#!/usr/bin/env bash
# Runs a Rust unit test binary under the libtest arguments it is given: the
# test target of one module of that binary, which its unit target skips
# (UNIT_TEST_MODULE_TARGETS in scripts/generate-bazel-rust-builds.mjs).
#
# usage: run_rust_test_module.sh <test binary> <libtest argument>...
set -euo pipefail

binary="${1:?usage: run_rust_test_module.sh <test binary> <libtest argument>...}"
shift
exec "./${binary}" "$@"
