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

# emit-schemas writes in-place to artifacts/schemas/ relative to its cwd. To
# avoid mutating the committed workspace tree while we validate, we run the
# emitter inside an isolated TEMP_ROOT (a copy of the workspace's
# artifacts/schemas/ seeded with the committed files) and diff committed vs
# freshly emitted entirely inside that temp dir. The real workspace tree is
# never written to, so `git status` stays clean after this gate runs.
TEMP_ROOT=$(mktemp -d)
trap 'rm -rf "$TEMP_ROOT"' EXIT
FRESH_DIR="$TEMP_ROOT/artifacts/schemas"
mkdir -p "$FRESH_DIR"

echo "Re-emitting schemas to temp directory..."

# Seed the temp artifacts dir with the committed files so the emitter overwrites
# them in place (matching its real-run behavior) without touching the workspace.
cp "$COMMITTED"/*.json "$FRESH_DIR"/ 2>/dev/null || true

# Build and run emit-schemas with cwd=TEMP_ROOT so it writes to
# "$TEMP_ROOT/artifacts/schemas" instead of the committed workspace tree.
# `--manifest-path` keeps the build pointed at the real workspace while the
# emitter's relative artifacts/schemas/ output lands in the temp dir.
(cd "$TEMP_ROOT" && "$CARGO" run -p meerkat-contracts --features schema --bin emit-schemas \
    --manifest-path "$ROOT/Cargo.toml" 2>&1 | tail -1)

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
    red "Schema freshness check FAILED"
    red "Run: ./scripts/repo-cargo run -p meerkat-contracts --features schema --bin emit-schemas"
    red "Then: python3 tools/sdk-codegen/generate.py"
    red "Then commit the updated artifacts."
    exit 1
else
    green "All schemas are fresh"
fi
