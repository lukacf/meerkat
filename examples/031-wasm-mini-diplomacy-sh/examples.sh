#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
WORK="$ROOT/.work"
MOB_RUNTIME="$WORK/mob-runtime"
PACK_RUNTIME="$WORK/runtime.mobpack"
RUNTIME_OUT="$WORK/runtime"
WEB_DIR="$ROOT/web"
WEB_DIST="$WEB_DIR/dist"
WORKSPACE_ROOT="$(cd "$ROOT/../.." && pwd)"

resolve_rkat() {
  if [[ -n "${RKAT_BIN:-}" ]]; then
    printf '%s\n' "$RKAT_BIN"
    return
  fi
  if [[ -n "${RKAT:-}" ]]; then
    printf '%s\n' "$RKAT"
    return
  fi

  local candidate
  for candidate in \
    "$WORKSPACE_ROOT/target/debug/rkat" \
    "$WORKSPACE_ROOT/target/release/rkat"
  do
    if [[ -x "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return
    fi
  done

  if [[ -x "$WORKSPACE_ROOT/scripts/repo-cargo" ]]; then
    local target_dir
    target_dir="$("$WORKSPACE_ROOT/scripts/repo-cargo" --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')"
    for candidate in "$target_dir/debug/rkat" "$target_dir/release/rkat"; do
      if [[ -x "$candidate" ]]; then
        printf '%s\n' "$candidate"
        return
      fi
    done
  fi

  printf '%s\n' "rkat"
}

RKAT_BIN="$(resolve_rkat)"

mkdir -p "$WORK" "$MOB_RUNTIME"

# ── Minimal mobpack (only needed to trigger `rkat mob web build`) ──
# The actual faction/narrator definitions are constructed inline in TypeScript
# via init_runtime_from_config + mob_create.

cat > "$MOB_RUNTIME/manifest.toml" <<TOML
[mobpack]
name = "diplomacy-runtime"
version = "1.0.0"
description = "WASM runtime for mini-diplomacy arena"
TOML

cat > "$MOB_RUNTIME/definition.json" <<JSON
{
  "id": "diplomacy-runtime",
  "profiles": {
    "default": {
      "model": "claude-sonnet-4-6",
      "peer_description": "default"
    }
  }
}
JSON

"$RKAT_BIN" mob pack "$MOB_RUNTIME" -o "$PACK_RUNTIME"
"$RKAT_BIN" mob web build "$PACK_RUNTIME" -o "$RUNTIME_OUT"

cd "$WEB_DIR"
npm install
npm run build

cp "$RUNTIME_OUT/runtime.js" "$WEB_DIST/runtime.js"
cp "$RUNTIME_OUT/runtime_bg.wasm" "$WEB_DIST/runtime_bg.wasm"

PORT="${PORT:-4173}"
echo "Built arena app: $WEB_DIST"
echo "Serve with: python3 -m http.server \"$PORT\" --directory \"$WEB_DIST\""
echo "Then open: http://127.0.0.1:$PORT"
