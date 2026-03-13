#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
WORK="$ROOT/.work"
MOB_DIR="$WORK/release-triage"
PACK="$WORK/release-triage.mobpack"
KEY="$WORK/release.key"
RKAT="${RKAT:-rkat}"

rm -rf "$MOB_DIR"
mkdir -p "$MOB_DIR/skills" "$MOB_DIR/config" "$WORK"

cat > "$MOB_DIR/manifest.toml" <<'TOML'
[mobpack]
name = "release-triage"
version = "1.0.0"
description = "Portable multi-role mobpack for release regression triage and rollback planning"

[requires]
capabilities = ["comms"]

[models]
default = "claude-sonnet-4-5"
lead = "claude-opus-4-6"
TOML

cat > "$MOB_DIR/config/defaults.toml" <<'TOML'
[agent]
model = "claude-sonnet-4-5"
structured_output_retries = 2

[budget]
max_tokens_per_turn = 8192
warning_threshold = 0.8
TOML

cat > "$MOB_DIR/definition.json" <<'JSON'
{
  "id": "release-triage",
  "orchestrator": { "profile": "lead" },
  "profiles": {
    "lead": {
      "model": "claude-opus-4-6",
      "skills": ["lead-playbook"],
      "peer_description": "Release lead who owns severity, decisions, and executive updates",
      "external_addressable": true,
      "tools": {
        "builtins": true,
        "comms": true,
        "mob": true,
        "mob_tasks": true
      }
    },
    "signal-analyst": {
      "model": "claude-sonnet-4-5",
      "skills": ["signal-analysis"],
      "peer_description": "Observability analyst who correlates error spikes, deploy windows, and blast radius",
      "external_addressable": false,
      "tools": {
        "builtins": true,
        "comms": true,
        "mob_tasks": true
      }
    },
    "customer-ops": {
      "model": "claude-sonnet-4-5",
      "skills": ["customer-impact"],
      "peer_description": "Support liaison who summarizes customer impact, revenue exposure, and communication needs",
      "external_addressable": false,
      "tools": {
        "builtins": true,
        "comms": true,
        "mob_tasks": true
      }
    },
    "rollback-chief": {
      "model": "claude-sonnet-4-5",
      "skills": ["rollback-planning"],
      "peer_description": "Deployment specialist who prepares rollback, mitigation, and verification plans",
      "external_addressable": false,
      "tools": {
        "builtins": true,
        "comms": true,
        "mob_tasks": true
      }
    }
  },
  "wiring": {
    "auto_wire_orchestrator": true,
    "role_wiring": [
      { "a": "signal-analyst", "b": "rollback-chief" },
      { "a": "customer-ops", "b": "rollback-chief" }
    ]
  },
  "skills": {
    "lead-playbook": {
      "source": "path",
      "path": "skills/lead-playbook.md"
    },
    "signal-analysis": {
      "source": "path",
      "path": "skills/signal-analysis.md"
    },
    "customer-impact": {
      "source": "path",
      "path": "skills/customer-impact.md"
    },
    "rollback-planning": {
      "source": "path",
      "path": "skills/rollback-planning.md"
    }
  }
}
JSON

cat > "$MOB_DIR/skills/lead-playbook.md" <<'MD'
# Release Triage Lead

## Mission
Own the first 15 minutes of a release incident. Decide severity, coordinate specialists,
and produce a decisive operator brief.

## Inputs You Should Demand
- deploy timestamp and version identifier
- current error-rate or latency anomaly
- affected services, tenants, and user journeys
- mitigation already attempted
- customer-facing symptoms

## Working Pattern
1. Create tasks for signal analysis, customer impact, and rollback planning.
2. Ask each specialist for a concise update with evidence, not guesses.
3. Reconcile conflicting signals and explicitly call out uncertainty.
4. Produce a release triage brief with:
   - severity
   - suspected blast radius
   - recommended action: continue, halt rollout, partial rollback, full rollback
   - next 3 actions
   - stakeholder update draft

## Tone
Calm, operational, and concrete. Favor reversible actions and short feedback loops.
MD

cat > "$MOB_DIR/skills/signal-analysis.md" <<'MD'
# Signal Analyst

You are the observability specialist for a production release triage room.

## Focus
- correlate deploy time with metric changes
- identify which symptoms are new vs pre-existing
- estimate blast radius by service, route, or tenant cohort
- separate likely causal signals from noise

## Output Format
- `Signals:` 3-5 bullet facts
- `Most likely regression:` one sentence
- `Confidence:` low/medium/high
- `What to verify next:` 1-2 checks
MD

cat > "$MOB_DIR/skills/customer-impact.md" <<'MD'
# Customer Ops Analyst

You represent support, success, and commercial impact during a bad release.

## Focus
- which customers or segments are affected
- whether the issue blocks revenue, onboarding, or renewals
- whether there is already external visibility
- what internal/external updates are needed now

## Output Format
- `Affected users:` concise summary
- `Business impact:` low/medium/high with one reason
- `Support guidance:` what support should say right now
- `Exec note:` two-sentence stakeholder update
MD

cat > "$MOB_DIR/skills/rollback-planning.md" <<'MD'
# Rollback Chief

You prepare the safest mitigation path for a bad release.

## Focus
- rollback vs feature-flag disable vs canary halt
- prerequisites and risks for each option
- verification steps after mitigation
- owner handoff and timing

## Output Format
- `Recommended mitigation:` one line
- `Why this path:` one short paragraph
- `Execution checklist:` 3-5 bullets
- `Rollback risk:` low/medium/high with one sentence
MD

echo "==> Building release-triage mobpack workspace in $MOB_DIR"
find "$MOB_DIR" -maxdepth 2 -type f | sed "s#^$MOB_DIR/#  - #"

# Demo signing key for local verification only.
printf '0707070707070707070707070707070707070707070707070707070707070707' > "$KEY"

echo ""
echo "==> Packing and signing portable artifact"
"$RKAT" mob pack "$MOB_DIR" -o "$PACK" --sign "$KEY"

echo ""
echo "==> Inspecting artifact contents"
"$RKAT" mob inspect "$PACK"

echo ""
echo "==> Validating artifact contract"
"$RKAT" mob validate "$PACK"

echo ""
echo "==> Deploying the exact signed artifact for a realistic incident prompt"
"$RKAT" mob deploy "$PACK" \
  "You are triaging release 2026.03.13-rc4. Five minutes after rollout, checkout error rate jumped from 0.4% to 7.8%, p95 latency doubled for EU traffic, and support has 14 fresh tickets from enterprise tenants. Determine severity, probable blast radius, whether to halt or roll back, and draft the first stakeholder update." \
  --trust-policy permissive
