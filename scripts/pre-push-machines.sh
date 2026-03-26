#!/usr/bin/env bash
# If machine-related files changed, run codegen + verify
set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"

changed=$(git diff --cached --name-only HEAD | grep -E '(machine-schema/src/catalog|_authority\.rs|machine-kernels/src/generated)' || true)
if [ -n "$changed" ]; then
    echo "Machine files changed, running codegen + verify..."
    "$CARGO" xtask machine-codegen --all || exit 1
    "$CARGO" xtask machine-verify --all || exit 1
fi
