# 031 - WASM Mini Diplomacy Arena (Shell + Web)

Flagship browser example: **9 autonomous AI agents** across 3 factions wage a territory war with real-time strategy, diplomacy, and deception — all running in-browser via the Meerkat WASM runtime.

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
flow turns the conversation logs from all 9 agents into dramatic narrative with
omniscient perspective. The UI labels the channel **Correspondent** and the
panel **War Correspondent**. The mob ID is `diplomacy-narrator`; the member and
channel IDs remain `narrator`. The host reads the `mob_flow_status` envelope
(`{ run: MobRun | null }`) and treats `run: null` as still pending.

### Nine chat cells and a separate narrator feed (10 channels)

| Channel | Agents | Content |
|---------|--------|---------|
| France / Prussia / Russia (Planner ↔ Operator) | Planner ↔ Operator | Private strategy debate |
| France / Prussia / Russia (Planner ↔ Ambassador) | Planner ↔ Ambassador | Diplomatic briefing (what to lie about) |
| Franco-Prussian / Franco-Russian / Prussian-Russian | Ambassador ↔ Ambassador | Cross-faction negotiation |
| Correspondent (`narrator` internally) | Narrator | Omniscient dramatic narrative |

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

1. Click **Enter the War Room**, open settings (gear icon), and enter at least one provider API key
2. Select models for France, Prussia, Russia and the narrator (available models depend on your keys)
3. Click **Start**, then dismiss the reading guide with **Got it — Begin Campaign**
4. Watch the 3×3 chat grid — each cell's arrow toggles compact summaries and verbose messages
5. Territories change color on the map as combat resolves
6. The **Correspondent** channel shows dramatic omniscient narrative after each turn

### Controls

- **Pause/Resume** — pause the game loop at the next completed-turn boundary (agents finish the current round)
- **Step** — advance exactly one turn when idle; during a round, finish only that round and pause
- **Start** — replace the campaign after the current round finishes, closing old subscriptions and mobs first
- **Export** — download game state, messages and typed run-failure reports as JSON

## Key Files

| File | Purpose |
|------|---------|
| `web/src/main.ts` | Startup, turn orchestration, controls and HTML shell |
| `web/src/runner.ts` | Owned campaign scheduling, bounded order and narrator polling |
| `web/src/agents.ts` | Faction/narrator definitions and inline skills |
| `web/src/events.ts` | Typed event ingress, peer-ID routing, structured summaries and battle narration |
| `web/src/game.ts` | Combat resolution and per-order outcomes |
| `web/src/ui.ts` / `map.ts` | Chat grid, narrator feed, scores / territory map |
| `web/src/config.ts` / `types.ts` | Provider/model selection / game and runtime types |
| `web/src/styles.css` | War-room grid and territory map styling |
| `examples.sh` | Build script (mobpack + prebuilt WASM + Vite) |
| `../../crates/meerkat-web-runtime/src/lib.rs` | WASM exports powering the runtime |
| `../../crates/meerkat-mob/src/runtime/flow_frame_engine.rs` | Flow engine (narrator uses this) |
| `../../crates/meerkat-comms/src/router.rs` | Cross-namespace inproc routing |

## Notes

- Page reload destroys all state (WASM runtime is fully in-memory)
- Each turn takes 1-3 minutes depending on model and conversation depth
- The game detects API failures and stops with a clear error message
- Debug: open browser console to see Rust tracing output via `tracing-wasm`

## Local regression checks

`npm --prefix web test` runs real example functions in a headless browser, including
an offline initialization of the current repo-local WASM. Provider requests are
intercepted with synthetic errors; this is not a live-provider campaign test.
`npm --prefix web run build` runs strict TypeScript checking before bundling.
The tests require Playwright Chromium and the current `sdks/web/wasm` pair.

The strict full-startup check exercises actual faction creation, wiring and
subscriptions with successful synthetic provider responses. It passes with the
repo-local WASM rebuilt from this checkout, including the cross-target comms-drain
repair. A previously built 0.8.40 artifact can still contain the old wiring
failure; rebuild rather than relying on its version string. Offline startup is
not proof of a live-provider campaign, and the test does not replace real wiring.
