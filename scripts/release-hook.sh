#!/usr/bin/env bash
# Pre-release hook for cargo-release.
#
# Called with the new version as $1. Performs:
#   1. Bump Python + TypeScript SDK versions
#   2. Re-emit schemas + run SDK codegen
#   3. Stage the changed files for the release commit
#
# cargo-release calls this AFTER bumping Cargo.toml versions but BEFORE
# creating the release commit, so we can add SDK files to the same commit.

set -euo pipefail

VERSION="${1:?Usage: release-hook.sh <version>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

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

echo "==> Release hook complete"
