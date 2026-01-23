<p align="center">
  <pre>
       /\_/\
      ( o.o )   <b>Meerkat</b>
       > ^ <    Rust Agentic Interface Kit
  </pre>
</p>

<h1 align="center">Meerkat</h1>

<p align="center">
<strong>A production-grade agent harness built in Rust for reliability, speed, and multi-agent coordination.</strong>
</p>

<p align="center">
  <a href="#quick-start">Quick Start</a> &bull;
  <a href="#features">Features</a> &bull;
  <a href="#multi-agent-communication">Multi-Agent</a> &bull;
  <a href="#architecture">Architecture</a> &bull;
  <a href="#cli">CLI</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.89+-orange?logo=rust" alt="Rust 1.89+">
  <img src="https://img.shields.io/badge/License-MIT%2FApache--2.0-blue" alt="License">
  <img src="https://img.shields.io/badge/MCP-Native-green" alt="MCP Native">
  <img src="https://img.shields.io/badge/Multi--Agent-Ed25519-purple" alt="Multi-Agent">
</p>

---

## Why Meerkat?

**For production agentic workloads where reliability matters more than interactive features.**

If you're building CI/CD pipelines, batch processing, autonomous services, or multi-agent systems—and you need predictable behavior, low latency, and minimal resource usage—Meerkat is your tool.

| | Meerkat | Claude Code / Codex CLI / Gemini CLI |
|---|---|---|
| **Primary use** | Automated agentic pipelines | Interactive development |
| **Language** | Rust | TypeScript / Python |
| **Deployment** | Single 5MB binary | Runtime + dependencies |
| **Startup time** | <10ms | 1-3s |
| **Memory footprint** | ~20MB | 200MB+ |
| **Multi-agent native** | ✓ Ed25519 encrypted P2P | ✗ |
| **Deterministic state machine** | ✓ | Varies |
| **Budget enforcement** | ✓ Strict limits | Best-effort |

Meerkat handles the hard parts—state machines, retries, budgets, streaming, MCP, multi-agent coordination—with Rust's reliability guarantees.

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
        .model("claude-sonnet-4")
        .system_prompt("You are a helpful assistant.")
        .run("What is the capital of France?")
        .await?;

    println!("{}", result.text);
    // => "The capital of France is Paris."
    Ok(())
}
```

### As a CLI

```bash
# Install
cargo install --path meerkat-cli

# Run
export ANTHROPIC_API_KEY=sk-...
rkat run "What is the capital of France?"

# With MCP tools
rkat mcp add filesystem -- npx @anthropic/mcp-server-filesystem /tmp
rkat run "List files in /tmp"
```

## Features

| Feature | Description |
|---------|-------------|
| 🦀 **Pure Rust** | Single binary, no runtime dependencies, ~5MB |
| ⚡ **Fast** | <10ms cold start, minimal memory, predictable latency |
| 🔌 **Multi-provider** | Anthropic, OpenAI, Gemini with unified interface |
| 🔧 **MCP Native** | Connect to any Model Context Protocol server |
| 💰 **Budget Controls** | Strict token limits, time limits, tool call caps |
| 💾 **Session Persistence** | Resume conversations from disk |
| 📡 **Streaming** | Real-time token output with event channels |
| 🌐 **Multi-Agent** | Ed25519 encrypted peer-to-peer agent coordination |
| 🎯 **Zero Opinions** | You control prompts, tools, and formatting |

## Multi-Agent Communication

Meerkat includes **first-class support for secure agent-to-agent communication**—a feature not found in interactive CLI tools.

```
    Agent A                           Agent B
   ┌───────┐                         ┌───────┐
   │ rkat  │◄── Ed25519 encrypted ──►│ rkat  │
   │       │    TCP + noise protocol │       │
   └───┬───┘                         └───┬───┘
       │                                 │
       ▼                                 ▼
   Your LLM                          Your LLM
```

**Use cases:**
- 🏭 **Swarm orchestration** - Coordinator dispatches tasks to worker agents
- 🔍 **Specialist collaboration** - Research agent queries domain expert agents
- 🔄 **Pipeline handoffs** - Agent A completes phase 1, hands context to Agent B
- 🎯 **Consensus protocols** - Multiple agents vote on decisions

```rust
use meerkat_comms::{CommsConfig, Keypair, TrustedPeers};
use meerkat_comms_agent::{CommsAgent, CommsManager};

// Each agent has an Ed25519 identity
let keypair = Keypair::generate();
let manager = CommsManager::new(config);

// Wrap your agent with comms capabilities
let agent = CommsAgent::new(inner_agent, manager);

// Agent can now send/receive messages from trusted peers
agent.run("Coordinate with agent-b on this task").await?;
```

**CLI support:**
```bash
# Configure agent identity and peers
rkat run --comms-name "agent-a" "Send results to agent-b"
```

See [docs/ARCHITECTURE.md](docs/architecture.md) for the full comms protocol design.

## Architecture

```
                        ┌──────────────────┐
                        │   Your Agent     │
                        └────────┬─────────┘
                                 │
                        ┌────────▼─────────┐
                        │     meerkat      │  Facade crate
                        └────────┬─────────┘
                                 │
     ┌───────────┬───────────────┼───────────────┬───────────┐
     │           │               │               │           │
