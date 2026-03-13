#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
WORK="$ROOT/.work"
SRC="$ROOT/mobpack"
PROMPTS="$ROOT/prompts"
MOB_DIR="$WORK/incident-war-room"
PACK="$WORK/incident-war-room.mobpack"
WEB_OUT="$WORK/incident-war-room-web"

if [[ -x "$ROOT/../../target/debug/rkat" ]]; then
  RKAT="$ROOT/../../target/debug/rkat"
elif [[ -x "$ROOT/../../target/release/rkat" ]]; then
  RKAT="$ROOT/../../target/release/rkat"
else
  RKAT="${RKAT:-rkat}"
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
"$RKAT" mob web build "$PACK" -o "$WEB_OUT"

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
