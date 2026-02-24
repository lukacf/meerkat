#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
WORK="$ROOT/.work"
MOB_DIR="$WORK/dashboard-copilot"
PACK="$WORK/dashboard-copilot.mobpack"
WEB_OUT="$WORK/dashboard-copilot-web"

mkdir -p "$MOB_DIR/skills" "$WORK"

cat > "$MOB_DIR/manifest.toml" <<'TOML'
[mobpack]
name = "dashboard-copilot"
version = "1.0.0"
description = "Embedded dashboard assistant"

[requires]
capabilities = ["comms"]
TOML

cat > "$MOB_DIR/definition.json" <<'JSON'
{
  "id":"dashboard-copilot",
  "orchestrator":{"profile":"ops"},
  "profiles":{
    "ops":{
      "model":"claude-sonnet-4-5",
      "skills":[],
      "tools":{"comms":true},
      "peer_description":"Ops copilot",
      "external_addressable":true
    }
  },
  "skills":{}
}
JSON

cat > "$MOB_DIR/skills/copilot.md" <<'MD'
# Dashboard Copilot

You watch streaming telemetry and:
- detect rollout anomalies
- summarize probable root cause
- propose next actions with risk notes
MD

rkat mob pack "$MOB_DIR" -o "$PACK"
rkat mob web build "$PACK" -o "$WEB_OUT"

# Show generated policy/profile info for embed integrators
cat "$WEB_OUT/manifest.web.toml"

echo "Embed idea: serve $WEB_OUT and mount in an iframe/panel in your internal dashboard."
