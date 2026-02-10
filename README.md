<p align="center">
  <img src=".github/meerkat-logo.png" alt="Meerkat" width="280">
</p>

<h1 align="center">Meerkat</h1>

<p align="center">
<strong>A modular, high-performance agent harness built in Rust.</strong>
</p>

<p align="center">
  <a href="#quick-start">Quick Start</a> &bull;
  <a href="#core-capabilities">Capabilities</a> &bull;
  <a href="#surfaces">Surfaces</a> &bull;
  <a href="#architecture">Architecture</a> &bull;
  <a href="#documentation">Docs</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.89+-orange?logo=rust" alt="Rust 1.89+">
  <img src="https://img.shields.io/badge/License-MIT%2FApache--2.0-blue" alt="License">
  <img src="https://img.shields.io/badge/MCP-Native-green" alt="MCP Native">
  <img src="https://img.shields.io/badge/Multi--Agent-Ed25519-purple" alt="Multi-Agent">
</p>

---

## Why Meerkat?

Meerkat is a **library-first, modular agent harness** — a set of composable Rust crates that handle the hard parts of building agentic systems: state machines, retries, budgets, streaming, tool execution, MCP, multi-agent coordination.

It is designed to be **stable** (deterministic state machine, typed errors, compile-time guarantees) and **fast** (<10ms cold start, ~20MB memory, single 5MB binary).

The library comes first; surfaces come second. The CLI, REST API, JSON-RPC server, MCP server, Python SDK, and TypeScript SDK are all thin layers over the same engine. Pick the entry point that fits your architecture.

### How it compares

| | Meerkat | Claude Code / Codex CLI / Gemini CLI |
|---|---|---|
| **Design** | Library-first, modular crates | CLI-first, SDK bolted on |
| **Modularity** | Compose only what you need — from bare agent loop to full-featured harness | Monolithic, all-or-nothing |
| **Languages** | Rust core + Python & TypeScript SDKs | TypeScript or Python |
| **Interface** | CLI, REST, JSON-RPC, MCP server, language SDKs | Rich interactive TUI |
| **Memory system** | Semantic memory with HNSW indexing + auto-compaction | File-based context |
| **Multi-agent** | Native Ed25519-authenticated P2P messaging | No |
| **Deployment** | Single 5MB binary, <10ms startup, ~20MB RAM | Runtime + dependencies |

Those tools excel at interactive development with rich terminal UIs. Meerkat has no TUI — the CLI is a thin, scriptable surface — but you still get first-class features like hooks, skills, semantic memory, and rich session management. Meerkat is for automated pipelines, embedded agents, multi-agent systems, and anywhere you need programmatic control.

## Quick Start

### As a Library

```toml
[dependencies]
meerkat = "0.1"
tokio = { version = "1", features = ["full"] }
```

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let result = meerkat::with_anthropic(std::env::var("ANTHROPIC_API_KEY")?)
        .model("claude-sonnet-4-5")
        .system_prompt("You are a helpful assistant.")
        .run("What is the capital of France?")
        .await?;

    println!("{}", result.text);
    Ok(())
}
```

### As a CLI

```bash
cargo install --path meerkat-cli

export ANTHROPIC_API_KEY=sk-...
rkat run "What is the capital of France?"
rkat run --model gpt-5.2 "Write a haiku about Rust"
rkat run --model gemini-3-flash-preview "Explain async/await"
```

## Core Capabilities

| Capability | Description |
|------------|-------------|
| **Multi-provider** | Anthropic, OpenAI, Gemini with a unified streaming interface |
| **MCP Native** | Connect to any Model Context Protocol server |
| **Budget Controls** | Strict token limits, time limits, tool call caps |
| **Session Persistence** | Resume conversations from disk (JSONL or redb) |
| **Hooks** | 8 hook points with observe/rewrite/guardrail semantics ([guide](docs/hooks.md)) |
| **Structured Output** | JSON-schema-validated extraction from any provider ([guide](docs/structured-output.md)) |
| **Semantic Memory** | Auto-compact long conversations, recall via `memory_search` ([guide](docs/memory.md)) |
| **Sub-Agents** | Spawn/fork child agents with budget and tool isolation ([guide](docs/sub-agents.md)) |
| **Multi-Agent Comms** | Ed25519-authenticated peer-to-peer messaging ([guide](docs/comms.md)) |
| **Skills** | Composable knowledge packs with capability gating ([guide](docs/skills.md)) |
| **Built-in Tools** | Task management, shell, datetime, and more ([reference](docs/builtin-tools.md)) |
| **Streaming** | Real-time token output via event channels |

### Modularity

Pick only what you need:

```toml
# Minimal: just the agent loop + one provider
meerkat = { version = "0.1", default-features = false, features = ["anthropic"] }

