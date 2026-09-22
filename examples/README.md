# Meerkat Examples Library

35 examples covering Meerkat's shipping surfaces and major feature areas,
from "Hello World" to production-shaped multi-agent systems.

## Quick Start

```bash
# Set API keys: 010 checks ANTHROPIC_API_KEY, but its fresh unpinned run
# uses OpenAI's gpt-6-astra and needs access to that model.
export ANTHROPIC_API_KEY=sk-...
export OPENAI_API_KEY=sk-...

# Build the repo-local CLI/RPC binaries used by shell and SDK examples
./scripts/repo-cargo build -p rkat --bin rkat
./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc

# Point SDK examples at the repo-local RPC binary
export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"

# Install/build local SDK dependencies
python3 -m venv .venv
. .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -e sdks/python
npm --prefix sdks/typescript install
npm --prefix sdks/typescript run build

# Install shared TypeScript example dependencies once
(cd examples && npm install)

# Run a Python example
(cd examples/002-hello-meerkat-py && python3 main.py)

# Run a TypeScript example
(cd examples/003-hello-meerkat-ts && npx tsx main.ts)

# Run a shell example
(cd examples/010-mcp-tool-server-sh && ./setup.sh)
```

Setting an API key does not select its provider. Fresh, unpinned CLI runs in
004 and 010 select `gpt-6-astra`. Explicit model/provider configuration can
change the credentials needed, but must apply to the roots used by the script:
010 redirects its roots into `.work/`, and 004 includes an `--isolated` run.
An ordinary-realm-only model pin is not sufficient for every run.

Most numbered Rust examples are registered in `meerkat/Cargo.toml`; 017-019
are registered in `meerkat-mob/Cargo.toml` and use `-p meerkat-mob`. Examples
034 and 035 are standalone packages: select their `Cargo.toml` with
`--manifest-path` and the desired binary with `--bin`. See each example's own
run instructions for target names and required features. For example, run
the facade's 001 target from the workspace root:

```bash
./scripts/repo-cargo run -p meerkat --example 001-hello-meerkat --features jsonl-store
```

## Flagship Shell Examples

These are the strongest shell-driven examples if you want realistic,
pedagogical workflows rather than lightweight command recipes:

| # | Example | Why Start Here |
|---|---------|----------------|
| 010 | [mcp-tool-server-sh](010-mcp-tool-server-sh/) | End-to-end MCP integration: register a real local stdio server, inspect config, and run a live MCP-backed prompt |
| 028 | [mobpack-release-triage-sh](028-mobpack-release-triage-sh/) | Portable release-incident mobpack: build, sign, inspect, validate, and deploy a believable multi-role triage artifact |
| 029 | [web-incident-war-room-sh](029-web-incident-war-room-sh/) | Package a SEV-team definition and assemble a browser runtime bootstrap with kickoff prompts |
| 030 | [web-dashboard-copilot-sh](030-web-dashboard-copilot-sh/) | Produce a browser bootstrap plus dashboard context, prompts, and iframe placement assets |

## Verification Status

This repo mixes live examples, build-verified examples, and recipe-style
examples that depend on external toolchains or provider credentials. The table
below describes the expected local validation level.

| Status | Examples |
|--------|----------|
| **Live when provider keys/services are available** | 001-003, 005-015, 017-019, 021-028, 034-037 |
| **Build-verified locally** | Registered Rust examples via `./scripts/repo-cargo check`; 031 via a prebuilt `sdks/web/wasm` package plus Vite; 032 and 033 via repo-local WASM builds plus Vite |
| **Syntax-checked / recipe-oriented** | 004, 010, 028-030 shell entrypoints and 036 audio setup when live provider/audio devices are unavailable |

## Examples by Level

### Beginner — Getting Started

| # | Example | Surface | Description |
|---|---------|---------|-------------|
| 001 | [hello-meerkat-rs](001-hello-meerkat-rs/) | Rust | Minimal agent: one prompt, one response |
| 002 | [hello-meerkat-py](002-hello-meerkat-py/) | Python | Python SDK basics |
| 003 | [hello-meerkat-ts](003-hello-meerkat-ts/) | TypeScript | TypeScript SDK basics |
| 004 | [cli-one-liners-sh](004-cli-one-liners-sh/) | Shell | CLI commands for sessions, config, realms |
| 005 | [streaming-events-rs](005-streaming-events-rs/) | Rust | Real-time event processing from agent execution |

### Intermediate — Tools, Sessions & Configuration

