#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
WORK="$ROOT/.work"
MOB_NORTH="$WORK/mob-north"
MOB_SOUTH="$WORK/mob-south"
MOB_EAST="$WORK/mob-east"
MOB_NARRATOR="$WORK/mob-narrator"
PACK_NORTH="$WORK/north.mobpack"
PACK_SOUTH="$WORK/south.mobpack"
PACK_EAST="$WORK/east.mobpack"
PACK_NARRATOR="$WORK/narrator.mobpack"
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

mkdir -p "$WORK" "$MOB_NORTH/skills" "$MOB_SOUTH/skills" "$MOB_EAST/skills" "$MOB_NARRATOR/skills"

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
# Mini Diplomacy — 3-Faction War

You are a faction commander in a 3-faction territory control game.

## Map
12 territories divided among 3 factions (North, South, East).
Each territory has: controller, defense (0-100), value (2-4 influence points).

## Turn Structure
1. You receive the current game state (all territories, scores, diplomatic signals)
2. You choose ONE enemy territory to target
3. You set aggression (0-100) and fortify (100 - aggression)
4. You send a diplomatic signal visible to ALL other factions
5. All orders resolve simultaneously

## Combat
If your aggression > target's defense: you capture it (defense resets).
Higher fortify strengthens your own territories.

## Diplomacy
With 3 factions, alliances matter. You can:
- Propose alliances ("let's both attack East")
- Backstab ("I said I'd help but I'm attacking you")
- Signal intentions (truthful or deceptive)
- Negotiate truces or joint operations

## Victory
After max_turns, faction with highest cumulative score wins.

## Output Format
Return ONLY valid JSON:
```json
{
  "order": {
    "aggression": <0-100>,
    "fortify": <0-100, must equal 100-aggression>,
    "target_region": "<exact region id>",
    "diplomacy": "<your diplomatic signal to all factions>"
  },
  "reasoning": "<2-3 sentences explaining your strategy>"
}
```
MD

  cat > "$dir/skills/planner.md" <<'MD'
# Planner Role

You optimize 2-3 turns ahead. Consider:
- Which faction is strongest? Can you ally against them?
- Are diplomatic signals trustworthy? Track broken promises.
- Region value: high-value targets are worth more risk.
- Defense posture: don't overextend, protect your capital.

Produce compact strategic reasoning before your order.
MD

  cat > "$dir/skills/operator.md" <<'MD'
# Operator Role

Validate your order before committing:
- Target must be an enemy territory (not your own)
- Aggression + fortify must equal exactly 100
- Diplomatic signal should align with (or deliberately contradict) your strategy

Output the JSON order and a brief execution note.
MD
}

write_mob "$MOB_NORTH" "mini-diplomacy-north" "North faction — defend the highlands"
write_mob "$MOB_SOUTH" "mini-diplomacy-south" "South faction — control the delta"
write_mob "$MOB_EAST" "mini-diplomacy-east" "East faction — dominate the frontier"

# ── Narrator mobpack (separate — not a faction) ──
cat > "$MOB_NARRATOR/manifest.toml" <<TOML
[mobpack]
name = "mini-diplomacy-narrator"
version = "1.0.0"
description = "War correspondent narrating the campaign"

[requires]
capabilities = ["comms"]
TOML

cat > "$MOB_NARRATOR/definition.json" <<JSON
{
  "id":"mini-diplomacy-narrator",
  "orchestrator":{"profile":"narrator"},
  "profiles":{
    "narrator":{
      "model":"claude-sonnet-4-5",
      "skills":["narrator"],
      "tools":{},
      "peer_description":"War correspondent",
      "external_addressable":true
    }
  },
  "skills":{}
}
JSON

cat > "$MOB_NARRATOR/skills/narrator.md" <<'MD'
# War Correspondent

You are a dramatic war correspondent covering a 3-faction territorial conflict.

After each turn you receive a summary of what happened: which territories were attacked,
which changed hands, what diplomatic signals were sent, and the current score.

Write 2-3 sentences of vivid, dramatic narrative. Channel the style of a war dispatch
or a fantasy chronicle. Name the factions (North, South, East) and describe the action
with tension and flair. Reference specific territories and diplomatic betrayals when relevant.

Keep it concise — this appears as a turn summary, not a novel. No JSON output needed,
just the narrative text.
MD

"$RKAT_BIN" mob pack "$MOB_NORTH" -o "$PACK_NORTH"
"$RKAT_BIN" mob pack "$MOB_SOUTH" -o "$PACK_SOUTH"
"$RKAT_BIN" mob pack "$MOB_EAST" -o "$PACK_EAST"
"$RKAT_BIN" mob pack "$MOB_NARRATOR" -o "$PACK_NARRATOR"
"$RKAT_BIN" mob web build "$PACK_NORTH" -o "$RUNTIME_OUT"

cd "$WEB_DIR"
npm install
npm run build

cp "$RUNTIME_OUT/runtime.js" "$WEB_DIST/runtime.js"
cp "$RUNTIME_OUT/runtime_bg.wasm" "$WEB_DIST/runtime_bg.wasm"
cp "$PACK_NORTH" "$WEB_DIST/north.mobpack"
cp "$PACK_SOUTH" "$WEB_DIST/south.mobpack"
cp "$PACK_EAST" "$WEB_DIST/east.mobpack"
cp "$PACK_NARRATOR" "$WEB_DIST/narrator.mobpack"
cp "$RUNTIME_OUT/manifest.web.toml" "$WEB_DIST/manifest.web.toml"

PORT="${PORT:-4173}"
echo "Built arena app: $WEB_DIST"
echo "Serve with: python3 -m http.server \"$PORT\" --directory \"$WEB_DIST\""
echo "Then open: http://127.0.0.1:$PORT"
