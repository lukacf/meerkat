# 031 - WASM Mini Diplomacy Arena (Shell + Web)

Flagship browser example: **9 autonomous AI agents** across 3 factions wage a territory war with real-time strategy, diplomacy, and deception — all running in-browser via the Meerkat WASM runtime.

> **Current-runtime compatibility:** The checked-in narrator polling reads the
> legacy top-level flow status, but raw `mob_flow_status` now returns a JSON
> envelope `{ run: MobRun | null }`. As a result, the frontend does not display
> the completed narrative in the **Correspondent** channel / **War Correspondent**
> panel with the current runtime. A separate consumer migration must read `result.run` and
> handle `run: null` before examining status and output. That is a necessary
> migration, not a guarantee that all demo behavior is repaired; the flow
> engine itself is not shown to have failed by this display mismatch.

## What it demonstrates

This is primarily a **smoke test for the Meerkat WASM platform**, exercising:

- **In-memory autonomous host loops on WASM** - agents wake on comms messages and make LLM calls inside `wasm32-unknown-unknown`
- **Multi-mob orchestration** — 4 mobs (3 factions + narrator) created and managed via `MobMcpState`
- **Cross-mob comms** — ambassadors in different mobs discover each other and negotiate via `InprocRegistry` cross-namespace routing
- **Flow engine on WASM** — narrator mob uses a `turn_driven` flow for structured JSON output
- **Event streaming** - subscription `StreamRef` handles polled with
  `poll_subscription()` for real-time UI updates
- **Inline skills** — per-agent system prompts delivered via `SkillSource::Inline` in the mob definition

## Architecture

### 3 Faction Mobs (9 autonomous agents)

Each faction (France, Prussia, Russia) is a mob with 3 `autonomous_host` agents:

| Role | Responsibility | Peers |
|------|---------------|-------|
| **Planner** | Strategic analysis, coordinates team, briefs ambassador | Operator, Ambassador |
| **Operator** | Combat math, challenges plans, produces final orders | Planner |
| **Ambassador** | Diplomatic deception, negotiates with foreign ambassadors | Planner, Foreign Ambassadors |

Agents converse freely via comms tools (`send_message`, `peers`). No flows — conversations emerge organically.

### 1 Narrator Mob (turn-driven flow)

A separate mob with a single `turn_driven` agent. After each turn resolves, a
flow is intended to turn the conversation logs from all 9 agents into dramatic
narrative with omniscient perspective. The UI labels the channel
**Correspondent** and the panel **War Correspondent**. The mob ID is
`diplomacy-narrator`; the member and channel IDs remain `narrator`.
Displaying completed narrative currently requires the consumer migration noted
above.

### 10 DM Channels

| Channel | Agents | Content |
|---------|--------|---------|
| France / Prussia / Russia (Planner ↔ Operator) | Planner ↔ Operator | Private strategy debate |
| France / Prussia / Russia (Planner ↔ Ambassador) | Planner ↔ Ambassador | Diplomatic briefing (what to lie about) |
| Franco-Prussian / Franco-Russian / Prussian-Russian | Ambassador ↔ Ambassador | Cross-faction negotiation |
| Correspondent (`narrator` internally) | Narrator | Intended dramatic narrative; currently blocked by the polling mismatch above |

### Turn Flow

```
JS injects game state → Planners
  Planners ↔ Operators (strategy debate)
  Planners → Ambassadors (diplomatic brief with misdirection orders)
  Ambassadors ↔ Foreign Ambassadors (negotiate, lie, extract intel)
  Ambassadors → Planners (report back)
  Planners → Operators (finalize)
  Operators → FINAL ORDER
JS detects quiescence → extracts orders → resolves combat → narrator flow
```

### Wiring

- **Intra-mob**: explicit `mob_wire` — planner↔operator, planner↔ambassador
- **Cross-mob**: `mob_member_peer_target` plus `mob_wire_peer` gives each
  ambassador pair bidirectional trust
- Ambassadors have `external_addressable: true` for cross-namespace discovery

## Prerequisites

```bash
./scripts/repo-cargo build -p rkat --bin rkat  # builds rkat for repo-local runs
node --version                                 # 20+
npm --version
```

The script uses the prebuilt runtime at
`sdks/web/wasm/meerkat_web_runtime_bg.wasm`. If it is missing or Rust code has
changed, install `wasm-pack` and rebuild it first:

```bash
npm --prefix sdks/web install
npm --prefix sdks/web run build:wasm
```

You can instead set `MEERKAT_WASM=/path/to/meerkat_web_runtime_bg.wasm`.

`./examples.sh` will automatically prefer repo-local binaries built by
`./scripts/repo-cargo` when present.

## Build & Run

```bash
cd examples/031-wasm-mini-diplomacy-sh
chmod +x examples.sh && ./examples.sh
```

This will:
1. Create a minimal mobpack (definitions are constructed in TypeScript at runtime)
2. Assemble the browser runtime with `rkat mob web build --wasm ...`
3. Build the Vite web app
4. Copy WASM files to `web/dist/`

Then serve and open:
```bash
python3 -m http.server 4173 --directory web/dist
open http://127.0.0.1:4173
```

### Rebuild after Rust changes

```bash
./scripts/repo-cargo build -p rkat --bin rkat
npm --prefix sdks/web run build:wasm
cd examples/031-wasm-mini-diplomacy-sh
./examples.sh
```

The SDK build step compiles the WASM runtime. `rkat mob web build` then assembles
the prebuilt runtime and mobpack into the browser bundle.

## Usage

1. Open settings (gear icon), enter your Anthropic API key
2. Select a model (default: `claude-sonnet-4-6`)
3. Click **Start Campaign**
4. Watch agents deliberate in DM channels — click channels in the sidebar to follow conversations
5. Territories change color on the map as combat resolves
6. The **Correspondent** channel is intended to show narrative after each turn;
   current-runtime display requires the narrator polling migration noted above

### Controls

- **Pause/Resume** — stop/resume progress in the JavaScript host loop. The stop
  flag is checked during a round, not only between rounds, and does not cancel
  in-flight autonomous agent work.
- **Step** — run a host-loop iteration, then clear its running flag. Exact
  single-round stepping is a current limitation: `tick()` can initiate another
  iteration before the flag is cleared, so the number of completed rounds is
  not guaranteed.
- **Export** — download game state + all messages as JSON

## Key Files

| File | Purpose |
|------|---------|
| `web/src/main.ts` | Game engine, mob definitions, event streaming, UI |
| `web/src/styles.css` | Slack-like DM interface, territory map styling |
| `examples.sh` | Build script (mobpack + prebuilt WASM + Vite) |
| `../../meerkat-web-runtime/src/lib.rs` | WASM exports powering the runtime |
| `../../meerkat-mob/src/runtime/flow.rs` | Flow engine (narrator uses this) |
| `../../meerkat-comms/src/router.rs` | Cross-namespace inproc routing |

## Notes

- Page reload destroys all state (WASM runtime is fully in-memory)
- Each turn takes 1-3 minutes depending on model and conversation depth
- The game detects API failures and stops with a clear error message
- Debug: open browser console to see Rust tracing output via `tracing-wasm`
