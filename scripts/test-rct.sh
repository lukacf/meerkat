#!/bin/bash
# Run the fast test suite for Meerkat (unit + integration-fast).
# Slow suites (integration-real, e2e) are ignored by default.

set -e

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"

echo "=== Running Meerkat FAST Tests ==="
echo ""

echo "Running fast suite (unit + integration-fast)..."
if [ -n "${FAST_TARGET_DIR:-}" ]; then
  CARGO_TARGET_DIR="$FAST_TARGET_DIR" "$CARGO" test --workspace --lib --bins --tests
else
  "$CARGO" test --workspace --lib --bins --tests
fi

echo ""
echo "=== Fast Tests Complete ==="

echo ""
echo "Optional slow suites:"
echo "  $CARGO test --workspace integration_real -- --ignored --test-threads=1"
echo "  $CARGO test --workspace e2e_ -- --ignored --test-threads=1"
