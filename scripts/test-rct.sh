#!/bin/bash
# Run the fast deterministic suite for Meerkat (unit + integration-fast).
# The e2e lanes live behind dedicated cargo aliases.

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
echo "  $CARGO e2e-system"
echo "  $CARGO e2e-live"
echo "  $CARGO e2e-smoke"
