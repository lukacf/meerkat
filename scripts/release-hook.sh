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

# 2. Bump ContractVersion::CURRENT in version.rs to match package version
VERSION_RS="$ROOT/meerkat-contracts/src/version.rs"
IFS='.' read -r V_MAJOR V_MINOR V_PATCH <<< "$VERSION"
# Replace only the CURRENT const block (lines between "pub const CURRENT" and "};")
# Use temp file for portability (BSD sed -i '' vs GNU sed -i differ)
sed "/pub const CURRENT/,/};/{
    s/major: [0-9]*/major: $V_MAJOR/
    s/minor: [0-9]*/minor: $V_MINOR/
    s/patch: [0-9]*/patch: $V_PATCH/
}" "$VERSION_RS" > "${VERSION_RS}.tmp" && mv "${VERSION_RS}.tmp" "$VERSION_RS"
echo "  Updated ContractVersion::CURRENT to $VERSION"

# 3. Regenerate schemas + SDK types
echo "==> Emitting schemas..."
cargo run -p meerkat-contracts --features schema --bin emit-schemas

echo "==> Running SDK codegen..."
python3 "$ROOT/tools/sdk-codegen/generate.py"

# 3. Verify everything is in sync
echo "==> Verifying version parity..."
"$ROOT/scripts/verify-version-parity.sh"

# 4. Stage SDK and artifact files for the release commit
git add \
    "$ROOT/meerkat-contracts/src/version.rs" \
    "$ROOT/sdks/python/pyproject.toml" \
    "$ROOT/sdks/typescript/package.json" \
    "$ROOT/sdks/python/meerkat/generated/" \
    "$ROOT/sdks/typescript/src/generated/" \
    "$ROOT/artifacts/schemas/"

# 5. Mark as done for this version
echo "$VERSION" > "$SENTINEL"

echo "==> Release hook complete"
