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

## The Office Scene

The office is pixel art on a 640x360 grid, scaled to the window with
nearest-neighbour sampling; names, speech bubbles and call lines are drawn at
full display resolution on top. Each workstation is layered (seated character,
then the desk that hides their legs, then the desk phone) and depth-sorted.

| Asset | Contents |
|-------|----------|
| `web/public/office/bg.png` | Room shell with empty rugs where the desks go |
| `web/public/office/chars/<agent>.png` | Four seated frames: typing A, typing B, on the phone, thinking |
| `web/public/office/desks/<agent>.png` | The agent's desk and its role-specific props |
| `web/public/office/props/` | Desk phone (on and off the hook), envelope, folder |
| `web/public/office/cast.png` | Title-screen lineup |

The art was generated with GPT Image 2.5 and then snapped to the grid:
downsampled, palette-quantized, and (for characters) every frame registered on
the chair base so the animation never jitters.

What the animations mean:

- A new event flies an envelope from the Mail Room slot into Triage's in-tray.
- A `send_message` request flies an envelope from the sender's desk phone to
  the recipient's, drawing a line behind it; the recipient's phone rings when
  it lands (a request, not a delivery confirmation). The line then settles into
  a slim trace for at least 6 seconds, so a conversation stays visible as a
  network. Agents hold the handset while sending and while any tool executes.
- A thought cloud means the agent's turn is running; idle agents type.
- Speech bubbles show what an agent said or did. A turn that changed nothing
  is logged without a bubble, unless it answers a message from you. At most
  four bubbles show at once, placed clear of faces.
- A stored archive record flies a folder into the filing cabinet.
- Hover or click an agent to see their role; the selected agent gets a marker.

## Agent Setup

- **Quiet start.** Every member runs one kickoff turn at spawn, prompted to
  wait silently. The runtime notifies a member's wired peers when its kickoff
  ends, and that notice wakes them, so the office subscribes first, waits for
  every kickoff to resolve (read from the mob roster), and only then wires the
  26 comms edges. A failed kickoff fails startup visibly instead of wiring a
  partial office.
- **Host tools with real results.** `upsert_record`, `revoke_access` and
  `restore_access` are `register_tool_callback` tools: the agent gets the
  actual outcome (record created/updated or rejected with the reason; topology
  change complete, partial or failed). `request_human_approval` stays
  fire-and-forget, because the human decides later and the decision reaches
  Gate as a `HUMAN DECISION` message.
- **Tools per role.** The browser runtime gives every member the same tool
  set, so each profile's `tools.deny` removes the task tools and every host
  tool the role does not own: only Gate requests approval, only the Archivist
  writes records, only IT changes access.

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
decision outcomes are displayed as written by the archivist, without inferred
approval verdicts. Records live only in browser memory and disappear on page reload.
Pause requests cooperative cancellation at agent boundaries and waits for the
runtime to drain before showing **STOPPED**; an outstanding provider response
or the current turn's tool batch can still delay that drain. The runtime retries
retained pending cleanup until those turns finish or its existing stop deadline
is reached; the UI stays **STOPPING** while it waits.
If resume reports a retained, retryable lifecycle operation, the UI stays
**RESUMING** and rejoins that operation for up to 90 seconds. New work remains
disabled until the runtime acknowledges resume; other errors require Restart.
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
TypeScript/DOM handlers and real Cytoscape. The 22 regression groups include
Graph, Records and Log geometry and scrolling at 650x735 and 1280x900, and
check that the office wires only after every kickoff resolves.
The separate `test:offline` lane
requires a healthy built-page WASM bootstrap, stop/admission and resume.
It also verifies the unchanged admin policy in all ten initial role skills and
in each role's actual initial provider request.
The offline lane serves complete synthetic Anthropic responses only at the
expected messages endpoint. Its finite request bound is derived from the ten
initial turns and 52 possible directed terminal-kickoff notices, each with at most
one structured extraction, plus one isolated lifecycle control call: 125 requests
within three minutes. That is a ceiling: because the office wires only after
the kickoffs resolve, a healthy bootstrap delivers no kickoff notices and makes
just the ten initial calls. Per-role/phase checks reject repeated notice deliveries,
unexpected work and extra extraction calls, and teardown must leave no pending
fixture responses;
all other external traffic is blocked and synthetic keys are used. These checks
do not prove provider-backed comms delivery or an autonomous end-to-end scenario.
The strict offline lane intentionally fails if the runtime cannot establish the
required topology/subscriptions; roster size alone is not a healthy bootstrap.

### Server/Proxy Mode

```
http://127.0.0.1:4174?proxy=http://localhost:8080&model=claude-sonnet-5-5
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
