# Meerkat

**Rust Agentic Interface Kit** — A minimal, high-performance agent harness for LLM-powered applications.

[![Crates.io](https://img.shields.io/crates/v/meerkat.svg)](https://crates.io/crates/meerkat)
[![Documentation](https://docs.rs/meerkat/badge.svg)](https://docs.rs/meerkat)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](#license)

```
┌─────────────────────────────────────────────────────────────┐
│                         Meerkat                                │
├─────────────────────────────────────────────────────────────┤
│  1. LLM Client      - Call models with tool definitions     │
│  2. Tool Router     - Dispatch tool calls to MCP servers    │
│  3. Agent Loop      - Accumulate history, detect completion │
│  4. Budget Control  - Enforce time, token, and call limits  │
│  5. Checkpointing   - Save and resume session state         │
└─────────────────────────────────────────────────────────────┘
```

## Features

- **Provider-agnostic**: First-class support for Anthropic, OpenAI, and Gemini
- **MCP-native**: Tools come from MCP servers via the Model Context Protocol
- **Embeddable**: Use as a library, CLI, MCP server, or REST API
- **Resumable**: Checkpoint and resume long-running sessions
- **Observable**: Structured events for monitoring and debugging
- **Type-safe**: Full Rust type safety with comprehensive error handling

## Quick Start

### Installation

Add Meerkat to your `Cargo.toml`:

```toml
[dependencies]
meerkat = "0.1"
tokio = { version = "1", features = ["full"] }
```

### Simple Agent

```rust
use meerkat::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let result = meerkat::with_anthropic(std::env::var("ANTHROPIC_API_KEY")?)
        .model("claude-sonnet-4")
        .system_prompt("You are a helpful assistant.")
        .run("What is the capital of France?")
        .await?;

    println!("{}", result.text);
    Ok(())
}
```

### With Tools

```rust
use meerkat::{AgentBuilder, AgentToolDispatcher, ToolDef};
use async_trait::async_trait;
use serde_json::{json, Value};

struct Calculator;

#[async_trait]
impl AgentToolDispatcher for Calculator {
    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "add".to_string(),
            description: "Add two numbers".to_string(),
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
                Ok(format!("{}", a + b))
            }
            _ => Err(format!("Unknown tool: {}", name)),
        }
    }
}
```

### CLI Usage

```bash
# Install
cargo install meerkat-cli

# Run a simple prompt
rkat run "Explain quantum computing in simple terms"

# Resume a session
rkat resume <session-id> "Can you elaborate on that?"

# With options
rkat run --model claude-opus-4-5 --max-tokens 2048 "Write a haiku"

# Output as JSON
rkat run --output json "What is 2+2?"
```

## Architecture

Meerkat is organized as a workspace of focused crates:

```
meerkat/
├── meerkat-core/        # Agent loop, types, budget, retry logic
├── meerkat-client/      # LLM provider clients (Anthropic, OpenAI, Gemini)
├── meerkat-store/       # Session persistence (JSONL, memory)
├── meerkat-tools/       # Tool registry and validation
├── meerkat-mcp-client/  # MCP protocol client
├── meerkat-mcp-server/  # Expose Meerkat as MCP tools
├── meerkat-rest/        # Optional REST API server
├── meerkat-cli/         # Command-line interface
└── meerkat/             # Facade crate (main entry point)
```

### Access Patterns

Meerkat can be consumed in multiple ways:

| Pattern | Use Case | Entry Point |
|---------|----------|-------------|
| **SDK** | Embed in Rust applications | `meerkat::with_anthropic()` |
| **CLI** | Shell scripts, automation | `rkat run "prompt"` |
| **MCP Server** | Claude Code, other MCP clients | `meerkat_run`, `meerkat_resume` tools |
| **REST API** | HTTP clients, web apps | `POST /sessions` |

## Core Concepts

### Agent Loop

The agent runs a loop until completion:

```
┌─────────────────────────────────────────────────────────┐
│                     Agent Loop                          │
├─────────────────────────────────────────────────────────┤
│                                                         │
│   ┌─────────────┐                                       │
│   │  User Msg   │                                       │
│   └──────┬──────┘                                       │
│          │                                              │
│          ▼                                              │
│   ┌─────────────┐     tool_use      ┌─────────────┐    │
│   │   LLM Call  │ ───────────────── │ Tool Dispatch│    │
│   └──────┬──────┘                   └──────┬──────┘    │
│          │                                 │           │
│          │ end_turn                        │           │
│          ▼                                 │           │
│   ┌─────────────┐                         │           │
│   │   Result    │ ◄───────────────────────┘           │
│   └─────────────┘                                      │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

### Sessions

Sessions persist conversation history and can be resumed:

```rust
// Run initial prompt
let result = agent.run("Start a task").await?;
let session_id = result.session_id;

// Later: resume the session
let resumed = AgentBuilder::new()
    .resume_session(stored_session)
    .build(llm, tools, store);

let result = resumed.run("Continue the task").await?;
```

### Budget Enforcement

Control resource usage with budget limits:

```rust
use meerkat::BudgetLimits;
use std::time::Duration;

let result = meerkat::with_anthropic(api_key)
    .with_budget(BudgetLimits {
        max_tokens: Some(10_000),
        max_duration: Some(Duration::from_secs(60)),
        max_tool_calls: Some(20),
    })
    .run("Complex task")
    .await?;
```

### MCP Tools

Connect to MCP servers for tool access:

```rust
use meerkat::{McpRouter, McpServerConfig};

let config = McpServerConfig {
    name: "filesystem".to_string(),
    command: "npx".to_string(),
    args: vec!["-y", "@anthropic/mcp-server-filesystem", "/tmp"],
    env: Default::default(),
};

let router = McpRouter::new();
router.add_server(config).await?;

// Router implements AgentToolDispatcher
let agent = AgentBuilder::new()
    .build(llm, Arc::new(router), store);
```

## Providers

### Anthropic (Default)

```rust
meerkat::with_anthropic(api_key)
    .model("claude-sonnet-4")  // or claude-opus-4-5
    .run("Hello")
    .await?;
```

### OpenAI

```rust
meerkat::with_openai(api_key)
    .model("gpt-4o")
    .run("Hello")
    .await?;
```

### Gemini

```rust
meerkat::with_gemini(api_key)
    .model("gemini-2.0-flash-exp")
    .run("Hello")
    .await?;
```

## REST API

Start the REST server:

```bash
ANTHROPIC_API_KEY=your-key cargo run --package meerkat-rest
```

Endpoints:

```bash
# Create and run a session
curl -X POST http://localhost:8080/sessions \
  -H "Content-Type: application/json" \
  -d '{"prompt": "Hello!", "model": "claude-sonnet-4"}'

# Continue a session
curl -X POST http://localhost:8080/sessions/{id}/messages \
  -H "Content-Type: application/json" \
  -d '{"prompt": "Tell me more"}'

# Get session details
curl http://localhost:8080/sessions/{id}

# Stream events (SSE)
curl http://localhost:8080/sessions/{id}/events
```

## MCP Server

Meerkat can be exposed as an MCP server for use with Claude Code or other MCP clients:

```json
{
  "mcpServers": {
    "meerkat": {
      "command": "meerkat-mcp-server",
      "env": {
        "ANTHROPIC_API_KEY": "your-key"
      }
    }
  }
}
```

Available tools:
- `meerkat_run` — Run a new agent with a prompt
- `meerkat_resume` — Resume an existing session

## Configuration

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `ANTHROPIC_API_KEY` | Anthropic API key | — |
| `OPENAI_API_KEY` | OpenAI API key | — |
| `GOOGLE_API_KEY` | Google/Gemini API key | — |
| `RKAT_MODEL` | Default model | `claude-sonnet-4` |
| `RKAT_MAX_TOKENS` | Default max tokens | `4096` |
| `RKAT_STORE_PATH` | Session storage path | `~/.local/share/meerkat/sessions` |

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
| `memory-store` | In-memory storage (for tests) |

## Examples

See the [`examples/`](./meerkat/examples) directory:

- [`simple.rs`](./meerkat/examples/simple.rs) — Basic usage with the SDK
- [`with_tools.rs`](./meerkat/examples/with_tools.rs) — Custom tool implementation
- [`multi_turn_tools.rs`](./meerkat/examples/multi_turn_tools.rs) — Multi-turn conversation with tools

Run an example:

```bash
ANTHROPIC_API_KEY=your-key cargo run --example simple
```

## Testing

```bash
# Run unit tests
cargo test --workspace

# Run integration tests
cargo test --package meerkat --test integration

# Run E2E tests (requires API keys)
cargo test --package meerkat --test e2e -- --ignored
```

## Documentation

- [Design Document](./DESIGN.md) — Architecture and design decisions
- [Implementation Plan](./IMPLEMENTATION_PLAN.md) — Development methodology
- [API Documentation](https://docs.rs/meerkat) — Rust API reference

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contributing

Contributions are welcome! Please read the [contribution guidelines](CONTRIBUTING.md) first.