| # | Example | Surface | Description |
|---|---------|---------|-------------|
| 006 | [custom-tools-rs](006-custom-tools-rs/) | Rust | Build a travel assistant with weather + unit conversion tools |
| 007 | [multi-turn-sessions-py](007-multi-turn-sessions-py/) | Python | Multi-turn conversations with session management |
| 008 | [structured-output-ts](008-structured-output-ts/) | TypeScript | JSON schema-constrained output for data pipelines |
| 009 | [budget-and-retry-rs](009-budget-and-retry-rs/) | Rust | Production guardrails: token budgets, turn limits, retry policies |
| 010 | [mcp-tool-server-sh](010-mcp-tool-server-sh/) | Shell | Register a real local MCP server, inspect project config, and run a live MCP-backed prompt |

### Advanced — Hooks, Skills, Memory & Persistence

| # | Example | Surface | Description |
|---|---------|---------|-------------|
| 011 | [hooks-guardrails-rs](011-hooks-guardrails-rs/) | Rust | Intercept agent behavior at 8 hook points for audit, filtering, gating |
| 012 | [skills-loading-rs](012-skills-loading-rs/) | Rust | Compose inline and filesystem skills, with the broader source architecture explained in code |
| 013 | [context-compaction-rs](013-context-compaction-rs/) | Rust | Automatic context summarization for long-running conversations |
| 014 | [semantic-memory-rs](014-semantic-memory-rs/) | Rust | In-memory semantic recall, plus the production HNSW/SQLite architecture |
| 015 | [session-persistence-rs](015-session-persistence-rs/) | Rust | Direct JSONL save/load roundtrip plus a comparison with memory and SQLite session stores |

### Multi-Agent — Comms & Mobs

| # | Example | Surface | Description |
|---|---------|---------|-------------|
| 017 | [mob-coding-swarm-rs](017-mob-coding-swarm-rs/) | Rust | Define, validate, spawn, and wire a lead + worker coding mob |
| 018 | [mob-research-team-rs](018-mob-research-team-rs/) | Rust | Define, validate, spawn, and wire lead + specialist research profiles |
| 019 | [mob-pipeline-rs](019-mob-pipeline-rs/) | Rust | Mob definition, validation, topology wiring, and manual lint/test dispatch |

### Expert — Production Patterns & Multi-Surface

| # | Example | Surface | Description |
|---|---------|---------|-------------|
| 021 | [multi-provider-routing-py](021-multi-provider-routing-py/) | Python | Route to Anthropic, OpenAI, Gemini with provider-specific params |
| 022 | [rest-api-client-py](022-rest-api-client-py/) | Python | HTTP REST API integration (no SDK required) |
| 023 | [rpc-ide-integration-ts](023-rpc-ide-integration-ts/) | TypeScript | Local JSON-RPC transport for IDE extensions and desktop apps |
| 024 | [host-mode-event-mesh-rs](024-host-mode-event-mesh-rs/) | Rust | Standalone multi-turn event-processing pattern with event streaming |
| 025 | [full-stack-agent-rs](025-full-stack-agent-rs/) | Rust | Focused standalone composition of tools, budget, JSONL storage, prompt behavior, and events |
| 026 | [skills-v21-invoke-py](026-skills-v21-invoke-py/) | Python | Invoke a specific skill with canonical `SkillKey` refs |
| 027 | [skills-v21-invoke-ts](027-skills-v21-invoke-ts/) | TypeScript | Use `session.invokeSkill()` with canonical `SkillKey` refs |
| 028 | [mobpack-release-triage-sh](028-mobpack-release-triage-sh/) | Shell | Build, sign, validate, and deploy a realistic release-triage `.mobpack` |
| 029 | [web-incident-war-room-sh](029-web-incident-war-room-sh/) | Shell | Pack an incident-team definition and assemble its browser WASM bootstrap |
| 030 | [web-dashboard-copilot-sh](030-web-dashboard-copilot-sh/) | Shell | Assemble a release-copilot browser bootstrap with host-integration assets |
| 031 | [wasm-mini-diplomacy-sh](031-wasm-mini-diplomacy-sh/) | Shell + Web | 9 autonomous faction agents plus a turn-driven narrator across 4 WASM mobs |
| 032 | [wasm-webcm-agent](032-wasm-webcm-agent/) | Web (WASM) | Multi-provider coding agent mob in the browser - 4 agents (Anthropic + OpenAI + Gemini) collaborate via comms in a sandboxed Linux VM |
| 033 | [the-office-demo-sh](033-the-office-demo-sh/) | Shell + Web (WASM) | 10 autonomous office agents in the browser coordinate via comms - phone calls, speech bubbles, approval gates, and a live knowledge graph |
| 034 | [codemob-mcp](034-codemob-mcp/) | Rust (MCP) | Multi-agent MCP server with 8 structured-flow packs, progress notifications, and multi-provider model diversity |
| 035 | [mdm-tux-rs](035-mdm-tux-rs/) | Rust + TUI + Docker | Meerkat Device Manager: TUX terminal controller, kennel rendezvous, remote targets, hive coordination, peer comms, and scheduler wakeups |
| 036 | [realtime-audio-py](036-realtime-audio-py/) | Python | Command-line OpenAI realtime audio app with live transcript, callback tools, inline mob skills, and helper sub-agents |
| 037 | [live-webrtc-web](037-live-webrtc-web/) | Web + TypeScript | Browser WebRTC smoke-test cockpit with AEC microphone capture, live controls, note and text-pane callbacks, and real mob-mcp tools |

