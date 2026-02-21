# Meerkat Examples Library

25 fully functional examples covering every Meerkat surface and feature,
from "Hello World" to production multi-agent systems.

## Quick Start

```bash
# Set your API key
export ANTHROPIC_API_KEY=sk-...

# Run any Rust example
cargo run --example 001_hello_meerkat

# Run a Python example
cd examples/002-hello-meerkat-py && python main.py

# Run a TypeScript example
cd examples/003-hello-meerkat-ts && npx tsx main.ts

# Run a shell example
cd examples/004-cli-one-liners-sh && bash examples.sh
```

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
| 010 | [mcp-tool-server-sh](010-mcp-tool-server-sh/) | Shell | Connect external tools via Model Context Protocol |

### Advanced — Hooks, Skills, Memory & Persistence

| # | Example | Surface | Description |
|---|---------|---------|-------------|
| 011 | [hooks-guardrails-rs](011-hooks-guardrails-rs/) | Rust | Intercept agent behavior at 7 hook points for audit, filtering, gating |
| 012 | [skills-loading-rs](012-skills-loading-rs/) | Rust | Inject domain-specific knowledge from files, git, HTTP |
| 013 | [context-compaction-rs](013-context-compaction-rs/) | Rust | Automatic context summarization for infinite conversations |
| 014 | [semantic-memory-rs](014-semantic-memory-rs/) | Rust | Persistent, searchable memory across sessions |
| 015 | [session-persistence-rs](015-session-persistence-rs/) | Rust | Storage backends: JsonlStore, MemoryStore, RedbSessionStore |

### Multi-Agent — Sub-Agents, Comms & Mobs

| # | Example | Surface | Description |
|---|---------|---------|-------------|
| 016 | [sub-agent-orchestration-rs](016-sub-agent-orchestration-rs/) | Rust | Delegate subtasks to independent child agents |
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
| 024 | [host-mode-event-mesh-rs](024-host-mode-event-mesh-rs/) | Rust | Reactive agents processing incoming events |
| 025 | [full-stack-agent-rs](025-full-stack-agent-rs/) | Rust | Reference architecture with all features combined |

## Examples by Feature

| Feature | Examples |
|---------|----------|
| **Custom Tools** | 006, 025 |
| **Built-in Tools** | 016, 025 |
| **Streaming** | 005, 007, 023 |
| **Sessions** | 004, 007, 015, 022, 023 |
| **Budget & Retry** | 009 |
| **MCP Integration** | 010 |
| **Hooks** | 011 |
| **Skills** | 012, 017, 018, 019 |
| **Compaction** | 013 |
| **Semantic Memory** | 014 |
| **Persistence** | 015 |
| **Sub-Agents** | 016 |
| **Mobs** | 017, 018, 019 |
| **Comms** | 020, 024 |
| **Host Mode** | 024 |
| **Multi-Provider** | 021 |
| **REST API** | 022 |
| **JSON-RPC** | 023 |
| **Structured Output** | 008 |

## Examples by Surface

| Surface | Examples |
|---------|----------|
| **Rust SDK** | 001, 005, 006, 009, 011-020, 024, 025 |
| **Python SDK** | 002, 007, 021, 022 |
| **TypeScript SDK** | 003, 008, 023 |
| **CLI (Shell)** | 004, 010 |

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
npm install meerkat-sdk
# Or link from source:
cd sdks/typescript && npm link
```

### API Keys
```bash
export ANTHROPIC_API_KEY=sk-...     # Required for most examples
export OPENAI_API_KEY=sk-...        # Optional (example 021)
export GEMINI_API_KEY=...           # Optional (example 021)
```

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        MEERKAT PLATFORM                          │
│                                                                 │
│  Surfaces:  CLI (rkat)  │  REST API  │  JSON-RPC  │  MCP Server │
│             Python SDK  │  TypeScript SDK  │  Rust SDK           │
│                                                                 │
│  Agent:     AgentBuilder → Agent Loop → RunResult               │
│             LLM ↔ Tools ↔ Events ↔ Budget ↔ Retry              │
│                                                                 │
│  Features:  Skills  │  Hooks  │  Sessions  │  Memory            │
│             Comms   │  Sub-Agents  │  Mobs  │  MCP              │
│                                                                 │
│  Providers: Anthropic  │  OpenAI  │  Gemini                     │
│                                                                 │
│  Storage:   JsonlStore  │  MemoryStore  │  RedbSessionStore     │
└─────────────────────────────────────────────────────────────────┘
```

## Naming Convention

Examples follow the pattern:
```
XXX-name-of-example-{rs|py|ts|sh}/
├── main.{rs|py|ts} or examples.sh    # The runnable code
├── README.md                          # Explanation and concepts
└── (optional config files)            # mob.toml, etc.
```

The suffix indicates the primary language/surface:
- `rs` — Rust SDK
- `py` — Python SDK
- `ts` — TypeScript SDK
- `sh` — Shell/CLI
