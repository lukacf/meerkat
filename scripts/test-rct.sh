#!/bin/bash
# Run the fast test suite for Meerkat (unit + integration-fast).
# Slow suites (integration-real, e2e) are ignored by default.

set -e

echo "=== Running Meerkat FAST Tests ==="
echo ""

echo "Running fast suite (unit + integration-fast)..."
cargo test --workspace --lib --bins --tests

echo ""
echo "=== Fast Tests Complete ==="

echo ""
echo "Optional slow suites:"
echo "  cargo test --workspace integration_real -- --ignored --test-threads=1"
echo "  MEERKAT_LIVE_API_TESTS=1 cargo test --workspace e2e_ -- --ignored --test-threads=1"
