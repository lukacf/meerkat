#!/usr/bin/env bash

set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"
PYTHON="${PYTHON:-$(command -v python3.11 2>/dev/null || command -v python3)}"
JOBS="${MEERKAT_RELEASE_PACKAGING_JOBS:-${MEERKAT_PUBLISH_DRY_RUN_JOBS:-4}}"
LANE_PREFIX="${RUST_LANE_ID:-release-packaging}"

RELEASE_CRATES=()
while IFS= read -r crate; do
    RELEASE_CRATES+=("$crate")
done < <("$ROOT/scripts/release-rust-crates.sh")

"$ROOT/scripts/check-rust-release-config.sh"

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
TARGET_ROOT="${CARGO_TARGET_DIR:-$tmp_dir/targets}"

LOG_DIR="$tmp_dir/logs"
mkdir -p "$LOG_DIR"

run_package() {
    crate="$1"
    cfg="$LOG_DIR/$crate.config.toml"
    log_file="$LOG_DIR/$crate.log"
    result_file="$LOG_DIR/$crate.result"
    target_dir="$TARGET_ROOT/$crate"
    lane_id="$LANE_PREFIX-$crate"

    mkdir -p "$target_dir/package"
    rm -f "$target_dir/package/meerkat-core"
    ln -s "$ROOT/meerkat-core" "$target_dir/package/meerkat-core"

    printf '  %-34sPACKAGING\n' "$crate"
    if "$ROOT/scripts/generate-patch-config.sh" "$ROOT" "$crate" > "$cfg" &&
        CARGO_TARGET_DIR="$target_dir" RUST_LANE_ID="$lane_id" \
            "$CARGO" package -p "$crate" --locked --allow-dirty --config "$cfg" > "$log_file" 2>&1; then
        printf '%s:ok\n' "$crate" > "$result_file"
    else
        printf '%s:fail\n' "$crate" > "$result_file"
    fi
}

export ROOT
export CARGO
export LOG_DIR
export TARGET_ROOT
export LANE_PREFIX
export -f run_package

# $1 expands in the child shell.
# shellcheck disable=SC2016
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

version="$(awk '
  /^\[workspace.package\]/ { in_workspace_package = 1; next }
  /^\[/ { in_workspace_package = 0 }
  in_workspace_package && $1 == "version" {
    gsub(/"/, "", $3)
    print $3
    exit
  }
' "$ROOT/Cargo.toml")"
if [[ -z "$version" ]]; then
    echo "failed to resolve workspace package version" >&2
    exit 1
fi
package_target="$tmp_dir/package-target"
mkdir -p "$package_target/package"
for crate in "${RELEASE_CRATES[@]}"; do
    archive="$TARGET_ROOT/$crate/package/$crate-$version.crate"
    if [[ ! -f "$archive" ]]; then
        echo "missing verified package archive: $archive" >&2
        exit 1
    fi
    cp "$archive" "$package_target/package/"
done

echo "Running published-style facade link smoke..."
MEERKAT_PUBLISHED_FACADE_PACKAGE_TARGET="$package_target" \
    "$ROOT/scripts/check-published-facade-link.sh"

echo "All release crates package successfully"
