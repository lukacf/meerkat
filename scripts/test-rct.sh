#!/bin/bash
# Run the fast test suite for Meerkat (unit + integration-fast).
# Slow suites (integration-real, e2e) are ignored by default.

set -e

FAST_TARGET_DIR="${FAST_TARGET_DIR:-target/fast}"

echo "=== Running Meerkat FAST Tests ==="
echo ""

echo "Running fast suite (unit + integration-fast)..."
CARGO_TARGET_DIR="$FAST_TARGET_DIR" cargo test --workspace --lib --bins --tests

echo ""
echo "=== Fast Tests Complete ==="

echo ""
echo "Optional slow suites:"
echo "  cargo test --workspace integration_real -- --ignored --test-threads=1"
echo "  cargo test --workspace e2e_ -- --ignored --test-threads=1"
