#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
WORK="$ROOT/.work"
MOB_NORTH="$WORK/mob-north"
MOB_SOUTH="$WORK/mob-south"
PACK_NORTH="$WORK/north.mobpack"
PACK_SOUTH="$WORK/south.mobpack"
RUNTIME_OUT="$WORK/runtime"
WEB_DIR="$ROOT/web"
WEB_DIST="$WEB_DIR/dist"
WORKSPACE_ROOT="$(cd "$ROOT/../.." && pwd)"

if [[ -x "$WORKSPACE_ROOT/target/debug/rkat" ]]; then
  RKAT_BIN="$WORKSPACE_ROOT/target/debug/rkat"
elif [[ -x "$WORKSPACE_ROOT/target/release/rkat" ]]; then
  RKAT_BIN="$WORKSPACE_ROOT/target/release/rkat"
else
  RKAT_BIN="${RKAT_BIN:-rkat}"
fi

mkdir -p "$WORK" "$MOB_NORTH/skills" "$MOB_SOUTH/skills"

write_mob() {
  local dir="$1"
  local id="$2"
  local desc="$3"

  cat > "$dir/manifest.toml" <<TOML
[mobpack]
name = "$id"
version = "1.0.0"
description = "$desc"

[requires]
capabilities = ["comms"]
TOML

  cat > "$dir/definition.json" <<JSON
{
  "id":"$id",
  "orchestrator":{"profile":"planner"},
  "profiles":{
    "planner":{
      "model":"claude-sonnet-4-5",
      "skills":["game-rules","planner"],
      "tools":{"comms":true},
      "peer_description":"Strategic planner",
      "external_addressable":true
    },
    "operator":{
      "model":"claude-sonnet-4-5",
      "skills":["game-rules","operator"],
      "tools":{"comms":true},
      "peer_description":"Execution operator",
      "external_addressable":true
    }
  },
  "skills":{}
}
JSON

  cat > "$dir/skills/game-rules.md" <<'MD'
# Mini Diplomacy Rules

You are playing mini-diplomacy-v1.

Round phases:
1. Plan
2. Negotiate
3. Commit hidden order
4. Resolve simultaneously
5. Score update

Constraints:
- Never emit illegal actions.
- Output strictly valid JSON.
- Keep negotiation commitments explicit and traceable.
MD

  cat > "$dir/skills/planner.md" <<'MD'
# Planner Role

You optimize 2-3 turns ahead.

Produce:
- strategic intent
- expected opponent branch
- fallback branch
- compact rationale

Prioritize region value, defense posture, and deception risk.
MD

  cat > "$dir/skills/operator.md" <<'MD'
# Operator Role

Translate strategy into legal current-turn actions.

Checks before commit:
- target exists
- action remains legal
- fortify/aggression budget is coherent

Output a turn commit payload and execution status.
MD
}

write_mob "$MOB_NORTH" "mini-diplomacy-north" "North faction diplomacy mob"
write_mob "$MOB_SOUTH" "mini-diplomacy-south" "South faction diplomacy mob"

"$RKAT_BIN" mob pack "$MOB_NORTH" -o "$PACK_NORTH"
"$RKAT_BIN" mob pack "$MOB_SOUTH" -o "$PACK_SOUTH"
"$RKAT_BIN" mob web build "$PACK_NORTH" -o "$RUNTIME_OUT"

cd "$WEB_DIR"
npm install
npm run build

cp "$RUNTIME_OUT/runtime.js" "$WEB_DIST/runtime.js"
cp "$RUNTIME_OUT/runtime_bg.wasm" "$WEB_DIST/runtime_bg.wasm"
cp "$PACK_NORTH" "$WEB_DIST/north.mobpack"
cp "$PACK_SOUTH" "$WEB_DIST/south.mobpack"
cp "$RUNTIME_OUT/manifest.web.toml" "$WEB_DIST/manifest.web.toml"

PORT="${PORT:-4173}"
echo "Built arena app: $WEB_DIST"
echo "Serve with: python3 -m http.server \"$PORT\" --directory \"$WEB_DIST\""
echo "Then open: http://127.0.0.1:$PORT"