# Add persistence and compaction
meerkat = { version = "0.1", features = ["anthropic", "session-store", "session-compaction"] }

# Everything
meerkat = { version = "0.1", features = ["all-providers", "session-store", "session-compaction", "memory-store-session"] }
```

Disabled capabilities return typed errors (`SessionError::PersistenceDisabled`, etc.) — no panics, no silent degradation.

## Surfaces

All surfaces share the same `SessionService` lifecycle and `AgentFactory` construction pipeline.

| Surface | Use Case | Docs |
|---------|----------|------|
| **Rust crate** | Embed agents in your Rust application | [SDK guide](docs/SDK.md) |
| **Python SDK** | Script agents from Python (`pip install meerkat-sdk`) | [README](sdks/python/README.md) |
| **TypeScript SDK** | Script agents from Node.js (`npm i @meerkat/sdk`) | [README](sdks/typescript/README.md) |
| **CLI (`rkat`)** | Terminal, CI/CD, cron jobs, shell scripts | [CLI guide](docs/CLI.md) |
| **REST API** | HTTP integration for web services | [REST guide](docs/rest.md) |
| **JSON-RPC** | Stateful IDE/desktop integration over stdio | [RPC guide](docs/rpc.md) |
| **MCP Server** | Expose Meerkat as tools to other AI agents | [MCP guide](docs/mcp.md) |

## Architecture

```
                        ┌──────────────────┐
                        │   Your Surface   │  CLI / REST / RPC / MCP / SDK
                        └────────┬─────────┘
                                 │
                        ┌────────▼─────────┐
                        │     meerkat      │  Facade crate
                        └────────┬─────────┘
                                 │
     ┌───────────┬───────┬───────┼───────┬───────────┬───────────┐
     │           │       │       │       │           │           │
┌────▼────┐ ┌───▼───┐ ┌─▼───┐ ┌─▼─────┐ ┌───▼───┐ ┌─────▼─────┐
│ client  │ │session│ │store│ │ mcp   │ │ tools │ │  memory   │
├─────────┤ ├───────┤ ├─────┤ ├───────┤ ├───────┤ ├───────────┤
│Anthropic│ │Service│ │JSONL│ │Router │ │ Reg.  │ │ HNSW      │
│ OpenAI  │ │Compact│ │ redb│ │Stdio  │ │ Valid.│ │ redb      │
│ Gemini  │ │Events │ │     │ │HTTP   │ │       │ │           │
└─────────┘ └───────┘ └─────┘ └───────┘ └───────┘ └───────────┘
                                 │
                         ┌───────▼───────┐
                         │  meerkat-core │  Traits, state machine
                         │  No I/O deps  │  Pure logic
                         └───────────────┘
```

### State Machine

The agent loop follows a strict state machine for predictable behavior:

```
CallingLlm ─────► WaitingForOps ─────► DrainingEvents
    │                  │                     │
    │                  ▼                     ▼
    ├────────────► Completed ◄───────────────┤
    │                  ▲                     │
    ▼                  │                     │
ErrorRecovery ────────►│                     │
    │                                        │
    ▼                                        │
Cancelling ◄─────────────────────────────────┘
```

### Crates

| Crate | Purpose |
|-------|---------|
| `meerkat` | Facade: re-exports, `AgentFactory`, SDK helpers |
| `meerkat-core` | Agent loop, state machine, trait contracts |
| `meerkat-client` | LLM providers (Anthropic, OpenAI, Gemini) |
| `meerkat-session` | Session orchestration, compaction, event store |
| `meerkat-store` | Session persistence (JSONL, redb, in-memory) |
| `meerkat-memory` | Semantic memory (HNSW + redb) |
| `meerkat-tools` | Tool registry, built-in tools |
| `meerkat-hooks` | Hook engine and runtime adapters |
| `meerkat-skills` | Skill resolution and rendering |
| `meerkat-mcp` | MCP protocol client and tool router |
| `meerkat-comms` | Ed25519 encrypted P2P messaging |
| `meerkat-contracts` | Wire types, error codes, capability registry |
| `meerkat-cli` | CLI binary (`rkat`) |
| `meerkat-rest` | REST API server |
| `meerkat-rpc` | JSON-RPC stdio server |
| `meerkat-mcp-server` | MCP server (expose Meerkat as tools) |

## Examples

### Custom Tools

```rust
use meerkat::{AgentToolDispatcher, ToolCallView, ToolDef, ToolResult};
use meerkat::error::ToolError;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

