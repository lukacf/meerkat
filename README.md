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
| 🪝 **First-class Hooks** | Core hook points with deterministic rewrite/guardrail semantics |
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

// Each agent has an Ed25519 identity
let keypair = Keypair::generate();

// Comms is wired into the agent at build time via AgentFactory.
// The agent can then send/receive messages from trusted peers.
```

**CLI support:**
```bash
# Configure agent identity and peers
rkat run --comms-name "agent-a" "Send results to agent-b"
```

See [docs/architecture.md](docs/architecture.md) for the full architecture and comms protocol design.

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
     ┌───────────┬───────┬───────┼───────┬───────────┬───────────┐
     │           │       │       │       │           │           │
┌────▼────┐ ┌───▼───┐ ┌─▼───┐ ┌─▼─────┐ ┌───▼───┐ ┌─────▼─────┐
│ client  │ │session│ │store│ │ mcp   │ │ tools │ │  memory   │
├─────────┤ ├───────┤ ├─────┤ ├───────┤ ├───────┤ ├───────────┤
│ LLM     │ │Session│ │JSONL│ │Router │ │ Reg.  │ │ HnswMem.  │
│ APIs    │ │Service│ │ redb│ │Stdio  │ │ Valid.│ │ SimpleMem.│
│Anthropic│ │Compact│ │     │ │HTTP   │ │       │ │           │
│ OpenAI  │ │Events │ │     │ │SSE    │ │       │ │           │
│ Gemini  │ │       │ │     │ │       │ │       │ │           │
└─────────┘ └───────┘ └─────┘ └───────┘ └───────┘ └───────────┘
                                 │
                         ┌───────▼───────┐
                         │  meerkat-core │  No I/O deps
                         │  Traits, loop │  Pure logic
                         │  SessionError │
                         └───────────────┘
```

All four surfaces (CLI, REST, MCP Server, JSON-RPC) route through `SessionService` for session lifecycle. `AgentFactory::build_agent()` centralizes all agent construction.

### Crates

| Crate | Description |
|-------|-------------|
| `meerkat` | Facade crate: re-exports, `AgentFactory`, `FactoryAgentBuilder`, `build_ephemeral_service` |
| `meerkat-core` | Agent loop, state machine, types, trait contracts (`SessionService`, `Compactor`, `MemoryStore`) |
| `meerkat-session` | Session service orchestration (`EphemeralSessionService`, `DefaultCompactor`, `EventStore`) |
| `meerkat-memory` | Semantic memory (`HnswMemoryStore` via hnsw_rs + redb, `SimpleMemoryStore`) |
| `meerkat-hooks` | Hook runtime adapters + deterministic default hook engine |
| `meerkat-client` | LLM providers: Anthropic, OpenAI, Gemini |
| `meerkat-mcp` | MCP protocol client and tool router |
| `meerkat-store` | Session persistence (JSONL, redb, in-memory) |
| `meerkat-tools` | Tool registry and validation |
| `meerkat-cli` | CLI binary (`rkat`) |
| `meerkat-rest` | Optional REST API server |
| `meerkat-mcp-server` | Expose Meerkat as MCP tools |
| `meerkat-rpc` | JSON-RPC stdio server (stateful `SessionRuntime`, IDE/desktop integration) |
| `meerkat-comms` | Ed25519 encrypted P2P messaging protocol |

See [docs/CAPABILITY_MATRIX.md](docs/CAPABILITY_MATRIX.md) for build profiles, error codes, and feature behavior.

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
use meerkat_mcp::{McpRouter, McpServerConfig};

let mut router = McpRouter::new();
router.add_server(McpServerConfig::stdio(
    "filesystem",
    "npx",
    vec!["-y".into(), "@anthropic/mcp-server-filesystem".into(), "/tmp".into()],
    Default::default(),
)).await?;

// McpRouter implements AgentToolDispatcher — pass it to AgentFactory::build_agent()
```

### Session Lifecycle via SessionService

```rust
use meerkat::service::{CreateSessionRequest, StartTurnRequest, SessionService};

