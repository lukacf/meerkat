# Meerkat Getting Started Guide

A comprehensive guide to get you running with Meerkat, the Rust Agentic Interface Kit.

## Table of Contents

1. [Prerequisites](#prerequisites)
2. [Installation](#installation)
3. [First Run](#first-run)
4. [CLI Basics](#cli-basics)
5. [SDK Basics](#sdk-basics)
6. [Next Steps](#next-steps)

---

## Prerequisites

### Rust Toolchain

Meerkat requires Rust 1.85 or later:

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Verify version
rustc --version
# Should show: rustc 1.85.0 or later
```

### API Keys

You need at least one LLM provider API key:

| Provider | Environment Variable | Get a Key |
|----------|---------------------|-----------|
| Anthropic | `ANTHROPIC_API_KEY` | [console.anthropic.com](https://console.anthropic.com) |
| OpenAI | `OPENAI_API_KEY` | [platform.openai.com](https://platform.openai.com) |
| Google Gemini | `GOOGLE_API_KEY` | [aistudio.google.com](https://aistudio.google.com) |

---

## Installation

### Option A: Install CLI from Source

```bash
# Clone the repository
git clone https://github.com/your-org/meerkat.git
cd meerkat

# Build and install the CLI binary
cargo install --path meerkat-cli

# Verify installation
rkat --help
```

### Option B: Build from Repository

```bash
# Clone the repository
git clone https://github.com/your-org/meerkat.git
cd meerkat

# Build everything
cargo build --workspace --release

# The CLI binary is at:
./target/release/rkat --help
```

### Option C: Add as Library Dependency

Add to your `Cargo.toml`:

```toml
[dependencies]
meerkat = "0.1"
tokio = { version = "1", features = ["full"] }
```

For multiple providers:

```toml
[dependencies]
meerkat = { version = "0.1", features = ["all-providers"] }
tokio = { version = "1", features = ["full"] }
```

Available features:
- `anthropic` (default) - Claude models
- `openai` - GPT models
- `gemini` - Gemini models
- `all-providers` - All of the above
- `jsonl-store` (default) - Persistent session storage
- `memory-store` - In-memory session storage

---

## First Run

### Step 1: Set Your API Key

```bash
# For Anthropic (default)
export ANTHROPIC_API_KEY="sk-ant-..."

# Or for OpenAI
export OPENAI_API_KEY="sk-..."

# Or for Google Gemini
export GOOGLE_API_KEY="..."
```

Add to your shell profile (`~/.bashrc`, `~/.zshrc`) to persist across sessions.

### Step 2: Run Your First Prompt

Using the CLI:

```bash
rkat run "What is the capital of France?"
```

Expected output:

```
The capital of France is Paris.

[Session: 01936f8a-7b2c-7000-8000-000000000001 | Turns: 1 | Tokens: 42 in / 15 out]
```

### Step 3: Understanding the Output

The CLI shows:
- **Response text** - The LLM's answer
- **Session ID** - Unique identifier for this conversation (use to resume later)
- **Turns** - Number of conversation turns (user prompt + LLM response = 1 turn)
- **Tokens** - Input tokens consumed / Output tokens generated

---

## CLI Basics

### The `run` Command

Basic usage:

```bash
rkat run "Your prompt here"
```

#### All Available Flags

```bash
rkat run [OPTIONS] <PROMPT>

Options:
  --model <MODEL>           Model to use (default: claude-sonnet-4-20250514)
  -p, --provider <PROVIDER> LLM provider: anthropic, openai, gemini (auto-detected from model)
  --max-tokens <N>          Maximum tokens per turn (default: 4096)
  --max-total-tokens <N>    Maximum total tokens for the entire run
  --max-duration <DURATION> Maximum duration (e.g., "5m", "1h30m", "30s")
  --max-tool-calls <N>      Maximum tool calls allowed
  --output <FORMAT>         Output format: text, json (default: text)
  --stream                  Stream tokens to stdout as they arrive
  --param <KEY=VALUE>       Provider-specific parameter (can be repeated)
```

#### Examples

```bash
# Use a different model
rkat run --model claude-opus-4-5 "Explain quantum computing"

# Use OpenAI
rkat run --model gpt-4o "Write a haiku"

# Use Gemini
rkat run --model gemini-2.0-flash "Summarize this text"

# Stream output in real-time
rkat run --stream "Write a short story"

# Limit resources
rkat run --max-tokens 1000 --max-duration 30s "Complex task..."

# JSON output for scripting
rkat run --output json "What is 2+2?" | jq '.text'

# Provider-specific parameters
rkat run --model o1 --param reasoning_effort=high "Solve this math problem"
```

### Session Management

Sessions persist conversations for later resumption.

```bash
# List recent sessions
rkat sessions list
rkat sessions list --limit 50

# Show session details
rkat sessions show <session-id>

# Delete a session
rkat sessions delete <session-id>
```

#### Resuming Sessions

```bash
# Run initial conversation
rkat run "Remember: the secret code is ALPHA-7"
# Output shows: Session: 01936f8a-...

# Later, resume the conversation
rkat resume 01936f8a-... "What was the secret code?"
# The agent remembers: "The secret code is ALPHA-7"
```

### MCP Server Setup

MCP (Model Context Protocol) servers provide tools to agents.

```bash
# Add a stdio MCP server
rkat mcp add filesystem -- npx -y @anthropic/mcp-server-filesystem /tmp

# Add an HTTP MCP server
rkat mcp add my-api --url https://api.example.com/mcp

# Add with environment variables
rkat mcp add github -e GITHUB_TOKEN=ghp_xxx -- npx -y @anthropic/mcp-server-github

# Add to user scope (available globally)
rkat mcp add notes --user -- npx -y @anthropic/mcp-server-memory

# List configured servers
rkat mcp list
rkat mcp list --scope project
rkat mcp list --scope user --json

# Get server details
rkat mcp get filesystem

# Remove a server
rkat mcp remove filesystem
```

Configuration is stored in:
- Project scope: `.rkat/mcp.toml`
- User scope: `~/.config/rkat/mcp.toml`

---

## SDK Basics

### Minimal Example

Create a new project:

```bash
cargo new meerkat-demo
cd meerkat-demo
```

Add dependencies to `Cargo.toml`:

```toml
[dependencies]
meerkat = "0.1"
tokio = { version = "1", features = ["full"] }
```

Write `src/main.rs`:

```rust
use meerkat::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get API key from environment
    let api_key = std::env::var("ANTHROPIC_API_KEY")?;

    // Create and run an agent
    let result = meerkat::with_anthropic(api_key)
        .model("claude-sonnet-4")
        .system_prompt("You are a helpful assistant. Be concise.")
        .max_tokens(1024)
        .run("What is the capital of France?")
        .await?;

    println!("Response: {}", result.text);
    println!("Session ID: {}", result.session_id);
    println!("Turns: {}", result.turns);
    println!("Tokens: {}", result.usage.total_tokens());

    Ok(())
}
```

Run it:

```bash
ANTHROPIC_API_KEY=your-key cargo run
```

### Using Different Providers

```rust
// OpenAI
let result = meerkat::with_openai(std::env::var("OPENAI_API_KEY")?)
    .model("gpt-4o")
    .run("Hello!")
    .await?;

// Gemini
let result = meerkat::with_gemini(std::env::var("GOOGLE_API_KEY")?)
    .model("gemini-2.0-flash-exp")
    .run("Hello!")
    .await?;
```

### Adding Budget Limits

```rust
use meerkat::{BudgetLimits, prelude::*};
use std::time::Duration;

let result = meerkat::with_anthropic(api_key)
    .with_budget(BudgetLimits {
        max_tokens: Some(10_000),                    // Total tokens
        max_duration: Some(Duration::from_secs(60)), // Time limit
        max_tool_calls: Some(20),                    // Tool call limit
    })
    .run("Complex multi-step task")
    .await?;
```

### With Custom Tools (Full Example)

For custom tools, use the full `AgentBuilder` API:

```rust
use async_trait::async_trait;
use meerkat::prelude::*;
use meerkat::{
    AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    LlmStreamResult, Session, ToolDef,
};
use serde_json::{json, Value};
use std::sync::Arc;

// Define your tool dispatcher
struct MathTools;

#[async_trait]
impl AgentToolDispatcher for MathTools {
    fn tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "add".to_string(),
                description: "Add two numbers together".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "a": {"type": "number", "description": "First number"},
                        "b": {"type": "number", "description": "Second number"}
                    },
                    "required": ["a", "b"]
                }),
            },
            ToolDef {
                name: "multiply".to_string(),
                description: "Multiply two numbers".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "a": {"type": "number", "description": "First number"},
                        "b": {"type": "number", "description": "Second number"}
                    },
                    "required": ["a", "b"]
                }),
            },
        ]
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        let a = args["a"].as_f64().ok_or("Missing 'a'")?;
        let b = args["b"].as_f64().ok_or("Missing 'b'")?;

        match name {
            "add" => Ok(format!("{}", a + b)),
            "multiply" => Ok(format!("{}", a * b)),
            _ => Err(format!("Unknown tool: {}", name)),
        }
    }
}

