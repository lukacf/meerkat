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
  <a href="https://docs.rkat.ai">Docs</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.85+-orange?logo=rust" alt="Rust 1.85+">
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
meerkat = "0.3"
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
cargo install rkat

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
| **Hooks** | 8 hook points with observe/rewrite/guardrail semantics |
| **Structured Output** | JSON-schema-validated extraction from any provider |
| **Semantic Memory** | Auto-compact long conversations, recall via `memory_search` |
| **Sub-Agents** | Spawn/fork child agents with budget and tool isolation |
| **Multi-Agent Comms** | Ed25519-authenticated peer-to-peer messaging |
| **Skills** | Composable knowledge packs with capability gating |
| **Built-in Tools** | Task management, shell, datetime, and more |
| **Streaming** | Real-time token output via event channels |

### Modularity

The `meerkat` crate defaults to providers only — add subsystems as you need them:

```toml
# Default: all three providers, nothing else
[dependencies]
meerkat = "0.3"

# Just one provider
meerkat = { version = "0.3", default-features = false, features = ["anthropic"] }

# Add session persistence and compaction
meerkat = { version = "0.3", features = ["jsonl-store", "session-store", "session-compaction"] }

# Full harness: persistence, memory, comms, MCP, sub-agents, skills
meerkat = { version = "0.3", features = [
    "all-providers", "jsonl-store", "session-store", "session-compaction",
    "memory-store-session", "comms", "mcp", "sub-agents", "skills"
] }
```

| Feature | What it adds |
|---------|-------------|
| `anthropic`, `openai`, `gemini` | Individual LLM providers (all three on by default) |
| `all-providers` | Shorthand for all three |
| `jsonl-store` | File-based session persistence |
| `session-store` | Durable session storage (redb) |
| `session-compaction` | Auto-compact long conversations |
| `memory-store` | In-memory session storage (testing) |
| `memory-store-session` | Semantic memory with HNSW indexing |
| `comms` | Ed25519 inter-agent messaging |
| `mcp` | MCP protocol client and tool routing |
| `sub-agents` | Spawn/fork child agents |
| `skills` | Composable knowledge packs |

The prebuilt binaries (`rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`) include everything. For a custom binary build:

```bash
cargo install rkat --no-default-features --features "anthropic,openai,session-store,mcp"
```

Disabled capabilities return typed errors (`SessionError::PersistenceDisabled`, etc.) — no panics, no silent degradation.

## Surfaces

All surfaces share the same `SessionService` lifecycle and `AgentFactory` construction pipeline.

| Surface | Use Case | Docs |
|---------|----------|------|
| **Rust crate** | Embed agents in your Rust application | [SDK guide](https://docs.rkat.ai/rust/overview) |
| **Python SDK** | Script agents from Python | [Python SDK](https://docs.rkat.ai/sdks/python/overview) |
| **TypeScript SDK** | Script agents from Node.js | [TypeScript SDK](https://docs.rkat.ai/sdks/typescript/overview) |
| **CLI (`rkat`)** | Terminal, CI/CD, cron jobs, shell scripts | [CLI guide](https://docs.rkat.ai/cli/commands) |
| **REST API** | HTTP integration for web services | [REST guide](https://docs.rkat.ai/api/rest) |
| **JSON-RPC** | Stateful IDE/desktop integration over stdio | [RPC guide](https://docs.rkat.ai/api/rpc) |
| **MCP Server** | Expose Meerkat as tools to other AI agents | [MCP guide](https://docs.rkat.ai/api/mcp) |

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
| `rkat` | CLI binary |
| `meerkat-rest` | REST API server (binary: `rkat-rest`) |
| `meerkat-rpc` | JSON-RPC stdio server (binary: `rkat-rpc`) |
| `meerkat-mcp-server` | MCP server surface (binary: `rkat-mcp`) |

## Examples

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

See the [full documentation](https://docs.rkat.ai) for more: MCP tools, session lifecycle, structured output, hooks, sub-agents, skills, and multi-agent comms.

## Configuration

```bash
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

See the [configuration guide](https://docs.rkat.ai/concepts/configuration) for the full reference.

## Documentation

Full documentation is available at **[docs.rkat.ai](https://docs.rkat.ai)**.

| Section | Topics |
|---------|--------|
| [Getting Started](https://docs.rkat.ai/introduction) | Introduction, quickstart |
| [Core Concepts](https://docs.rkat.ai/concepts/sessions) | Sessions, tools, providers, configuration |
| [Guides](https://docs.rkat.ai/guides/hooks) | Hooks, skills, memory, sub-agents, comms, structured output |
| [CLI & APIs](https://docs.rkat.ai/cli/commands) | CLI reference, REST, JSON-RPC, MCP |
| [SDKs](https://docs.rkat.ai/rust/overview) | Rust, Python, TypeScript |
| [Reference](https://docs.rkat.ai/reference/architecture) | Architecture, capability matrix, built-in tools |

## Development

```bash
cargo build --workspace             # Build
cargo rct                           # Fast tests (unit + integration-fast)
cargo unit                          # Unit tests only
cargo int                           # Integration-fast only
cargo int-real                      # Integration-real (ignored by default)
cargo e2e                           # E2E tests (ignored; requires API keys)
```

## Contributing

1. Run `cargo rct` to verify all checks pass
2. Add tests for new functionality
3. Submit PRs to `main`

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