struct Calculator;

#[async_trait]
impl AgentToolDispatcher for Calculator {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef {
            name: "add".into(),
            description: "Add two numbers".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "a": {"type": "number"},
                    "b": {"type": "number"}
                },
                "required": ["a", "b"]
            }),
        })].into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        #[derive(serde::Deserialize)]
        struct AddArgs { a: f64, b: f64 }

        match call.name {
            "add" => {
                let args: AddArgs = call.parse_args()
                    .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
                Ok(ToolResult::success(call.id, (args.a + args.b).to_string()))
            }
            _ => Err(ToolError::not_found(call.name)),
        }
    }
}
```

### Budget Limits

```rust
let result = meerkat::with_anthropic(api_key)
    .budget(BudgetLimits {
        max_tokens: Some(10_000),
        max_duration: Some(Duration::from_secs(60)),
        max_tool_calls: Some(20),
    })
    .run("Solve this complex problem...")
    .await?;
```

### Streaming

```rust
let (tx, mut rx) = mpsc::channel(100);

tokio::spawn(async move {
    while let Some(event) = rx.recv().await {
        if let AgentEvent::TextDelta { delta } = event {
            print!("{}", delta);
        }
    }
});

agent.run_with_events("Write a poem".into(), tx).await?;
```

See [docs/examples.md](docs/examples.md) for more: MCP tools, session lifecycle, structured output, hooks, sub-agents, and multi-agent comms.

## Configuration

```bash
# Environment variables
export ANTHROPIC_API_KEY=sk-...
export OPENAI_API_KEY=sk-...
export GOOGLE_API_KEY=...
```

```toml
# .rkat/config.toml or ~/.rkat/config.toml
[agent]
model = "claude-opus-4-6"
max_tokens = 4096
```

See [docs/configuration.md](docs/configuration.md) for the full reference (providers, budgets, hooks, memory, sub-agents, comms, shell security).

## Documentation

| Guide | Description |
|-------|-------------|
| [Configuration](docs/configuration.md) | All config options, environment variables, feature flags |
| [CLI Reference](docs/CLI.md) | Full `rkat` command reference |
| [Rust SDK](docs/SDK.md) | Library API guide |
| [API Reference](docs/api-reference.md) | Complete type reference |
| [Architecture](docs/architecture.md) | Design deep-dive |
| [Hooks](docs/hooks.md) | Hook system guide |
| [Skills](docs/skills.md) | Skill system guide |
| [Structured Output](docs/structured-output.md) | Schema-validated extraction |
| [Memory & Compaction](docs/memory.md) | Semantic memory guide |
| [Sub-Agents](docs/sub-agents.md) | Agent spawning and orchestration |
| [Comms](docs/comms.md) | Inter-agent communication |
| [Built-in Tools](docs/builtin-tools.md) | Tool reference |
| [REST API](docs/rest.md) | HTTP API reference |
| [JSON-RPC](docs/rpc.md) | Stdio RPC reference |
| [MCP Server](docs/mcp.md) | MCP integration guide |
| [Capability Matrix](docs/CAPABILITY_MATRIX.md) | Build profiles, error codes, feature behavior |
| [Examples](docs/examples.md) | Worked examples |

## Development

```bash
cargo build --workspace             # Build
cargo rct                           # Fast tests (unit + integration-fast)
cargo unit                          # Unit tests only
cargo int                           # Integration-fast only
cargo int-real                      # Integration-real (ignored by default)
cargo e2e                           # E2E tests (ignored; requires API keys)
make ci                             # Full CI pipeline (fmt + lint + test + audit)
```

Rust version pinned to `1.89.0` via `rust-toolchain.toml`.

## Contributing

1. Run `make ci` to verify all checks pass
2. Add tests for new functionality
3. Submit PRs to `main`

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
