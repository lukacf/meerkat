#!/usr/bin/env bash

set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
PYTHON="$("$(dirname "${BASH_SOURCE[0]}")/require-python" 3.11 "$(basename "${BASH_SOURCE[0]}")")" || exit 1

RELEASE_CRATES=()
while IFS= read -r crate; do
    RELEASE_CRATES+=("$crate")
done < <("$ROOT/scripts/release-rust-crates.sh")

"$PYTHON" "$ROOT/scripts/check_rust_release_packaging.py" "$ROOT" "${RELEASE_CRATES[@]}"

echo "Rust release config is valid"
