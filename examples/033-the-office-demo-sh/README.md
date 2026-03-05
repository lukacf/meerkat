# 034 — The Office (WASM Multi-Agent Demo)

10 autonomous AI agents run an office together. Events arrive at the mail room, get triaged and routed to department specialists, personal assistants, and an archivist — all visualized as a pixel art office with phone call arcs, speech bubbles, and a knowledge graph.

## What it demonstrates

- **10 autonomous agents** in a single mob, each with distinct personality and responsibilities
- **Comms-based coordination** — agents call each other via the `send` tool, visualized as glowing phone arcs
- **Event-driven architecture** — external events flow through triage, fan out to specialists, accumulate knowledge
- **Human-in-the-loop** — the Gate agent routes high-risk actions to a human approval popup
- **Knowledge base** — the Archivist stores facts, viewable as an interactive force-directed graph
- **Structured output** — each agent cycle produces a headline and category for the UI
- **Full WASM runtime** — all 10 agents run in-browser via `meerkat-web-runtime`

## Agent Roster

| Agent | Role | Personality |
|-------|------|-------------|
| Max (Triage) | Routes incoming events | Brisk, efficient coordinator |
| Dev (IT) | Server alerts, provisioning | Laconic, technical |
| Robin (HR) | Policy, onboarding | Warm, thorough |
| Jordan (Facilities) | Physical space, maintenance | Practical, hands-on |
| Morgan (Finance) | Invoices, budgets | Precise, threshold-aware |
| Aria (Alex's PA) | Alex Chen's assistant | Proactive, family-aware |
| Scout (Sam's PA) | Sam Torres's assistant | Casual, efficiency-focused |
| Quinn (Pat's PA) | Pat Nakamura's assistant | Detail-oriented, vendor-savvy |
| Bailey (Gate) | Compliance checkpoint | Cautious, risk-evaluating |
| Sage (Archivist) | Knowledge base | Methodical, encyclopedic |

## Prerequisites

```bash
node --version  # 20+
```

## Build & Run

```bash
cd examples/033-the-office-demo-sh
chmod +x examples.sh && ./examples.sh
```

Then serve:
```bash
python3 -m http.server 4174 --directory web/dist
open http://127.0.0.1:4174
```

### Dev mode (with WASM runtime):
```bash
cd web && npm install && npm run dev
```

`npm run dev`/`npm run build` auto-sync the paired WASM runtime bundle from `@rkat/web` into `web/public/meerkat-pkg`.

## Usage

1. Open settings (gear icon), enter an API key (Anthropic, OpenAI, or Gemini)
2. Select a model
3. Click **Start** to initialize the 10-agent office
4. Click **+ Event** to inject a scenario (server alert, client escalation, etc.)
5. Watch phone arcs light up as agents coordinate
6. Read speech bubbles for real-time agent communication
7. Check the Incidents panel for the full message tree
8. When the Gate agent flags a high-risk action, an approval popup appears
9. Click the filing cabinet in the Archive zone to view the knowledge graph
10. Type messages to any agent via the chat input bar

### Server/Proxy Mode

```
http://127.0.0.1:4174?proxy=http://localhost:8080&model=claude-sonnet-4-6
```

API keys provided by the proxy server. Auto-starts on load.

## Event Scenarios

1. **Client Escalation** — Acme Corp's Q4 deliverables are late
2. **Server Room Alert** — Temperature at 85°F, rising
3. **Expense Report** — $4,200 invoice from CloudCorp (triggers Gate approval)
4. **Calendar Conflict** — CTO double-booked
5. **New Hire Onboarding** — Casey Rivera starts Monday
