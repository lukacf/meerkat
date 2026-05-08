#!/usr/bin/env bash

set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"
PYTHON="${PYTHON:-python3}"
JOBS="${MEERKAT_RELEASE_PACKAGING_JOBS:-${MEERKAT_PUBLISH_DRY_RUN_JOBS:-4}}"
TARGET_ROOT="${CARGO_TARGET_DIR:-/tmp/meerkat-release-packaging-target}"
ISOLATED_TARGETS="${MEERKAT_RELEASE_PACKAGING_ISOLATED_TARGETS:-0}"

RELEASE_CRATES=()
while IFS= read -r crate; do
    RELEASE_CRATES+=("$crate")
done < <("$ROOT/scripts/release-rust-crates.sh")

"$ROOT/scripts/check-rust-release-config.sh"

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

LOG_DIR="$tmp_dir/logs"
mkdir -p "$LOG_DIR"

if [[ "$ISOLATED_TARGETS" != 1 && "$ISOLATED_TARGETS" != true ]]; then
    mkdir -p "$TARGET_ROOT/package"
    rm -f "$TARGET_ROOT/package/meerkat-core"
    ln -s "$ROOT/meerkat-core" "$TARGET_ROOT/package/meerkat-core"
fi

run_package() {
    crate="$1"
    cfg="$LOG_DIR/$crate.config.toml"
    log_file="$LOG_DIR/$crate.log"
    result_file="$LOG_DIR/$crate.result"
    if [[ "$ISOLATED_TARGETS" == 1 || "$ISOLATED_TARGETS" == true ]]; then
        target_dir="$TARGET_ROOT/$crate"
    else
        target_dir="$TARGET_ROOT"
    fi

    if [[ "$ISOLATED_TARGETS" == 1 || "$ISOLATED_TARGETS" == true ]]; then
        mkdir -p "$target_dir/package"
        rm -f "$target_dir/package/meerkat-core"
        ln -s "$ROOT/meerkat-core" "$target_dir/package/meerkat-core"
    fi

    printf '  %-34sPACKAGING\n' "$crate"
    if "$ROOT/scripts/generate-patch-config.sh" "$ROOT" "$crate" > "$cfg" &&
        CARGO_TARGET_DIR="$target_dir" "$CARGO" package -p "$crate" --locked --allow-dirty --config "$cfg" > "$log_file" 2>&1; then
        printf '%s:ok\n' "$crate" > "$result_file"
    else
        printf '%s:fail\n' "$crate" > "$result_file"
    fi
}

export ROOT
export CARGO
export LOG_DIR
export TARGET_ROOT
export ISOLATED_TARGETS
export -f run_package

printf '%s\n' "${RELEASE_CRATES[@]}" |
    xargs -I{} -P "$JOBS" bash -lc 'run_package "$1"' _ "{}"

fail=0
for crate in "${RELEASE_CRATES[@]}"; do
    result_file="$LOG_DIR/$crate.result"
    if [[ ! -f "$result_file" ]]; then
        printf '  %-34sMISSING\n' "$crate"
        fail=1
        continue
    fi

    crate_result="$(cat "$result_file")"
    IFS=: read -r crate_name result <<< "$crate_result"
    if [[ "$result" == ok ]]; then
        printf '  %-34sOK\n' "$crate_name"
        continue
    fi

    fail=1
    printf '  %-34sFAIL\n' "$crate_name"
    if [[ -f "$LOG_DIR/$crate.log" ]]; then
        grep -nE "^\\s*error(:|\\[)" "$LOG_DIR/$crate.log" | head -n 20 || cat "$LOG_DIR/$crate.log"
    fi
done

if [[ "$fail" -ne 0 ]]; then
    echo "Some release crates failed to package"
    exit 1
fi

echo "Running published-style facade link smoke..."
if [[ "$ISOLATED_TARGETS" == 1 || "$ISOLATED_TARGETS" == true ]]; then
    "$ROOT/scripts/check-published-facade-link.sh"
else
    MEERKAT_PUBLISHED_FACADE_PACKAGE_TARGET="$TARGET_ROOT" \
        "$ROOT/scripts/check-published-facade-link.sh"
fi

echo "All release crates package successfully"
