#!/usr/bin/env bash
# Every published crate must ship the license texts its `license` field
# names, and no others. Cargo packages only files under the crate directory,
# so the workspace-root LICENSE-MIT and LICENSE-APACHE never reached a member
# crate's archive: 0.8.51 and earlier published all of them without license
# files. Each release crate carries symlinks to the root files, which
# `cargo package` follows. This gate asks cargo for the package file list,
# so it also catches an `include`/`exclude` that drops them.
#
# scripts/crate-license-files.sh derives the required texts from each crate's
# license expression. A crate licensed `Apache-2.0` alone must not ship
# LICENSE-MIT, which would misstate its licensing. An expression it does not
# know fails closed here.
#
# Usage: scripts/check-crate-license-files.sh [crate...]
# With no arguments it checks every crate in scripts/release-rust-crates.sh.

set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"
export ROOT CARGO
LICENSE_FILES=(LICENSE-MIT LICENSE-APACHE)

crates=("$@")
if [[ ${#crates[@]} -eq 0 ]]; then
    while IFS= read -r crate; do
        crates+=("$crate")
    done < <("$ROOT/scripts/release-rust-crates.sh")
fi

license_map="$("$(dirname "${BASH_SOURCE[0]}")/crate-license-files.sh" "${crates[@]}")"

fail=0
while IFS=$'\t' read -r crate expression required_files; do
    read -r -a required <<< "$required_files"
    if [[ ${#required[@]} -eq 0 ]]; then
        printf '  %-34sFAIL (unsupported license expression: %s)\n' "$crate" "$expression"
        fail=1
        continue
    fi
    if ! listing="$(cd "$ROOT" && "$CARGO" package --list -p "$crate" --allow-dirty 2>&1)"; then
        printf '  %-34sFAIL (cargo package --list failed)\n' "$crate"
        printf '%s\n' "$listing" | grep -E '^\s*error' | head -n 5 || true
        fail=1
        continue
    fi
    missing=()
    unexpected=()
    for file in "${LICENSE_FILES[@]}"; do
        if [[ " ${required[*]} " == *" $file "* ]]; then
            grep -qx "$file" <<< "$listing" || missing+=("$file")
        elif grep -qx "$file" <<< "$listing"; then
            unexpected+=("$file")
        fi
    done
    if [[ ${#missing[@]} -eq 0 && ${#unexpected[@]} -eq 0 ]]; then
        printf '  %-34sOK (%s)\n' "$crate" "$expression"
        continue
    fi
    problems=()
    [[ ${#missing[@]} -eq 0 ]] || problems+=("MISSING ${missing[*]}")
    [[ ${#unexpected[@]} -eq 0 ]] || problems+=("UNEXPECTED ${unexpected[*]} (license is $expression)")
    printf '  %-34s%s\n' "$crate" "${problems[*]}"
    fail=1
done <<< "$license_map"

if [[ "$fail" -ne 0 ]]; then
    echo "Some release crates do not package exactly the license texts their license field names." >&2
    echo "Add the named symlinks in the crate directory (ln -s ../../LICENSE-MIT crates/<crate>/, likewise LICENSE-APACHE)" >&2
    echo "and remove any license text the crate's license expression does not name." >&2
    exit 1
fi

echo "All release crates package the license texts their license field names"
