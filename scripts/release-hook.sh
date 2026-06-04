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
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"

case "${DRY_RUN:-}" in
    1|true|TRUE|yes|YES|on|ON)
        echo "==> Release hook: dry run detected; skipping mutating SDK/schema sync"
        exit 0
        ;;
esac

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
V_CORE="${VERSION%%-*}"
V_PRE=""
if [[ "$VERSION" == *-* ]]; then
    V_PRE="${VERSION#*-}"
fi
IFS='.' read -r V_MAJOR V_MINOR V_PATCH <<< "$V_CORE"
# Replace only the CURRENT const block (lines between "pub const CURRENT" and "};")
# Use temp file for portability (BSD sed -i '' vs GNU sed -i differ)
sed "/pub const CURRENT/,/};/{
    s/major: [0-9]*/major: $V_MAJOR/
    s/minor: [0-9]*/minor: $V_MINOR/
    s/patch: [0-9]*/patch: $V_PATCH/
}" "$VERSION_RS" > "${VERSION_RS}.tmp" && mv "${VERSION_RS}.tmp" "$VERSION_RS"
VERSION_PRERELEASE="$V_PRE" python3 - "$VERSION_RS" <<'PY'
from pathlib import Path
import os
import re
import sys

path = Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
pre = os.environ["VERSION_PRERELEASE"]
replacement = (
    f'pub const PRERELEASE: Option<&\'static str> = Some("{pre}");'
    if pre
    else "pub const PRERELEASE: Option<&'static str> = None;"
)
text = re.sub(
    r"pub const PRERELEASE: Option<&'static str> = .*;",
    replacement,
    text,
    count=1,
)
path.write_text(text, encoding="utf-8")
PY
echo "  Updated ContractVersion::CURRENT to $VERSION"

# 3. Regenerate schemas + SDK types
echo "==> Emitting schemas..."
"$CARGO" run -p meerkat-contracts --features schema --bin emit-schemas

echo "==> Running SDK codegen..."
python3 "$ROOT/tools/sdk-codegen/generate.py"

echo "==> Regenerating BuildBuddy BUILD files..."
make buildbuddy-generate

echo "==> Refreshing Bazel module lockfile..."
make buildbuddy-lock-update

# 3. Verify everything is in sync
echo "==> Verifying version parity..."
"$ROOT/scripts/verify-version-parity.sh"

echo "==> Verifying RPC surface alignment..."
"$ROOT/scripts/verify-rpc-surface-alignment.sh"

echo "==> Verifying SDK wrapper freshness..."
"$ROOT/scripts/verify-sdk-wrapper-freshness.sh"

# 4. Stage SDK and artifact files for the release commit
git add \
    "$ROOT/MODULE.bazel.lock" \
    "$ROOT/meerkat-contracts/src/version.rs" \
    "$ROOT/sdks/python/pyproject.toml" \
    "$ROOT/sdks/typescript/package.json" \
    "$ROOT/sdks/web/package.json" \
    "$ROOT/sdks/web/src/runtime.ts" \
    "$ROOT/sdks/web/src/generated/" \
    "$ROOT/sdks/python/meerkat/generated/" \
    "$ROOT/sdks/typescript/src/generated/" \
    "$ROOT/artifacts/schemas/"
git ls-files -z '*BUILD.bazel' | xargs -0 git add --

# 5. Mark as done for this version
echo "$VERSION" > "$SENTINEL"

echo "==> Release hook complete"
