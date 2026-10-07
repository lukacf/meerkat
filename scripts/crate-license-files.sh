#!/usr/bin/env bash
# Print the license texts each crate must ship, derived from the crate's own
# `license` expression (one `cargo metadata --no-deps` call, no build).
#
# Usage: scripts/crate-license-files.sh crate...
# Output: one line per crate, tab-separated:
#   <crate> <expression> <space-separated required files>
# The files column is empty for an expression outside the closed set below,
# or when the crate is not exactly one workspace package. Callers fail
# closed on an empty column: decide which texts the expression needs, then
# add it here.

set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"

license_files_for() {
    case "$1" in
        "MIT OR Apache-2.0" | "Apache-2.0 OR MIT") echo "LICENSE-MIT LICENSE-APACHE" ;;
        "Apache-2.0") echo "LICENSE-APACHE" ;;
        "MIT") echo "LICENSE-MIT" ;;
        *) echo "" ;;
    esac
}

if ! metadata="$(cd "$ROOT" && "$CARGO" metadata --no-deps --format-version 1 2>/dev/null)"; then
    echo "cargo metadata failed; cannot read the crates' license expressions" >&2
    exit 1
fi

for crate in "$@"; do
    expression="$(jq -r --arg name "$crate" \
        '[.packages[] | select(.name == $name) | .license // "<none>"]
         | if length == 1 then .[0] else "<not one workspace package>" end' \
        <<< "$metadata")"
    printf '%s\t%s\t%s\n' "$crate" "$expression" "$(license_files_for "$expression")"
done
