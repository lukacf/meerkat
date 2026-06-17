#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
WORK="$ROOT/.work"
SRC="$ROOT/mobpack"
PROMPTS="$ROOT/prompts"
MOB_DIR="$WORK/incident-war-room"
PACK="$WORK/incident-war-room.mobpack"
WEB_OUT="$WORK/incident-war-room-web"
WORKSPACE_ROOT="$(cd "$ROOT/../.." && pwd)"

resolve_rkat() {
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

RKAT="$(resolve_rkat)"

# `rkat mob web build` no longer compiles wasm itself; it requires a prebuilt
# meerkat-web-runtime artifact passed via --wasm. Use the committed one under
# sdks/web/wasm (override with MEERKAT_WASM=/path/to/*.wasm).
WASM_RUNTIME="${MEERKAT_WASM:-$WORKSPACE_ROOT/sdks/web/wasm/meerkat_web_runtime_bg.wasm}"
if [[ ! -s "$WASM_RUNTIME" ]]; then
  echo "error: meerkat-web-runtime wasm not found at $WASM_RUNTIME" >&2
  echo "Build it:  (cd \"$WORKSPACE_ROOT\" && wasm-pack build meerkat-web-runtime --target web --out-dir sdks/web/wasm)" >&2
  echo "or set MEERKAT_WASM=/path/to/meerkat_web_runtime_bg.wasm" >&2
  exit 1
fi

if [[ "${1:-}" == "--clean" ]]; then
  rm -rf "$WORK"
fi

rm -rf "$MOB_DIR" "$WEB_OUT"
mkdir -p "$WORK"
cp -R "$SRC" "$MOB_DIR"

echo "=== 029 — Web Incident War Room ==="
echo ""
echo "Source mobpack: $SRC"
echo "Working copy:   $MOB_DIR"
echo "Output bundle:  $WEB_OUT"
echo ""

echo "--- 1. Packing browser-safe incident war room mob ---"
"$RKAT" mob pack "$MOB_DIR" -o "$PACK"

echo ""
echo "--- 2. Inspecting artifact ---"
"$RKAT" mob inspect "$PACK"

echo ""
echo "--- 3. Building browser bundle ---"
# This is a locally-built, unsigned demo pack, so allow unsigned with a
# permissive trust policy (the default is strict, which rejects unsigned packs).
"$RKAT" mob web build "$PACK" -o "$WEB_OUT" --wasm "$WASM_RUNTIME" --trust-policy permissive

echo ""
echo "--- 4. Generated bundle contents ---"
ls -1 "$WEB_OUT"

echo ""
echo "--- 5. Derived web manifest ---"
cat "$WEB_OUT/manifest.web.toml"

echo ""
echo "--- 6. Suggested kickoff prompt ---"
cat "$PROMPTS/incident-kickoff.md"

echo ""
echo "Serve locally:"
echo "  cd '$WEB_OUT' && python3 -m http.server 4173"
echo ""
echo "Then open:"
echo "  http://127.0.0.1:4173"
echo ""
echo "Suggested user flow:"
echo "  1. Bring your own API key in the browser UI."
echo "  2. Paste the kickoff prompt above."
echo "  3. Ask the commander for a 15-minute incident plan, then request status updates each turn."
