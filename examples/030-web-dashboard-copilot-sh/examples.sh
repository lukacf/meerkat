#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
WORK="$ROOT/.work"
MOB_DIR="$WORK/dashboard-copilot"
PACK="$WORK/dashboard-copilot.mobpack"
WEB_OUT="$WORK/dashboard-copilot-web"
PLAYBOOK="$WORK/dashboard-context.json"
EMBED_SNIPPET="$WORK/embed-snippet.html"
QUESTIONS="$WORK/example-questions.md"
WORKSPACE_ROOT="$(cd "$ROOT/../.." && pwd)"

if [[ -x "$WORKSPACE_ROOT/target/debug/rkat" ]]; then
  RKAT_BIN="$WORKSPACE_ROOT/target/debug/rkat"
elif [[ -x "$WORKSPACE_ROOT/target/release/rkat" ]]; then
  RKAT_BIN="$WORKSPACE_ROOT/target/release/rkat"
else
  RKAT_BIN="${RKAT_BIN:-rkat}"
fi

mkdir -p "$MOB_DIR/skills" "$WORK"

cat > "$MOB_DIR/manifest.toml" <<'TOML'
[mobpack]
name = "dashboard-copilot"
version = "2.0.0"
description = "Embeddable release command center copilot for production dashboards"
authors = ["Meerkat examples"]

[requires]
meerkat = ">=0.4.7"
credentials = ["anthropic_api_key"]
capabilities = ["comms"]

[models]
default = "claude-sonnet-4-5"
TOML

cat > "$MOB_DIR/definition.json" <<'JSON'
{
  "id": "dashboard-copilot",
  "orchestrator": { "profile": "incident-commander" },
  "profiles": {
    "incident-commander": {
      "model": "claude-sonnet-4-5",
      "skills": ["incident-commander-playbook"],
      "peer_description": "Primary user-facing dashboard copilot. Triages telemetry, coordinates specialists, and summarizes operator actions.",
      "external_addressable": true,
      "tools": {
        "builtins": true,
        "comms": true,
        "mob": true,
        "mob_tasks": true
      }
    },
    "metrics-analyst": {
      "model": "claude-sonnet-4-5",
      "skills": ["metrics-analyst-playbook"],
      "peer_description": "Reads service health signals, spots regressions, and compares live metrics to rollout expectations.",
      "external_addressable": false,
      "tools": {
        "builtins": true,
        "comms": true,
        "mob_tasks": true
      }
    },
    "rollout-guard": {
      "model": "claude-sonnet-4-5",
      "skills": ["rollout-guard-playbook"],
      "peer_description": "Evaluates deployment risk, rollback criteria, blast radius, and operator safeguards.",
      "external_addressable": false,
      "tools": {
        "builtins": true,
        "comms": true,
        "mob_tasks": true
      }
    },
    "status-scribe": {
      "model": "claude-sonnet-4-5",
      "skills": ["status-scribe-playbook"],
      "peer_description": "Turns the team discussion into concise operator-ready status updates and next-step checklists.",
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
      { "a": "metrics-analyst", "b": "rollout-guard" },
      { "a": "metrics-analyst", "b": "status-scribe" },
      { "a": "rollout-guard", "b": "status-scribe" }
    ]
  },
  "skills": {
    "incident-commander-playbook": {
      "source": "path",
      "path": "skills/incident-commander.md"
    },
    "metrics-analyst-playbook": {
      "source": "path",
      "path": "skills/metrics-analyst.md"
    },
    "rollout-guard-playbook": {
      "source": "path",
      "path": "skills/rollout-guard.md"
    },
    "status-scribe-playbook": {
      "source": "path",
      "path": "skills/status-scribe.md"
    }
  }
}
JSON

cat > "$MOB_DIR/skills/incident-commander.md" <<'MD'
# Incident Commander Playbook

You are the dashboard copilot embedded next to service telemetry during a rollout.

## Mission
- Turn noisy dashboard inputs into an operator-grade recommendation.
- Ask the metrics analyst for signal interpretation.
- Ask rollout guard whether the current state is safe to continue.
- Ask the status scribe for a human-facing summary once the team converges.

## Operator contract
- Always answer in terms of dashboard-visible evidence.
- Distinguish clearly between:
  - what is happening now,
  - what changed after the rollout,
  - what the operator should do in the next 10 minutes.
- If the system looks unsafe, say "recommend rollback" explicitly and explain why.

## Output style
- Start with a one-line health verdict.
- Then give:
  1. evidence,
  2. likely cause,
  3. next action,
  4. risk if ignored.
MD

cat > "$MOB_DIR/skills/metrics-analyst.md" <<'MD'
# Metrics Analyst Playbook

