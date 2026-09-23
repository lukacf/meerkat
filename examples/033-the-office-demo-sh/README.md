# 033 - The Office (WASM Multi-Agent Demo)

10 autonomous AI agents run an office together. Events arrive at the mail room, get triaged and routed to department specialists, personal assistants, and an archivist — all visualized as a pixel art office with phone call arcs, speech bubbles, and a knowledge graph.

## What it demonstrates

- **10 autonomous agents** in a single mob, each with distinct personality and responsibilities
- **Comms-based coordination** — canonical `peer_id` send requests are visualized as phone arcs (requests, not delivery confirmations)
- **Event-driven architecture** — external events flow through triage, fan out to specialists, accumulate knowledge
- **Human-in-the-loop** — the Gate agent routes high-risk actions to a human approval popup
- **Knowledge base** — the Archivist stores facts, viewable as an interactive force-directed graph
- **Structured output** — each agent cycle produces a headline and category for the UI
- **Browser WASM runtime** - all 10 agents run in-browser via
  `meerkat-web-runtime`

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
rustc --version # stable Rust toolchain
wasm-pack --version
node --version  # 20+
```

`examples.sh` rebuilds the repo-local `@rkat/web` WASM runtime before the Vite
build, so both the Rust toolchain and `wasm-pack` must be available on `PATH`.

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
(cd ../../sdks/web && npm install && npm run build:wasm)
cd web && npm install && npm run dev
```

`npm run dev`/`npm run build` sync an existing WASM runtime bundle from the
repo-local `sdks/web` package, or from an installed `@rkat/web` package, into
`web/public/meerkat-pkg`. Use `examples.sh` for the full rebuild path.

## Usage

1. Click **PRESS START**. If no key is configured, enter a key in the dialog and click **SAVE & START**.
2. To select a model first, cancel the dialog, open **Settings** (gear), enter a key and select a model, then close Settings.
3. Click the top-bar **Start** (or **Retry** after a startup failure). **LIVE** requires all 10 members, canonical peer targets, 26 comms wires and 10 event subscriptions. **Restart** tears down and recreates the office with the current settings.
4. Click one of the six named scenario buttons to inject an event.
5. Watch phone arcs light up as agents coordinate
6. Read speech bubbles for real-time agent communication
7. Read **LOG** for chronological headline previews, not a full-message tree. Initial inputs identify their scenario/chat; agent replies and host effects are explicitly **UNCORRELATED ACTIVITY**, not assigned to the newest scenario.
8. When Gate requests approval, expand its item and choose **APPROVE** or **DENY**. Failed delivery stays visible for retry of the same decision; an accepted inbox receipt is not proof of execution. Requests from a destroyed office remain marked expired and cannot be sent to a new Gate.
9. Click the filing cabinet in the Archive zone (or **RECORDS**) to view records; click **GRAPH** for the knowledge graph. Both visible views refresh after archive writes.
10. Choose an agent via the agent-name button and use the chat input and **Send**.
11. **Pause agents** awaits runtime stop and blocks new scenario/chat/approval work; **Resume agents** resumes the mob and renews peer targets and subscriptions. A failed transition displays an error and requires **Restart**; it is never reported as a successful pause.

The approval popup is a demo protocol implemented with a JavaScript tool and
agent instructions. It is not a security or authorization boundary. Archive
records also live only in browser memory and disappear on page reload.
The static Boss-mode policy is included in every initial role skill, before the
first turn. Startup does not append it as a later System message; support for
mid-conversation System messages depends on the selected model.
IT's access tools change this demo's in-memory comms topology, not credentials.
Partial/failed topology operations are reported rather than labelled successful.
Restoring one endpoint never reconnects another endpoint still marked disconnected.
Archive records remain visible across an office restart; pending approval requests
belong to the old runtime and expire. Event-stream lag is reported because missing
host tool effects cannot be reconstructed safely.
Replay protection retains the most recent 4,096 canonical event identities per
runtime; it never uses provider tool-call IDs or approval wording.

### Local regression checks

```bash
cd web
npm run typecheck
npm run build
npm test
npm run test:offline
```

The tests run Playwright's bundled headless Chromium (install it once with
`npx --prefix web playwright install chromium`; `CHROME_BIN` overrides the
binary, and `CHROME_SANDBOX=1` keeps the Chromium sandbox on), actual compiled
TypeScript/DOM handlers and real Cytoscape. The separate `test:offline` lane
requires a healthy built-page WASM bootstrap, stop/admission and resume.
It also verifies the unchanged admin policy in all ten initial role skills and
in each role's actual initial provider request.
The offline lane serves complete synthetic Anthropic responses only at the
expected messages endpoint. Its finite request bound is derived from the ten
initial turns and 52 possible directed terminal-kickoff notices, each with at most
one structured extraction, plus one isolated lifecycle control call: 125 requests
within three minutes. Per-role/phase checks reject repeated notice deliveries,
unexpected work and extra extraction calls, and teardown must leave no pending
fixture responses;
all other external traffic is blocked and synthetic keys are used. These checks
do not prove provider-backed comms delivery or an autonomous end-to-end scenario.
The strict offline lane intentionally fails if the runtime cannot establish the
required topology/subscriptions; roster size alone is not a healthy bootstrap.

### Server/Proxy Mode

```
http://127.0.0.1:4174?proxy=http://localhost:8080&model=claude-sonnet-4-6
```

Provider requests are routed to the configured proxy base URL, which must own
the real credentials and expose `/anthropic`, `/openai`, and `/gemini` routes.
The browser uses placeholder keys and auto-starts on load.

## Event Scenarios

1. **Client Escalation** — Acme Corp's Q4 deliverables are late
2. **Server Room Alert** — Temperature at 85°F, rising
3. **Expense Report** — $4,200 invoice from CloudCorp (triggers Gate approval)
4. **Calendar Conflict** — CTO double-booked
5. **New Hire Onboarding** — Casey Rivera starts Monday
6. **Security Breach** — Unusual admin-console login activity from a suspicious IP using the `deploy-bot` service account