┌────▼────┐ ┌────▼────┐ ┌────────▼────────┐ ┌───▼───┐ ┌─────▼─────┐
│  core   │ │ client  │ │   mcp-client    │ │ store │ │   tools   │
├─────────┤ ├─────────┤ ├─────────────────┤ ├───────┤ ├───────────┤
│ Agent   │ │ LLM     │ │ MCP router      │ │ JSONL │ │ Registry  │
│ loop    │ │ APIs    │ │ Tool dispatch   │ │ Memory│ │ Validate  │
│ State   │ │         │ │                 │ │       │ │           │
│ Budget  │ │Anthropic│ │ Stdio/HTTP/SSE  │ │       │ │           │
│ Retry   │ │ OpenAI  │ │                 │ │       │ │           │
│         │ │ Gemini  │ │                 │ │       │ │           │
└─────────┘ └─────────┘ └─────────────────┘ └───────┘ └───────────┘
```

### Crates

| Crate | Description |
|-------|-------------|
| `meerkat` | Facade crate with SDK helpers and re-exports |
| `meerkat-core` | Agent loop, state machine, types (no I/O dependencies) |
| `meerkat-client` | LLM providers: Anthropic, OpenAI, Gemini |
| `meerkat-mcp-client` | MCP protocol client and tool router |
| `meerkat-store` | Session persistence (JSONL, in-memory) |
| `meerkat-tools` | Tool registry and validation |
| `meerkat-cli` | CLI binary (`rkat`) |
| `meerkat-rest` | Optional REST API server |
| `meerkat-mcp-server` | Expose Meerkat as MCP tools |
| **Multi-Agent** | |
| `meerkat-comms` | Ed25519 encrypted P2P messaging protocol |
| `meerkat-comms-agent` | Agent wrapper with inbox/outbox and routing |
| `meerkat-comms-mcp` | Expose comms as MCP tools |

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

## Examples

### Custom Tools

```rust
use meerkat::{AgentToolDispatcher, ToolDef};
use async_trait::async_trait;
use serde_json::{json, Value};

struct Calculator;

#[async_trait]
impl AgentToolDispatcher for Calculator {
    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
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
        }]
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "add" => {
                let a = args["a"].as_f64().unwrap();
                let b = args["b"].as_f64().unwrap();
                Ok((a + b).to_string())
            }
            _ => Err("Unknown tool".into()),
        }
    }
}
```

### Budget Limits

```rust
use meerkat::BudgetLimits;
use std::time::Duration;

let result = meerkat::with_anthropic(api_key)
    .budget(BudgetLimits {
        max_tokens: Some(10_000),
        max_duration: Some(Duration::from_secs(60)),
        max_tool_calls: Some(20),
    })
    .run("Solve this complex problem...")
    .await?;
```

### MCP Tools

```rust
use meerkat::{McpRouter, McpServerConfig};

let mut router = McpRouter::new();
router.add_server(McpServerConfig {
    name: "filesystem".into(),
    transport: StdioTransport {
        command: "npx".into(),
        args: vec!["-y".into(), "@anthropic/mcp-server-filesystem".into(), "/tmp".into()],
        env: Default::default(),
    }.into(),
}).await?;

// Router implements AgentToolDispatcher
let agent = AgentBuilder::new()
    .build(llm, Arc::new(router), store);
```

### Session Resume

```rust
// Run initial prompt
let result = agent.run("Start a task").await?;
let session_id = result.session_id;

// Later: resume the session
let session = store.load(&session_id).await?.unwrap();
let mut agent = AgentBuilder::new()
    .resume_session(session)
    .build(llm, tools, store);

let result = agent.run("Continue the task").await?;
```

### Streaming

```rust
use meerkat::AgentEvent;
use tokio::sync::mpsc;

let (tx, mut rx) = mpsc::channel(100);

// Spawn event handler
tokio::spawn(async move {
    while let Some(event) = rx.recv().await {
        if let AgentEvent::TextDelta { delta } = event {
            print!("{}", delta);
        }
    }
});

// Run with events
agent.run_with_events("Write a poem".into(), tx).await?;
```

## CLI

```
rkat run <prompt>           Run an agent with a prompt
  --model <model>           Model (default: claude-sonnet-4-20250514)
  --provider <p>            Provider: anthropic, openai, gemini
  --max-tokens <n>          Max tokens per turn (default: 4096)
  --max-total-tokens <n>    Total token budget
  --max-duration <dur>      Time limit (e.g., "5m", "1h30m")
  --stream                  Stream tokens to stdout
  --output <format>         Output: text, json

rkat resume <id> <prompt>   Resume a previous session

