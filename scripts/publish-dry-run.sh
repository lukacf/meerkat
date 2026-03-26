#!/usr/bin/env bash

set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
JOBS="${MEERKAT_PUBLISH_DRY_RUN_JOBS:-4}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"

if ! command -v xargs >/dev/null 2>&1; then
  echo "xargs is required for parallel publish checks"
  exit 1
fi

tmp_cfg=$(mktemp)
tmp_dir=$(mktemp -d)
trap 'rm -f "$tmp_cfg"; rm -rf "$tmp_dir"' EXIT

"$(dirname "$0")/generate-patch-config.sh" "$ROOT" > "$tmp_cfg"

LOG_DIR="$tmp_dir/logs"
mkdir -p "$LOG_DIR"

mapfile -t PACKAGES < <("$(dirname "$0")/release-rust-crates.sh")

run_publish() {
  pkg="$1"
  cfg="$2"
  log_file="$LOG_DIR/$pkg.log"
  result_file="$LOG_DIR/$pkg.result"
  if "$CARGO" publish -p "$pkg" --dry-run --allow-dirty --config "$cfg" > "$log_file" 2>&1; then
    printf "%s:ok\n" "$pkg" > "$result_file"
  else
    printf "%s:fail\n" "$pkg" > "$result_file"
  fi
}

export -f run_publish
export LOG_DIR
export CARGO

# shellcheck disable=SC2068
printf '%s\n' "${PACKAGES[@]}" \
  | xargs -n1 -I{} -P "$JOBS" bash -lc 'run_publish "$1" "$2"' _ "{}" "$tmp_cfg"

FAIL=0
for pkg in "${PACKAGES[@]}"; do
  result_file="$LOG_DIR/$pkg.result"
  if [ ! -f "$result_file" ]; then
    printf "  %-25sMISSING\n" "$pkg"
    FAIL=1
    continue
  fi

  pkg_result="$(cat "$result_file")"
  IFS=: read -r pkg_name result <<< "$pkg_result"
  if [ "$result" = ok ]; then
    printf "  %-25sOK\n" "$pkg_name"
    continue
  fi
  FAIL=1
  printf "  %-25sFAIL\n" "$pkg_name"
  if [ -f "$LOG_DIR/$pkg.log" ]; then
    grep -nE "^\\s*error(:|\\[)" "$LOG_DIR/$pkg.log" | head -n 20 || cat "$LOG_DIR/$pkg.log"
  fi
done

if [ "$FAIL" -ne 0 ]; then
  echo "Some crates are not publish-ready"
  exit 1
fi

echo "All crates are publish-ready"