// LLM adapter wrapping the Anthropic client
struct LlmAdapter {
    client: Arc<AnthropicClient>,
    model: String,
}

impl LlmAdapter {
    fn new(api_key: String, model: String) -> Self {
        Self {
            client: Arc::new(AnthropicClient::new(api_key)),
            model,
        }
    }
}

#[async_trait]
impl AgentLlmClient for LlmAdapter {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&serde_json::Value>,
    ) -> Result<LlmStreamResult, meerkat::AgentError> {
        use futures::StreamExt;
        use meerkat::{LlmEvent, LlmRequest, StopReason, ToolCall, Usage};

        let request = LlmRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            max_tokens,
            temperature,
            stop_sequences: None,
            provider_params: provider_params.cloned(),
        };

        let mut stream = self.client.stream(&request);

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = Usage::default();

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => match event {
                    LlmEvent::TextDelta { delta } => content.push_str(&delta),
                    LlmEvent::ToolCallComplete { id, name, args } => {
                        tool_calls.push(ToolCall { id, name, args });
                    }
                    LlmEvent::UsageUpdate { usage: u } => usage = u,
                    LlmEvent::Done { stop_reason: sr } => stop_reason = sr,
                    _ => {}
                },
                Err(e) => return Err(meerkat::AgentError::LlmError(e.to_string())),
            }
        }

        Ok(LlmStreamResult {
            content,
            tool_calls,
            stop_reason,
            usage,
        })
    }

    fn provider(&self) -> &'static str {
        "anthropic"
    }
}

