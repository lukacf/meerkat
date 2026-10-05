#!/usr/bin/env bash
# Every published crate must ship both license texts its `license` field
# names. Cargo packages only files under the crate directory, so the
# workspace-root LICENSE-MIT and LICENSE-APACHE never reached a member
# crate's archive: 0.8.51 and earlier published all of them without license
# files. Each release crate carries symlinks to the root files, which
# `cargo package` follows. This gate asks cargo for the package file list,
# so it also catches an `include`/`exclude` that drops them.
#
# Usage: scripts/check-crate-license-files.sh [crate...]
# With no arguments it checks every crate in scripts/release-rust-crates.sh.

set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"
REQUIRED_FILES=(LICENSE-MIT LICENSE-APACHE)

crates=("$@")
if [[ ${#crates[@]} -eq 0 ]]; then
    while IFS= read -r crate; do
        crates+=("$crate")
    done < <("$ROOT/scripts/release-rust-crates.sh")
fi

fail=0
for crate in "${crates[@]}"; do
    if ! listing="$(cd "$ROOT" && "$CARGO" package --list -p "$crate" --allow-dirty 2>&1)"; then
        printf '  %-34sFAIL (cargo package --list failed)\n' "$crate"
        printf '%s\n' "$listing" | grep -E '^\s*error' | head -n 5 || true
        fail=1
        continue
    fi
    missing=()
    for file in "${REQUIRED_FILES[@]}"; do
        if ! grep -qx "$file" <<< "$listing"; then
            missing+=("$file")
        fi
    done
    if [[ ${#missing[@]} -eq 0 ]]; then
        printf '  %-34sOK\n' "$crate"
    else
        printf '  %-34sMISSING %s\n' "$crate" "${missing[*]}"
        fail=1
    fi
done

if [[ "$fail" -ne 0 ]]; then
    echo "Some release crates do not package LICENSE-MIT and LICENSE-APACHE." >&2
    echo "Add symlinks in the crate directory: ln -s ../../LICENSE-MIT ../../LICENSE-APACHE crates/<crate>/" >&2
    exit 1
fi

echo "All release crates package LICENSE-MIT and LICENSE-APACHE"
