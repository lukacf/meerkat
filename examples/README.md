# Meerkat Examples Library

33 examples covering every Meerkat surface and feature,
from "Hello World" to production multi-agent systems.

## Quick Start

```bash
# Set your API key
export ANTHROPIC_API_KEY=sk-...

# Install shared TypeScript example dependencies once
cd examples && npm install

# Run a Python example
(cd 002-hello-meerkat-py && python main.py)

# Run a TypeScript example
(cd 003-hello-meerkat-ts && npx tsx main.ts)

# Run a shell example
(cd 010-mcp-tool-server-sh && ./setup.sh)
```

Rust examples in this folder are wired into `meerkat/Cargo.toml` and can be run
directly from the workspace root with `cargo run -p meerkat --example <name>`.
For example: `cargo run -p meerkat --example 001-hello-meerkat --features jsonl-store`.

## Flagship Shell Examples

These are the strongest shell-driven examples if you want realistic,
pedagogical workflows rather than lightweight command recipes:

| # | Example | Why Start Here |
|---|---------|----------------|
| 010 | [mcp-tool-server-sh](010-mcp-tool-server-sh/) | End-to-end MCP integration: register a real local stdio server, inspect config, and run a live MCP-backed prompt |
| 028 | [mobpack-release-triage-sh](028-mobpack-release-triage-sh/) | Portable release-incident mobpack: build, sign, inspect, validate, and deploy a believable multi-role triage artifact |
| 029 | [web-incident-war-room-sh](029-web-incident-war-room-sh/) | Browser-deployable incident room: pack a real SEV workflow into a zero-install web bundle with kickoff prompts |
| 030 | [web-dashboard-copilot-sh](030-web-dashboard-copilot-sh/) | Embeddable ops copilot: produce a web bundle plus dashboard context, prompts, and iframe starter assets |

## Verification Status

This repo now mixes fully live-ran examples, build-verified examples, and
recipe-style examples that are still useful but depend on heavier external
toolchains. The table below reflects the current verification state for this
branch.

| Status | Examples |
|--------|----------|
| **Live-ran** | 001, 002, 003, 007, 008, 010, 021, 022, 023, 028, 029, 030 |
| **Build-verified** | Registered Rust examples via `cargo check`; 031, 032, 033 via Vite builds; 034 via `cargo check` |
| **Syntax-checked / recipe-oriented** | 004 and shell entrypoints that are safe but intentionally operational rather than fully automated |

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
| 011 | [hooks-guardrails-rs](011-hooks-guardrails-rs/) | Rust | Intercept agent behavior at 7 hook points for audit, filtering, gating |
| 012 | [skills-loading-rs](012-skills-loading-rs/) | Rust | Inject domain-specific knowledge from files, git, HTTP |
| 013 | [context-compaction-rs](013-context-compaction-rs/) | Rust | Automatic context summarization for infinite conversations |
| 014 | [semantic-memory-rs](014-semantic-memory-rs/) | Rust | Persistent, searchable memory across sessions |
| 015 | [session-persistence-rs](015-session-persistence-rs/) | Rust | Session persistence patterns and store implementations, including JSONL, in-memory, and redb-backed stores |

### Multi-Agent — Comms & Mobs

| # | Example | Surface | Description |
|---|---------|---------|-------------|
| 017 | [mob-coding-swarm-rs](017-mob-coding-swarm-rs/) | Rust | Orchestrator + worker mob for coding tasks |
| 018 | [mob-research-team-rs](018-mob-research-team-rs/) | Rust | Diverge/converge research with specialized profiles |
| 019 | [mob-pipeline-rs](019-mob-pipeline-rs/) | Rust | Sequential CI/CD pipeline with stage handoffs |
| 020 | [comms-peer-messaging-rs](020-comms-peer-messaging-rs/) | Rust | Ed25519-signed peer-to-peer agent communication |

### Expert — Production Patterns & Multi-Surface

