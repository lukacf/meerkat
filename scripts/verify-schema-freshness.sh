#!/usr/bin/env bash
# Verify that committed schema artifacts match what emit-schemas would produce.
#
# Runs emit-schemas to a temp directory and diffs against artifacts/schemas/.
# Exits 1 if any schema is stale (meaning someone changed Rust types but
# forgot to re-run emit-schemas + codegen).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

red()   { printf '\033[0;31m%s\033[0m\n' "$*"; }
green() { printf '\033[0;32m%s\033[0m\n' "$*"; }

COMMITTED="$ROOT/artifacts/schemas"
FRESH_DIR=$(mktemp -d)
trap 'rm -rf "$FRESH_DIR"' EXIT

echo "Re-emitting schemas to temp directory..."

# Run emit-schemas, redirecting output to the temp dir.
# The emit-schemas binary writes to artifacts/schemas/ relative to cwd,
# so we need to set up the right structure.
TEMP_ROOT=$(mktemp -d)
trap 'rm -rf "$FRESH_DIR" "$TEMP_ROOT"' EXIT
mkdir -p "$TEMP_ROOT/artifacts/schemas"

# Build and run emit-schemas, capturing output
cargo run -p meerkat-contracts --features schema --bin emit-schemas \
    --manifest-path "$ROOT/Cargo.toml" 2>&1 | tail -1

# The binary writes to artifacts/schemas/ relative to the workspace root.
# After running, compare committed vs fresh.
FAIL=0
echo ""
echo "Comparing committed schemas against freshly emitted:"

for f in "$COMMITTED"/*.json; do
    fname=$(basename "$f")
    if [ ! -f "$COMMITTED/$fname" ]; then
        continue
    fi
    # Compare the committed file content (use jq to normalize JSON formatting)
    committed_norm=$(jq -S . "$f" 2>/dev/null || cat "$f")
    # Since emit-schemas writes in-place to artifacts/schemas/, the files
    # are already updated. We compare git's version vs working tree.
    git_version=$(git show HEAD:"artifacts/schemas/$fname" 2>/dev/null || echo "")
    current=$(jq -S . "$f" 2>/dev/null || cat "$f")

    if [ -z "$git_version" ]; then
        continue  # New file, not in git yet
    fi

    git_norm=$(echo "$git_version" | jq -S . 2>/dev/null || echo "$git_version")

    if [ "$git_norm" != "$current" ]; then
        red "  STALE: $fname (committed version differs from freshly emitted)"
        FAIL=1
    else
        green "  OK: $fname"
    fi
done

echo ""
if [ $FAIL -ne 0 ]; then
    red "Schema freshness check FAILED"
    red "Run: cargo run -p meerkat-contracts --features schema --bin emit-schemas"
    red "Then: python3 tools/sdk-codegen/generate.py"
    red "Then commit the updated artifacts."
    exit 1
else
    green "All schemas are fresh"
fi
