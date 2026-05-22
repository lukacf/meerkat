#!/usr/bin/env bash

set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
PYTHON="${PYTHON:-$(command -v python3.11 2>/dev/null || command -v python3)}"

RELEASE_CRATES=()
while IFS= read -r crate; do
    RELEASE_CRATES+=("$crate")
done < <("$ROOT/scripts/release-rust-crates.sh")

"$PYTHON" "$ROOT/scripts/check_rust_release_packaging.py" "$ROOT" "${RELEASE_CRATES[@]}"

echo "Rust release config is valid"
