# 031 — WASM Mini Diplomacy Arena (Shell + Web)

Flagship browser example: **9 autonomous AI agents** across 3 factions wage a territory war with real-time strategy, diplomacy, and deception — all running in-browser via the Meerkat WASM runtime.

## What it demonstrates

This is primarily a **smoke test for the Meerkat WASM platform**, exercising:

- **Autonomous host mode on WASM** — agents run continuous host loops, wake on comms messages, make LLM calls, all inside `wasm32-unknown-unknown`
- **Multi-mob orchestration** — 4 mobs (3 factions + narrator) created and managed via `MobMcpState`
- **Cross-mob comms** — ambassadors in different mobs discover each other and negotiate via `InprocRegistry` cross-namespace routing
- **Flow engine on WASM** — narrator mob uses a `turn_driven` flow for structured JSON output
- **Event streaming** — `try_recv()` on raw `broadcast::Receiver` for real-time UI updates
- **Inline skills** — per-agent system prompts delivered via `SkillSource::Inline` in the mob definition

## Architecture

### 3 Faction Mobs (9 autonomous agents)

Each faction (North, South, East) is a mob with 3 `autonomous_host` agents:

| Role | Responsibility | Peers |
|------|---------------|-------|
| **Planner** | Strategic analysis, coordinates team, briefs ambassador | Operator, Ambassador |
| **Operator** | Combat math, challenges plans, produces final orders | Planner |
| **Ambassador** | Diplomatic deception, negotiates with foreign ambassadors | Planner, Foreign Ambassadors |

Agents converse freely via comms tools (`send`, `peers`). No flows — conversations emerge organically.

### 1 Narrator Mob (turn-driven flow)

A separate mob with a single `turn_driven` agent. After each turn resolves, a flow injects the complete conversation logs from all 9 agents and produces dramatic narrative with omniscient perspective.

### 10 DM Channels

| Channel | Agents | Content |
|---------|--------|---------|
| N/S/E: Plan↔Op | Planner ↔ Operator | Private strategy debate |
| N/S/E: Plan↔Amb | Planner ↔ Ambassador | Diplomatic briefing (what to lie about) |
| N↔S, N↔E, S↔E Diplo | Ambassador ↔ Ambassador | Cross-faction negotiation |
| #narrator | Narrator | Omniscient dramatic narrative |

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
- **Cross-mob**: `wire_cross_mob` — all ambassador pairs get bidirectional trust
- Ambassadors have `external_addressable: true` for cross-namespace discovery

## Prerequisites

```bash
cargo build -p meerkat-cli      # builds rkat
cargo install wasm-pack          # if not already installed
node --version                   # 20+
npm --version
```

## Build & Run

```bash
cd examples/031-wasm-mini-diplomacy-sh
chmod +x examples.sh && ./examples.sh
```

This will:
1. Create a minimal mobpack (definitions are constructed in TypeScript at runtime)
2. Build the WASM runtime via `rkat mob web build`
3. Build the Vite web app
4. Copy WASM files to `web/dist/`

Then serve and open:
```bash
python3 -m http.server 4173 --directory web/dist
open http://127.0.0.1:4173
```

### Manual WASM rebuild (after Rust changes)

```bash
RUSTFLAGS='--cfg getrandom_backend="wasm_js"' \
  wasm-pack build meerkat-web-runtime --target web \
  --out-dir ../examples/031-wasm-mini-diplomacy-sh/.work/runtime/pkg --dev

# Copy to dist
cp .work/runtime/pkg/meerkat_web_runtime.js web/dist/runtime.js
cp .work/runtime/pkg/meerkat_web_runtime_bg.wasm web/dist/meerkat_web_runtime_bg.wasm

# Rebuild web bundle
cd web && npm run build
# Re-copy WASM (Vite wipes dist/)
cp ../.work/runtime/pkg/meerkat_web_runtime.js dist/runtime.js
cp ../.work/runtime/pkg/meerkat_web_runtime_bg.wasm dist/meerkat_web_runtime_bg.wasm
```

## Usage

1. Open settings (gear icon), enter your Anthropic API key
2. Select a model (default: `claude-sonnet-4-5`)
3. Click **Start Campaign**
4. Watch agents deliberate in DM channels — click channels in the sidebar to follow conversations
5. Territories change color on the map as combat resolves
6. Narrator channel shows dramatic omniscient narrative after each turn

### Controls

- **Pause/Resume** — pause the game loop between turns
- **Step** — advance exactly one turn
- **Export** — download game state + all messages as JSON

## Key Files

| File | Purpose |
|------|---------|
| `web/src/main.ts` | Game engine, mob definitions, event streaming, UI |
| `web/src/styles.css` | Slack-like DM interface, territory map styling |
| `examples.sh` | Build script (mobpack → WASM → Vite) |
| `../../meerkat-web-runtime/src/lib.rs` | 25 WASM exports powering the runtime |
| `../../meerkat-mob/src/runtime/flow.rs` | Flow engine (narrator uses this) |
| `../../meerkat-comms/src/router.rs` | Cross-namespace inproc routing |

## Notes

- Page reload destroys all state (WASM runtime is fully in-memory)
- Each turn takes 1-3 minutes depending on model and conversation depth
- The game detects API failures and stops with a clear error message
- Debug: open browser console to see Rust tracing output via `tracing-wasm`
