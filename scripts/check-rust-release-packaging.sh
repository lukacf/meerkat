#!/usr/bin/env bash

set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/meerkat-release-packaging-target}"

RELEASE_CRATES=()
while IFS= read -r crate; do
    RELEASE_CRATES+=("$crate")
done < <("$ROOT/scripts/release-rust-crates.sh")

python3 "$ROOT/scripts/check_rust_release_packaging.py" "$ROOT" "${RELEASE_CRATES[@]}"

for crate in "${RELEASE_CRATES[@]}"; do
    echo "Packaging $crate"
    tmp_cfg="$(mktemp)"
    "$ROOT/scripts/generate-patch-config.sh" "$ROOT" "$crate" > "$tmp_cfg"
    "$CARGO" package -p "$crate" --locked --allow-dirty --config "$tmp_cfg"
    rm -f "$tmp_cfg"
done

echo "All release crates package successfully"