## Examples by Feature

| Feature | Examples |
|---------|----------|
| **Custom Tools** | 006, 025, 036, 037 |
| **Built-in Tools** | 025 |
| **Streaming** | 005, 007 |
| **Sessions** | 004, 007, 015, 022, 023 |
| **Budget & Retry** | 009 |
| **MCP Integration** | 010 |
| **Hooks** | 011 |
| **Skills** | 012, 017, 018, 019, 026, 027, 036 |
| **Compaction** | 013 |
| **Semantic Memory** | 014 |
| **Persistence** | 015 |
| **Mobs** | 017, 018, 019, 028, 029, 030, 031, 032, 033, 034, 035, 036, 037 |
| **Mobpack** | 028, 029, 030, 031 |
| **Browser WASM** | 029, 030, 031, 032, 033 |
| **Comms** | 017-019, 028, 031-037 |
| **Standalone autonomous mob loops** | 031, 032, 033 |
| **Runtime-backed long-lived sessions** | 035, 036, 037 |
| **Multi-Provider** | 021, 032, 034, 035 |
| **MCP Server** | 034 |
| **Flow Engine** | 019, 031, 034 |
| **Remote Computer Control** | 035 |
| **Scheduling** | 035 |
| **Realtime Audio** | 036, 037 |
| **REST API** | 022 |
| **JSON-RPC** | 023, 035, 037 |
| **Structured Output** | 008, 031, 033 |

## Examples by Surface

| Surface | Examples |
|---------|----------|
| **Rust SDK** | 001, 005, 006, 009, 011-015, 017-019, 024, 025, 034, 035 |
| **Python SDK** | 002, 007, 021, 026, 036 |
| **TypeScript SDK** | 003, 008, 023, 027 |
| **REST client** | 022 |
| **CLI (Shell)** | 004, 010, 028, 029, 030, 031 |
| **WASM (Browser)** | 029, 030, 031, 032, 033 |
| **WebRTC (Browser)** | 037 |

## Prerequisites

### Rust Examples
```bash
# Build from source
make build

# Run one registered Rust example from the workspace root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 001-hello-meerkat --features jsonl-store
```

### Python Examples
```bash
python3 -m venv .venv
. .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -e sdks/python
./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
```

### TypeScript Examples
```bash
npm --prefix sdks/typescript install
npm --prefix sdks/typescript run build
(cd examples && npm install)
./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
```

### API Keys
```bash
export ANTHROPIC_API_KEY=sk-...     # Required for most examples
export OPENAI_API_KEY=sk-...        # 004/010 fresh defaults; also 021, 034, 035 live suite, 036
export GEMINI_API_KEY=...           # Optional (examples 021, 034, 035 live suite)
```

## Architecture Overview

```text
CLI / REST / JSON-RPC / MCP / Python / TypeScript
                       |
                       v
             runtime-backed session path
      SessionService + MeerkatMachine + runtime bindings
                       |
                       v
 realm-scoped stores: sessions, runtime, schedule, WorkGraph,
                  jobs, blobs, and artifacts

Standalone Rust examples / browser WASM
                       |
                       v
             explicit standalone path
       in-process agent loop and ephemeral runtime state

Both paths compose agents through `AgentFactory`, the model catalog, and auth
bindings. Available tools, hooks, skills, memory, comms, mob, and compaction
features vary by surface; the browser WASM runtime intentionally exposes a
smaller in-memory subset. Persistent realms default to SQLite through
`RealmStorageProvider`. JSONL is an explicit inspectable backend; memory stores
are for ephemeral/test usage.
```

## Naming Convention

Examples follow the pattern:
```
XXX-name-of-example-{rs|py|ts|sh}/
├── main.{rs|py|ts} or examples.sh    # The runnable code
├── README.md                          # Explanation and concepts
├── (optional shared deps from ../package.json for TS examples)
└── (optional config files)            # mob.toml, etc.
```

The suffix indicates the primary language/surface:
- `rs` — Rust SDK
- `py` — Python SDK
- `ts` — TypeScript SDK
- `sh` — Shell/CLI