| # | Example | Surface | Description |
|---|---------|---------|-------------|
| 021 | [multi-provider-routing-py](021-multi-provider-routing-py/) | Python | Route to Anthropic, OpenAI, Gemini with provider-specific params |
| 022 | [rest-api-client-py](022-rest-api-client-py/) | Python | HTTP REST API integration (no SDK required) |
| 023 | [rpc-ide-integration-ts](023-rpc-ide-integration-ts/) | TypeScript | JSON-RPC for IDE extensions and desktop apps |
| 024 | [host-mode-event-mesh-rs](024-host-mode-event-mesh-rs/) | Rust | Multi-turn keep-alive event mesh (reactive agents processing incoming events) |
| 025 | [full-stack-agent-rs](025-full-stack-agent-rs/) | Rust | Reference architecture with all features combined |
| 026 | [skills-v21-invoke-py](026-skills-v21-invoke-py/) | Python | Invoke a specific skill with canonical `SkillKey` refs |
| 027 | [skills-v21-invoke-ts](027-skills-v21-invoke-ts/) | TypeScript | Use `session.invokeSkill()` with canonical `SkillKey` refs |
| 028 | [mobpack-release-triage-sh](028-mobpack-release-triage-sh/) | Shell | Build, sign, validate, and deploy a realistic release-triage `.mobpack` |
| 029 | [web-incident-war-room-sh](029-web-incident-war-room-sh/) | Shell | Build a browser-deployable SEV war room with specialized incident roles and kickoff prompts |
| 030 | [web-dashboard-copilot-sh](030-web-dashboard-copilot-sh/) | Shell | Build an embeddable release command-center copilot with dashboard context and starter embed assets |
| 031 | [wasm-mini-diplomacy-sh](031-wasm-mini-diplomacy-sh/) | Shell + Web | 9 autonomous agents across 4 WASM mobs wage a 3-faction territory war with strategy, diplomacy, and deception |
| 032 | [wasm-webcm-agent](032-wasm-webcm-agent/) | Web (WASM) | Multi-provider coding agent mob in the browser — 4 agents (Anthropic + OpenAI + Gemini) collaborate via comms in a sandboxed Linux VM |
| 034 | [codemob-mcp](034-codemob-mcp/) | Rust (MCP) | Multi-agent MCP server — 7 mobpacks (advisor, review, architect, brainstorm, red-team, panel, rct) with flow and comms execution, progress notifications, multi-provider model diversity |

## Examples by Feature

| Feature | Examples |
|---------|----------|
| **Custom Tools** | 006, 025 |
| **Built-in Tools** | 025 |
| **Streaming** | 005, 007 |
| **Sessions** | 004, 007, 015, 022, 023 |
| **Budget & Retry** | 009 |
| **MCP Integration** | 010 |
| **Hooks** | 011 |
| **Skills** | 012, 017, 018, 019, 026, 027 |
| **Compaction** | 013 |
| **Semantic Memory** | 014 |
| **Persistence** | 015 |
| **Mobs** | 017, 018, 019, 028, 029, 030, 031, 032, 034 |
| **Mobpack** | 028, 029, 030, 031, 034 |
| **WASM Web Build** | 029, 030, 031, 032 |
| **Comms** | 020, 024, 032, 034 |
| **Keep-alive / long-lived sessions** | 024, 032, 034 |
| **Multi-Provider** | 021, 032, 034 |
| **MCP Server** | 034 |
| **Flow Engine** | 034 |
| **REST API** | 022 |
| **JSON-RPC** | 023 |
| **Structured Output** | 008 |

## Examples by Surface

| Surface | Examples |
|---------|----------|
| **Rust SDK** | 001, 005, 006, 009, 011-020, 024, 025, 034 |
| **Python SDK** | 002, 007, 021, 022, 026 |
| **TypeScript SDK** | 003, 008, 023, 027 |
| **CLI (Shell)** | 004, 010, 028, 029, 030, 031 |
| **WASM (Browser)** | 029, 030, 031, 032 |

## Prerequisites

### Rust Examples
```bash
# Build from source
cargo build --workspace

# Or install from crates.io
cargo install meerkat
```

### Python Examples
```bash
pip install meerkat-sdk
# Or install from source:
pip install -e sdks/python
```

### TypeScript Examples
```bash
cd examples && npm install
```

### API Keys
```bash
export ANTHROPIC_API_KEY=sk-...     # Required for most examples
export OPENAI_API_KEY=sk-...        # Optional (examples 021, 034)
export GEMINI_API_KEY=...           # Optional (examples 021, 034)
```

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        MEERKAT PLATFORM                          │
│                                                                 │
│  Surfaces:  CLI (rkat)  │  REST API  │  JSON-RPC  │  MCP Server │
│             Python SDK  │  TypeScript SDK  │  Rust SDK           │
│                                                                 │
│  Runtime:   SessionService → AgentFactory::build_agent()        │
│             → Agent Loop → RunResult                            │
│             LLM ↔ Tools ↔ Events ↔ Budget ↔ Retry              │
│                                                                 │
│  Features:  Skills  │  Hooks  │  Sessions  │  Memory            │
│             Comms   │  Mobs  │  MCP  │  WASM Web Build         │
│                                                                 │
│  Providers: Anthropic  │  OpenAI  │  Gemini                     │
│                                                                 │
│  Storage:   SQLite realms (default)  │  JsonlStore  │  MemoryStore  │  redb-backed stores │
└─────────────────────────────────────────────────────────────────┘
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
