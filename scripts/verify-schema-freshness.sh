#!/usr/bin/env bash
# Verify that committed schema artifacts match what emit-schemas would produce.
#
# Runs emit-schemas to a temp directory and diffs against artifacts/schemas/.
# Exits 1 if any schema is stale (meaning someone changed Rust types but
# forgot to re-run emit-schemas + codegen).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"

red()   { printf '\033[0;31m%s\033[0m\n' "$*"; }
green() { printf '\033[0;32m%s\033[0m\n' "$*"; }

COMMITTED="$ROOT/artifacts/schemas"

# emit-schemas accepts an explicit output directory. Emit into a temporary
# directory and compare that exact output with the committed artifacts so the
# gate never depends on process cwd or mutates the workspace.
TEMP_ROOT=$(mktemp -d)
trap 'rm -rf "$TEMP_ROOT"' EXIT
FRESH_DIR="$TEMP_ROOT/artifacts/schemas"
mkdir -p "$FRESH_DIR"

echo "Re-emitting schemas to temp directory..."

# Seed the temp artifacts dir with the committed files so the emitter overwrites
# them in place (matching its real-run behavior) without touching the workspace.
cp "$COMMITTED"/*.json "$FRESH_DIR"/ 2>/dev/null || true

# `--manifest-path` keeps the build pointed at the real workspace and the
# argument after `--` binds the emitter's output location explicitly.
"$CARGO" run -p meerkat-contracts --features schema --bin emit-schemas \
    --manifest-path "$ROOT/Cargo.toml" -- "$FRESH_DIR" 2>&1 | tail -1

# Compare committed (workspace) vs freshly emitted (temp dir). The workspace
# tree is untouched, so any difference means the committed schemas are stale.
FAIL=0
echo ""
echo "Comparing committed schemas against freshly emitted:"

for f in "$COMMITTED"/*.json; do
    fname=$(basename "$f")
    fresh="$FRESH_DIR/$fname"
    if [ ! -f "$fresh" ]; then
        red "  MISSING: $fname (committed but not emitted)"
        FAIL=1
        continue
    fi
    # Normalize JSON formatting before comparing so cosmetic whitespace/key
    # ordering differences don't trip the gate.
    committed_norm=$(jq -S . "$f" 2>/dev/null || cat "$f")
    fresh_norm=$(jq -S . "$fresh" 2>/dev/null || cat "$fresh")

    if [ "$committed_norm" != "$fresh_norm" ]; then
        red "  STALE: $fname (committed version differs from freshly emitted)"
        FAIL=1
    else
        green "  OK: $fname"
    fi
done

echo ""
if [ $FAIL -ne 0 ]; then
    if [ -n "${MEERKAT_SCHEMA_DIAGNOSTIC_DIR:-}" ]; then
        rm -rf "$MEERKAT_SCHEMA_DIAGNOSTIC_DIR"
        mkdir -p "$MEERKAT_SCHEMA_DIAGNOSTIC_DIR"
        cp "$FRESH_DIR"/*.json "$MEERKAT_SCHEMA_DIAGNOSTIC_DIR"/
        red "Fresh schemas preserved at: $MEERKAT_SCHEMA_DIAGNOSTIC_DIR"
    fi
    red "Schema freshness check FAILED"
    red "Run: ./scripts/repo-cargo run -p meerkat-contracts --features schema --bin emit-schemas"
    red "Then: python3 tools/sdk-codegen/generate.py"
    red "Then commit the updated artifacts."
    exit 1
else
    green "All schemas are fresh"
fi
