#!/usr/bin/env bash
# Pre-release hook for cargo-release.
#
# Called with the new version as $1. Performs:
#   1. Bump Python + TypeScript SDK versions
#   2. Re-emit schemas + run SDK codegen
#   3. Stage the changed files for the release commit
#
# cargo-release calls this from each crate directory (with shared-version,
# it runs once per crate). We cd to the workspace root and use a sentinel
# file to ensure the work only happens once.

set -euo pipefail

VERSION="${1:?Usage: release-hook.sh <version>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# cargo-release runs this hook per-crate. Only execute once.
SENTINEL="$ROOT/.release-hook-done"
if [[ -f "$SENTINEL" ]] && [[ "$(cat "$SENTINEL")" == "$VERSION" ]]; then
    exit 0
fi

# All commands must run from workspace root (emit-schemas writes to ./artifacts/)
cd "$ROOT"

echo "==> Release hook: syncing SDK versions to $VERSION"

# 1. Bump SDK package versions
"$ROOT/scripts/bump-sdk-versions.sh" "$VERSION"

# 2. Regenerate schemas + SDK types
echo "==> Emitting schemas..."
cargo run -p meerkat-contracts --features schema --bin emit-schemas

echo "==> Running SDK codegen..."
python3 "$ROOT/tools/sdk-codegen/generate.py"

# 3. Verify everything is in sync
echo "==> Verifying version parity..."
"$ROOT/scripts/verify-version-parity.sh"

# 4. Stage SDK and artifact files for the release commit
git add \
    "$ROOT/sdks/python/pyproject.toml" \
    "$ROOT/sdks/typescript/package.json" \
    "$ROOT/sdks/python/meerkat/generated/" \
    "$ROOT/sdks/typescript/src/generated/" \
    "$ROOT/artifacts/schemas/"

# 5. Mark as done for this version
echo "$VERSION" > "$SENTINEL"

echo "==> Release hook complete"
