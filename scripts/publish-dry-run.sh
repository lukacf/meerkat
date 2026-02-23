#!/usr/bin/env bash

set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
JOBS="${MEERKAT_PUBLISH_DRY_RUN_JOBS:-4}"

if ! command -v xargs >/dev/null 2>&1; then
  echo "xargs is required for parallel publish checks"
  exit 1
fi

tmp_cfg=$(mktemp)
tmp_dir=$(mktemp -d)
trap 'rm -f "$tmp_cfg"; rm -rf "$tmp_dir"' EXIT

cat > "$tmp_cfg" <<EOF
[patch.crates-io]
meerkat-core = { path = "$ROOT/meerkat-core" }
meerkat-client = { path = "$ROOT/meerkat-client" }
meerkat-store = { path = "$ROOT/meerkat-store" }
meerkat-tools = { path = "$ROOT/meerkat-tools" }
meerkat-session = { path = "$ROOT/meerkat-session" }
meerkat-memory = { path = "$ROOT/meerkat-memory" }
meerkat-mcp = { path = "$ROOT/meerkat-mcp" }
meerkat-mcp-server = { path = "$ROOT/meerkat-mcp-server" }
meerkat-hooks = { path = "$ROOT/meerkat-hooks" }
meerkat-skills = { path = "$ROOT/meerkat-skills" }
meerkat-comms = { path = "$ROOT/meerkat-comms" }
meerkat-rpc = { path = "$ROOT/meerkat-rpc" }
meerkat-rest = { path = "$ROOT/meerkat-rest" }
meerkat-contracts = { path = "$ROOT/meerkat-contracts" }
meerkat = { path = "$ROOT/meerkat" }
meerkat-mob = { path = "$ROOT/meerkat-mob" }
meerkat-mob-mcp = { path = "$ROOT/meerkat-mob-mcp" }
rkat = { path = "$ROOT/meerkat-cli" }
EOF

LOG_DIR="$tmp_dir/logs"
mkdir -p "$LOG_DIR"

PACKAGES=(
  meerkat-core
  meerkat-contracts
  meerkat-client
  meerkat-store
  meerkat-tools
  meerkat-session
  meerkat-memory
  meerkat-mcp
  meerkat-mcp-server
  meerkat-hooks
  meerkat-skills
  meerkat-comms
  meerkat-rpc
  meerkat-rest
  meerkat
  meerkat-mob
  meerkat-mob-mcp
  rkat
)

run_publish() {
  pkg="$1"
  cfg="$2"
  log_file="$LOG_DIR/$pkg.log"
  result_file="$LOG_DIR/$pkg.result"
  if cargo publish -p "$pkg" --dry-run --allow-dirty --no-verify --config "$cfg" > "$log_file" 2>&1; then
    printf "%s:ok\n" "$pkg" > "$result_file"
  else
    printf "%s:fail\n" "$pkg" > "$result_file"
  fi
}

export -f run_publish
export LOG_DIR

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
