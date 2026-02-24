#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
WORK="$ROOT/.work"
MOB_DIR="$WORK/release-triage"
PACK="$WORK/release-triage.mobpack"
KEY="$WORK/release.key"

mkdir -p "$MOB_DIR/skills" "$WORK"

cat > "$MOB_DIR/manifest.toml" <<'TOML'
[mobpack]
name = "release-triage"
version = "1.0.0"
description = "Release triage mobpack example"

[requires]
capabilities = ["comms"]
TOML

cat > "$MOB_DIR/definition.json" <<'JSON'
{
  "id":"release-triage",
  "orchestrator":{"profile":"lead"},
  "profiles":{
    "lead":{
      "model":"claude-sonnet-4-5",
      "skills":[],
      "tools":{"comms":true},
      "peer_description":"Release lead",
      "external_addressable":true
    }
  },
  "skills":{}
}
JSON

cat > "$MOB_DIR/skills/triage.md" <<'MD'
# Release Triage

1. Read open incidents
2. Classify severity
3. Propose immediate mitigations
MD

# Example signing key (demo only)
printf '0707070707070707070707070707070707070707070707070707070707070707' > "$KEY"

rkat mob pack "$MOB_DIR" -o "$PACK" --sign "$KEY"
rkat mob inspect "$PACK"
rkat mob validate "$PACK"

# Deploy from exact artifact (use --trust-policy strict in real environments)
rkat mob deploy "$PACK" "triage the top 3 release regressions" --trust-policy permissive