rkat sessions list          List saved sessions
rkat sessions show <id>     Show session details
rkat sessions delete <id>   Delete a session

rkat mcp add <name> ...     Add an MCP server
rkat mcp list               List MCP servers
rkat mcp remove <name>      Remove an MCP server
```

### MCP Server Management

```bash
# Add stdio server
rkat mcp add filesystem -- npx @anthropic/mcp-server-filesystem /tmp

# Add HTTP server
rkat mcp add api --url http://localhost:8080/mcp

# List servers
rkat mcp list

# Remove server
rkat mcp remove filesystem
```

## Configuration

### MCP Servers

```toml
# .rkat/mcp.toml (project) or ~/.config/rkat/mcp.toml (user)

[servers.filesystem]
transport = "stdio"
command = "npx"
args = ["-y", "@anthropic/mcp-server-filesystem", "/home/user"]

[servers.api]
transport = "http"
url = "http://localhost:8080/mcp"
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `OPENAI_API_KEY` | OpenAI API key |
| `GOOGLE_API_KEY` | Google AI (Gemini) API key |
| `RKAT_MODEL` | Default model |
| `RKAT_MAX_TOKENS` | Default max tokens per turn |

### Feature Flags

```toml
[dependencies]
meerkat = { version = "0.1", features = ["anthropic", "openai", "gemini"] }
```

| Feature | Description |
|---------|-------------|
| `anthropic` | Anthropic Claude support (default) |
| `openai` | OpenAI GPT support |
| `gemini` | Google Gemini support |
| `all-providers` | All LLM providers |
| `jsonl-store` | JSONL file storage (default) |
| `memory-store` | In-memory storage |

## When to Use Meerkat

### Meerkat vs Interactive CLI Tools (Claude Code, Codex CLI, Gemini CLI)

Those tools are **excellent for interactive development**—coding alongside an AI assistant with rich terminal UIs, file watching, and conversational workflows.

**Meerkat is for when you need:**
- ✅ **Unattended execution** - CI/CD, cron jobs, background services
- ✅ **Predictable resource usage** - Fixed memory, strict budgets, no surprises
- ✅ **Multi-agent systems** - Agents coordinating without human intervention
- ✅ **Embedded/edge deployment** - Single 5MB binary, no runtime
- ✅ **Programmatic control** - Library-first design with full Rust API

### Meerkat vs Python Frameworks (LangChain, AutoGen, CrewAI)

| | Meerkat | Python Frameworks |
|---|---|---|
| **Startup** | <10ms | 1-3s |
| **Memory** | ~20MB | 200MB+ |
| **Deployment** | Single binary | Python + deps |
| **Type safety** | Compile-time | Runtime |
| **Concurrency** | Native async | GIL limitations |
| **Multi-agent** | Built-in encrypted P2P | Framework-specific |

**Choose Meerkat when:**
- Performance and reliability are non-negotiable
- You're deploying to resource-constrained environments
- You need compile-time guarantees
- Your team knows Rust (or wants to learn)

**Choose Python frameworks when:**
- Rapid prototyping is the priority
- You need the Python ML ecosystem
- Your team is Python-native

## Development

```bash
# Build
cargo build --workspace

# Test
cargo test --workspace

# Integration tests
cargo test --package meerkat --test integration

# E2E tests (requires API keys)
cargo test --package meerkat --test e2e -- --ignored

# Cargo aliases
cargo rct    # Run all unit tests
cargo int    # Integration tests
cargo e2e    # E2E tests
```

### CI/CD Pipeline

The project uses a Makefile-driven CI/CD pipeline with pre-commit hooks for local development.

**Setup (one-time):**
```bash
# Install pre-commit hooks
make install-hooks

# Or manually:
pip install pre-commit
pre-commit install
pre-commit install --hook-type pre-push
```

**Makefile targets:**
```bash
make fmt        # Format code with rustfmt
make fmt-check  # Check formatting (CI mode)
make lint       # Run clippy with strict warnings
make test       # Run unit tests
make test-all   # Run all tests including integration
make audit      # Security audit with cargo-deny
make ci         # Full CI pipeline (fmt + lint + test + audit)
```

**Pre-commit hooks (~14s on full codebase):**

| Hook | Stage | Description |
|------|-------|-------------|
| `cargo fmt` | commit | Code formatting |
| `cargo clippy` | commit | Lint checks |
| `cargo test` (unit) | commit | Fast unit tests |
| `cargo test` (all) | push | Full test suite |
| `cargo deny` | push | Security/license audit |
| `gitleaks` | commit | Secret detection |

The hooks run automatically:
- **On commit:** Fast checks (~14s) - formatting, linting, unit tests
- **On push:** Full checks - all tests + security audit

**Rust version:** Pinned to `1.89.0` via `rust-toolchain.toml` for consistent builds.

## Contributing

Contributions are welcome! Please submit PRs to the `main` branch.

Before submitting:
1. Run `make ci` to verify all checks pass
2. Add tests for new functionality
3. Update documentation if needed

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