// Create a session and run the first turn
let result = service.create_session(CreateSessionRequest {
    model: "claude-sonnet-4".into(),
    prompt: "Start a task".into(),
    system_prompt: None,
    max_tokens: None,
    event_tx: None,
    host_mode: false,
}).await?;

let session_id = result.session_id;

// Later: run another turn on the same session
let result = service.start_turn(&session_id, StartTurnRequest {
    prompt: "Continue the task".into(),
    event_tx: None,
    host_mode: false,
}).await?;
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
  --model <model>           Model (default: claude-opus-4-6)
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

Non-secret settings (model, max tokens, storage, server ports) are configured in
`~/.rkat/config.toml` or `.rkat/config.toml`.

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
| `session-store` | Persistent sessions (redb-backed `PersistentSessionService`) |
| `session-compaction` | Auto-compact long conversations (`DefaultCompactor`) |
| `memory-store-session` | Semantic memory indexing (`HnswMemoryStore`) |

### Pick Only What You Need

```toml
# Minimal: just the agent loop
[dependencies]
meerkat = { version = "0.1", default-features = false, features = ["anthropic"] }

# Add persistence
meerkat = { version = "0.1", default-features = false, features = ["anthropic", "session-store"] }

# Add compaction
meerkat = { version = "0.1", default-features = false, features = ["anthropic", "session-store", "session-compaction"] }

# Kitchen sink
meerkat = { version = "0.1", features = ["all-providers", "session-store", "session-compaction", "memory-store-session"] }
```

When a capability is disabled, the corresponding `SessionService` methods return typed errors (`SessionError::PersistenceDisabled`, `SessionError::CompactionDisabled`) rather than panicking or silently degrading. See [docs/CAPABILITY_MATRIX.md](docs/CAPABILITY_MATRIX.md) for the full behavior matrix.

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

# Fast tests (unit + integration-fast; skips doctests; default for hooks)
cargo test --target-dir target/fast --workspace --lib --bins --tests

# Unit tests only
cargo test --target-dir target/fast --workspace --lib --bins

# Integration-fast tests only
cargo test --target-dir target/fast --workspace --tests

# Integration-real tests (spawns processes / requires binaries)
cargo test --workspace integration_real -- --ignored --test-threads=1

# E2E tests (live APIs; requires keys)
cargo test --workspace e2e_ -- --ignored --test-threads=1

# Cargo aliases (defined in .cargo/config.toml)
cargo rct       # Fast tests (unit + integration-fast)
cargo unit      # Unit tests only
cargo int       # Integration-fast tests only
cargo int-real  # Integration-real tests (ignored by default)
cargo e2e       # E2E tests (ignored by default)
```

**Test categories (convention):**
- **Unit**: no real API calls or process spawning (run via `cargo unit`)
- **Integration-fast**: mocked integrations, fast by default (run via `cargo int`)
- **Integration-real**: tests named `integration_real_*` and marked `#[ignore = "integration-real: ..."]`
- **E2E**: tests named `e2e_*` and marked `#[ignore = "e2e: ..."]`

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
make test       # Fast tests (unit + integration-fast)
make test-unit  # Unit tests only
make test-int   # Integration-fast tests only
make test-int-real # Integration-real tests (ignored by default)
make test-e2e   # E2E tests (ignored by default)
make test-all   # Full test suite (all-features + all-targets)
make audit      # Security audit with cargo-deny
make ci         # Full CI pipeline (fmt + lint + test + audit)
```

**Pre-commit hooks:**

| Hook | Stage | Description |
|------|-------|-------------|
| `cargo test` (fast) | commit | Fast tests (unit + integration-fast) |
| `cargo test` (fast) | push | Fast tests (unit + integration-fast) |
| `cargo clippy` | push | Lint checks |
| `cargo doc` | push | Documentation build |
| `cargo deny` | push | Security/license audit |
| `gitleaks` | push | Secret detection |

The hooks run automatically:
- **On commit:** Fast tests
- **On push:** Fast tests + lint/docs/audit/secret checks

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
