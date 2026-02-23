#!/usr/bin/env bash
# Verify version parity across Rust workspace, Python SDK, TypeScript SDK,
# contract version, and generated schema artifacts.
#
# Exit 0 if everything is in sync, exit 1 with diagnostics on any mismatch.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FAIL=0

red()    { printf '\033[0;31m%s\033[0m\n' "$*"; }
green()  { printf '\033[0;32m%s\033[0m\n' "$*"; }
yellow() { printf '\033[0;33m%s\033[0m\n' "$*"; }

# ── 1. Package version parity ──────────────────────────────────────────────

CARGO_VER=$(cargo metadata --manifest-path "$ROOT/Cargo.toml" \
    --no-deps --format-version 1 \
    | jq -r '.packages[] | select(.name == "meerkat") | .version')

PY_VER=$(python3 -c "
import pathlib
try:
    import tomllib  # py311+
except ModuleNotFoundError:
    import tomli as tomllib  # py310 fallback
d = tomllib.loads(pathlib.Path('$ROOT/sdks/python/pyproject.toml').read_text())
print(d['project']['version'])
")

TS_VER=$(node -p "require('$ROOT/sdks/typescript/package.json').version")

echo "Package versions:"
echo "  Cargo (meerkat):  $CARGO_VER"
echo "  Python SDK:       $PY_VER"
echo "  TypeScript SDK:   $TS_VER"

if [ "$CARGO_VER" != "$PY_VER" ] || [ "$CARGO_VER" != "$TS_VER" ]; then
    red "FAIL: package version mismatch"
    FAIL=1
else
    green "  Package versions: OK"
fi

# ── 2. Contract version parity ─────────────────────────────────────────────
# Source of truth: ContractVersion::CURRENT in meerkat-contracts/src/version.rs

RUST_CONTRACT=$(sed -n '/pub const CURRENT/,/};/{
    s/.*major: \([0-9]*\).*/\1/p
    s/.*minor: \([0-9]*\).*/\1/p
    s/.*patch: \([0-9]*\).*/\1/p
}' "$ROOT/meerkat-contracts/src/version.rs" | paste -sd. -)

ARTIFACT_CONTRACT=""
if [ -f "$ROOT/artifacts/schemas/version.json" ]; then
    ARTIFACT_CONTRACT=$(jq -r '.contract_version' "$ROOT/artifacts/schemas/version.json")
fi

PY_CONTRACT=$(sed -n 's/^CONTRACT_VERSION = "\(.*\)"/\1/p' \
    "$ROOT/sdks/python/meerkat/generated/types.py" 2>/dev/null || echo "")

TS_CONTRACT=$(sed -n 's/^export const CONTRACT_VERSION = "\(.*\)";/\1/p' \
    "$ROOT/sdks/typescript/src/generated/types.ts" 2>/dev/null || echo "")

echo ""
echo "Contract versions:"
echo "  Rust (version.rs):          $RUST_CONTRACT"
echo "  Artifact (version.json):    ${ARTIFACT_CONTRACT:-<missing>}"
echo "  Python SDK (generated):     ${PY_CONTRACT:-<missing>}"
echo "  TypeScript SDK (generated): ${TS_CONTRACT:-<missing>}"

CONTRACT_OK=true
if [ -n "$ARTIFACT_CONTRACT" ] && [ "$RUST_CONTRACT" != "$ARTIFACT_CONTRACT" ]; then
    red "FAIL: artifacts/schemas/version.json is stale (run: cargo run -p meerkat-contracts --features schema --bin emit-schemas)"
    CONTRACT_OK=false
    FAIL=1
fi
if [ -n "$PY_CONTRACT" ] && [ "$RUST_CONTRACT" != "$PY_CONTRACT" ]; then
    red "FAIL: Python SDK CONTRACT_VERSION is stale (run: python3 tools/sdk-codegen/generate.py)"
    CONTRACT_OK=false
    FAIL=1
fi
if [ -n "$TS_CONTRACT" ] && [ "$RUST_CONTRACT" != "$TS_CONTRACT" ]; then
    red "FAIL: TypeScript SDK CONTRACT_VERSION is stale (run: python3 tools/sdk-codegen/generate.py)"
    CONTRACT_OK=false
    FAIL=1
fi
if [ "$RUST_CONTRACT" != "$CARGO_VER" ]; then
    red "FAIL: contract version ($RUST_CONTRACT) != package version ($CARGO_VER)"
    red "  Run: scripts/release-hook.sh or manually update meerkat-contracts/src/version.rs"
    CONTRACT_OK=false
    FAIL=1
fi
if $CONTRACT_OK; then
    green "  Contract versions: OK"
fi

# ── 3. Internal crate dep versions ─────────────────────────────────────────
# All internal deps in workspace Cargo.toml should match workspace.package.version

echo ""
echo "Internal dependency versions:"
INTERNAL_OK=true
while IFS= read -r line; do
    # Extract: crate-name = { version = "X.Y.Z", ... }
    dep_name=$(echo "$line" | sed 's/ *=.*//')
    dep_ver=$(echo "$line" | sed -n 's/.*version = "\([^"]*\)".*/\1/p')
    if [ -n "$dep_ver" ] && [ "$dep_ver" != "$CARGO_VER" ]; then
        red "  FAIL: $dep_name has version \"$dep_ver\", expected \"$CARGO_VER\""
        INTERNAL_OK=false
        FAIL=1
    fi
done < <(grep -E '^meerkat' "$ROOT/Cargo.toml" | grep 'version = ')

if $INTERNAL_OK; then
    green "  Internal deps: OK"
fi

# ── Summary ─────────────────────────────────────────────────────────────────

echo ""
if [ $FAIL -ne 0 ]; then
    red "Version parity check FAILED"
    exit 1
else
    green "All version parity checks passed"
fi
