#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
WORK="$ROOT/.work"
MOB_DIR="$WORK/incident-war-room"
PACK="$WORK/incident-war-room.mobpack"
WEB_OUT="$WORK/incident-war-room-web"

mkdir -p "$MOB_DIR/skills" "$WORK"

cat > "$MOB_DIR/manifest.toml" <<'TOML'
[mobpack]
name = "incident-war-room"
version = "1.0.0"
description = "Browser deployable incident command mob"

[requires]
capabilities = ["comms"]
TOML

cat > "$MOB_DIR/definition.json" <<'JSON'
{
  "id":"incident-war-room",
  "orchestrator":{"profile":"commander"},
  "profiles":{
    "commander":{
      "model":"claude-sonnet-4-5",
      "skills":[],
      "tools":{"comms":true},
      "peer_description":"Incident commander",
      "external_addressable":true
    }
  },
  "skills":{}
}
JSON

cat > "$MOB_DIR/skills/incident.md" <<'MD'
# Incident Playbook

- Establish impact and scope
- Prioritize mitigation path
- Maintain a running status summary
MD

rkat mob pack "$MOB_DIR" -o "$PACK"
rkat mob web build "$PACK" -o "$WEB_OUT"

echo "Built web bundle: $WEB_OUT"
ls -1 "$WEB_OUT"

echo "Serve locally:"
echo "  cd '$WEB_OUT' && python3 -m http.server 4173"