Focus on telemetry interpretation, not final decision-making.

## Read signals like an SRE
- Compare current error rate, latency, saturation, and queue depth against the pre-rollout baseline.
- Treat simultaneous movement in error rate + latency + queue depth as strong evidence of user-visible degradation.
- Watch for partial-regression patterns:
  - one region degrading,
  - one tenant tier affected,
  - one dependency causing cascading latency.

## Reporting format
- Signal summary
- Confidence
- Most suspicious metric relationship
- What data would strengthen or weaken the hypothesis
MD

cat > "$MOB_DIR/skills/rollout-guard.md" <<'MD'
# Rollout Guard Playbook

You are the safety officer for production changes.

## Decide whether to continue, pause, or roll back
- Continue only if user-facing metrics remain healthy and degradation is within the stated error budget.
- Pause when the evidence is mixed and more observation time could disambiguate the cause.
- Recommend rollback when blast radius is growing or rollback criteria have already been met.

## Think in operational terms
- What is the rollback trigger?
- What is the likely blast radius if we wait 15 more minutes?
- What safeguards should be applied before resuming?

## Reporting format
- Recommendation: continue / pause / rollback
- Why now
- Confidence
- Safeguards or follow-up checks
MD

cat > "$MOB_DIR/skills/status-scribe.md" <<'MD'
# Status Scribe Playbook

Convert technical reasoning into communication an incident commander can paste into Slack, PagerDuty notes, or a release update.

## Produce
- a 2-3 sentence executive summary,
- a bullet list of operator next steps,
- a short "customer impact" line.

## Constraints
- Be concrete and time-bound.
- Do not invent metrics not present in the conversation.
- If the team is uncertain, say what we are watching next.
MD

cat > "$PLAYBOOK" <<'JSON'
{
  "service": "checkout-api",
  "rollout": {
    "version": "2026.03.13-rc2",
    "started_at": "2026-03-13T09:12:00Z",
    "scope_percent": 35,
    "operator": "release-bot"
  },
  "baseline": {
    "p95_latency_ms": 240,
    "error_rate_percent": 0.3,
    "queue_depth": 18
  },
  "current": {
    "p95_latency_ms": 780,
    "error_rate_percent": 2.8,
    "queue_depth": 91,
    "cpu_percent": 82
  },
  "regional_view": {
    "us-east-1": "healthy",
    "eu-west-1": "degraded",
    "ap-southeast-1": "healthy"
  },
  "notes": [
    "Spike started 7 minutes after enabling the rollout to 35%.",
    "Cart checkout failures are concentrated in eu-west-1.",
    "No database saturation alert yet, but request queue is climbing."
  ]
}
JSON

cat > "$EMBED_SNIPPET" <<'HTML'
<section class="ops-panel">
  <header>
    <h2>Release Command Center Copilot</h2>
    <p>Embed the generated web bundle beside latency/error charts and rollout controls.</p>
  </header>

  <iframe
    src="./dashboard-copilot-web/index.html"
    title="Dashboard Copilot"
    style="width: 100%; min-height: 720px; border: 0; border-radius: 16px;"
    loading="lazy"
  ></iframe>
</section>
HTML

cat > "$QUESTIONS" <<'MD'
# Example operator prompts

- "Summarize whether the current rollout should continue, pause, or roll back."
- "Which metrics suggest user-visible degradation versus background noise?"
- "Give me a Slack-ready status update for the incident channel."
- "What should I watch for over the next 10 minutes before increasing rollout scope?"
- "If we do not roll back now, what is the likely blast radius?"
MD

echo "=== Packing dashboard copilot mobpack ==="
"$RKAT_BIN" mob pack "$MOB_DIR" -o "$PACK"

echo
echo "=== Inspecting artifact ==="
"$RKAT_BIN" mob inspect "$PACK"

echo
echo "=== Validating artifact ==="
"$RKAT_BIN" mob validate "$PACK"

echo
echo "=== Building browser bundle ==="
"$RKAT_BIN" mob web build "$PACK" -o "$WEB_OUT"

echo
echo "=== Generated web manifest (derived output) ==="
cat "$WEB_OUT/manifest.web.toml"

echo
echo "=== Sample dashboard context ==="
cat "$PLAYBOOK"

echo
echo "=== Example operator prompts ==="
cat "$QUESTIONS"

echo
echo "Artifacts:"
echo "  mobpack:        $PACK"
echo "  web bundle:     $WEB_OUT"
echo "  dashboard JSON: $PLAYBOOK"
echo "  embed snippet:  $EMBED_SNIPPET"
echo
echo "Suggested next step:"
echo "  Serve '$WEB_OUT' from your dashboard host and inject the dashboard JSON as contextual state."
