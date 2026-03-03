#!/usr/bin/env bash
# Bump Python and TypeScript SDK versions to match the Rust workspace version.
#
# Usage:
#   ./scripts/bump-sdk-versions.sh          # reads version from Cargo.toml
#   ./scripts/bump-sdk-versions.sh 0.3.0    # explicit version
#
# Intended to be called as a cargo-release pre-release hook or manually.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [ $# -ge 1 ]; then
    VERSION="$1"
else
    VERSION=$(cargo metadata --manifest-path "$ROOT/Cargo.toml" \
        --no-deps --format-version 1 \
        | jq -r '.packages[] | select(.name == "meerkat") | .version')
fi

echo "Bumping SDK versions to $VERSION"

# ── Python SDK ──────────────────────────────────────────────────────────────

PYPROJECT="$ROOT/sdks/python/pyproject.toml"
if [ -f "$PYPROJECT" ]; then
    # Portable sed -i: macOS requires '' arg, GNU does not.
    if sed --version >/dev/null 2>&1; then
        sed -i "s/^version = \".*\"/version = \"$VERSION\"/" "$PYPROJECT"
    else
        sed -i '' "s/^version = \".*\"/version = \"$VERSION\"/" "$PYPROJECT"
    fi
    echo "  Updated: $PYPROJECT"
fi

# ── TypeScript SDK ──────────────────────────────────────────────────────────

PACKAGE_JSON="$ROOT/sdks/typescript/package.json"
if [ -f "$PACKAGE_JSON" ]; then
    # Use node to preserve JSON formatting
    node -e "
        const fs = require('fs');
        const pkg = JSON.parse(fs.readFileSync('$PACKAGE_JSON', 'utf8'));
        pkg.version = '$VERSION';
        fs.writeFileSync('$PACKAGE_JSON', JSON.stringify(pkg, null, 2) + '\n');
    "
    echo "  Updated: $PACKAGE_JSON"
fi

# ── Web SDK (@rkat/web) ───────────────────────────────────────────────────

WEB_PACKAGE_JSON="$ROOT/sdks/web/package.json"
if [ -f "$WEB_PACKAGE_JSON" ]; then
    node -e "
        const fs = require('fs');
        const pkg = JSON.parse(fs.readFileSync('$WEB_PACKAGE_JSON', 'utf8'));
        pkg.version = '$VERSION';
        fs.writeFileSync('$WEB_PACKAGE_JSON', JSON.stringify(pkg, null, 2) + '\n');
    "
    echo "  Updated: $WEB_PACKAGE_JSON"
fi

# Update EXPECTED_VERSION in web SDK runtime.ts
WEB_RUNTIME_TS="$ROOT/sdks/web/src/runtime.ts"
if [ -f "$WEB_RUNTIME_TS" ]; then
    if sed --version >/dev/null 2>&1; then
        sed -i "s/^const EXPECTED_VERSION = '.*';/const EXPECTED_VERSION = '$VERSION';/" "$WEB_RUNTIME_TS"
    else
        sed -i '' "s/^const EXPECTED_VERSION = '.*';/const EXPECTED_VERSION = '$VERSION';/" "$WEB_RUNTIME_TS"
    fi
    echo "  Updated: $WEB_RUNTIME_TS (EXPECTED_VERSION)"
fi

echo "Done. Run 'make verify-version-parity' to confirm."