// In-memory session store
struct MemoryStore {
    sessions: std::sync::Mutex<std::collections::HashMap<String, Session>>,
}

impl MemoryStore {
    fn new() -> Self {
        Self {
            sessions: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

#[async_trait]
impl AgentSessionStore for MemoryStore {
    async fn save(&self, session: &Session) -> Result<(), meerkat::AgentError> {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.insert(session.id().to_string(), session.clone());
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<Session>, meerkat::AgentError> {
        let sessions = self.sessions.lock().unwrap();
        Ok(sessions.get(id).cloned())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")?;

    // Create components
    let llm = Arc::new(LlmAdapter::new(api_key, "claude-sonnet-4".to_string()));
    let tools = Arc::new(MathTools);
    let store = Arc::new(MemoryStore::new());

    // Build and run the agent
    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4")
        .system_prompt("You are a math assistant. Use tools to perform calculations.")
        .max_tokens_per_turn(1024)
        .build(llm, tools, store);

    let result = agent
        .run("What is 25 + 17, then multiply by 3?".to_string())
        .await?;

    println!("Response: {}", result.text);
    println!("Tool calls: {}", result.tool_calls);

    Ok(())
}
```

### Running the Examples

The repository includes working examples:

```bash
# Simple chat example
ANTHROPIC_API_KEY=your-key cargo run --example simple

# Custom tools example
ANTHROPIC_API_KEY=your-key cargo run --example with_tools

# Multi-turn conversation with tools
ANTHROPIC_API_KEY=your-key cargo run --example multi_turn_tools
```

---

## Next Steps

### Learn More

- **[Architecture Guide](./architecture.md)** - Understand Meerkat's internals, state machine, and extension points
- **[Examples Guide](./examples.md)** - Detailed examples for common use cases
- **[Configuration Guide](./configuration.md)** - All configuration options
- **[API Reference](./api-reference.md)** - Complete API documentation

### Key Concepts

- **Agent Loop** - The core execution engine: `CallingLlm` -> `WaitingForOps` -> `DrainingEvents` -> `Completed`
- **Sessions** - Persistent conversation state, enables resume
- **Budget** - Resource limits (tokens, time, tool calls)
- **Tools** - Extend agent capabilities via `AgentToolDispatcher` or MCP servers

### Architecture Overview

```
meerkat-core      -> Agent loop, types, budget, retry, state machine
meerkat-client    -> LLM providers (Anthropic, OpenAI, Gemini)
meerkat-store     -> Session persistence (JsonlStore, MemoryStore)
meerkat-tools     -> Tool registry and validation
meerkat-mcp-client -> MCP protocol client, tool routing
meerkat-cli       -> CLI binary (rkat)
meerkat           -> Facade crate with SDK helpers
```

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Error |
| 2 | Budget exhausted (graceful termination) |
