# Configuration Guide

RAIK supports configuration through multiple layers: environment variables, configuration files, and programmatic settings.

## Environment Variables

### API Keys

| Variable | Provider | Required |
|----------|----------|----------|
| `ANTHROPIC_API_KEY` | Anthropic Claude | For Anthropic models |
| `OPENAI_API_KEY` | OpenAI GPT | For OpenAI models |
| `GOOGLE_API_KEY` | Google Gemini | For Gemini models |

### Runtime Settings

| Variable | Description | Default |
|----------|-------------|---------|
| `RAIK_MODEL` | Default model name | Provider-dependent |
| `RAIK_MAX_TOKENS` | Default max tokens per turn | `4096` |
| `RAIK_STORE_PATH` | Session storage directory | `~/.local/share/raik/sessions` |
| `RAIK_HOST` | REST API host | `127.0.0.1` |
| `RAIK_PORT` | REST API port | `8080` |

### Testing Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `ANTHROPIC_MODEL` | Model for Anthropic E2E tests | `claude-opus-4-5` |
| `OPENAI_MODEL` | Model for OpenAI E2E tests | `gpt-5.2` |
| `GEMINI_MODEL` | Model for Gemini E2E tests | `gemini-3-flash-preview` |

## CLI Options

### raik run

```bash
raik run [OPTIONS] <PROMPT>
```

| Option | Description | Default |
|--------|-------------|---------|
| `--model`, `-m` | Model to use | `claude-sonnet-4` |
| `--max-tokens`, `-t` | Maximum tokens per turn | `4096` |
| `--max-duration` | Maximum runtime (e.g., "30s", "5m") | None |
| `--max-tool-calls` | Maximum tool invocations | None |
| `--system-prompt`, `-s` | System prompt | None |
| `--output`, `-o` | Output format (`text`, `json`) | `text` |
| `--stream` | Stream output as it arrives | `false` |

### raik resume

```bash
raik resume [OPTIONS] <SESSION_ID> <PROMPT>
```

| Option | Description | Default |
|--------|-------------|---------|
| `--model`, `-m` | Model to use | `claude-sonnet-4` |
| `--max-tokens`, `-t` | Maximum tokens | `4096` |
| `--output`, `-o` | Output format | `text` |

### raik sessions

```bash
raik sessions list [--limit N]
raik sessions show <SESSION_ID>
```

## Feature Flags

Configure features in your `Cargo.toml`:

```toml
[dependencies]
raik = { version = "0.1", default-features = false, features = ["anthropic"] }
```

### Provider Features

| Feature | Description | Default |
|---------|-------------|---------|
| `anthropic` | Anthropic Claude support | Yes |
| `openai` | OpenAI GPT support | No |
| `gemini` | Google Gemini support | No |
| `all-providers` | All LLM providers | No |

### Storage Features

| Feature | Description | Default |
|---------|-------------|---------|
| `jsonl-store` | JSONL file storage | Yes |
| `memory-store` | In-memory storage | No |

### Common Configurations

```toml
# Anthropic only (smallest binary)
raik = { version = "0.1", features = ["anthropic", "jsonl-store"] }

# All providers
raik = { version = "0.1", features = ["all-providers", "jsonl-store"] }

# Testing (in-memory storage)
raik = { version = "0.1", features = ["anthropic", "memory-store"] }
```

## Programmatic Configuration

### Quick Builder (SDK Helpers)

```rust
use raik::BudgetLimits;
use std::time::Duration;

let result = raik::with_anthropic(api_key)
    .model("claude-opus-4-5")           // Model selection
    .system_prompt("You are helpful.")   // System prompt
    .max_tokens(2048)                    // Tokens per turn
    .with_budget(BudgetLimits {          // Resource limits
        max_tokens: Some(50_000),
        max_duration: Some(Duration::from_secs(120)),
        max_tool_calls: Some(50),
    })
    .with_retry_policy(RetryPolicy {     // Error handling
        max_retries: 5,
        initial_delay: Duration::from_millis(500),
        max_delay: Duration::from_secs(30),
        multiplier: 2.0,
    })
    .run("Your prompt")
    .await?;
```

### Full AgentBuilder

For complete control:

```rust
use raik::{AgentBuilder, BudgetLimits, RetryPolicy};

let mut agent = AgentBuilder::new()
    // Model settings
    .model("claude-opus-4-5")
    .system_prompt("You are a helpful assistant.")
    .max_tokens_per_turn(4096)

    // Resource limits
    .budget(BudgetLimits {
        max_tokens: Some(100_000),
        max_duration: Some(Duration::from_secs(300)),
        max_tool_calls: Some(100),
    })

    // Retry configuration
    .retry_policy(RetryPolicy {
        max_retries: 3,
        initial_delay: Duration::from_millis(500),
        max_delay: Duration::from_secs(10),
        multiplier: 2.0,
    })

    // Resume existing session
    .resume_session(existing_session)

    // Build with dependencies
    .build(llm_client, tool_dispatcher, session_store);
```

## Models

### Anthropic Models

| Model | Context | Best For |
|-------|---------|----------|
| `claude-opus-4-5` | 200K | Complex reasoning, highest quality |
| `claude-sonnet-4` | 200K | Balanced performance and cost |
| `claude-haiku-3-5` | 200K | Fast, simple tasks |

### OpenAI Models

| Model | Context | Best For |
|-------|---------|----------|
| `gpt-4o` | 128K | General purpose |
| `gpt-4-turbo` | 128K | Complex tasks |
| `gpt-3.5-turbo` | 16K | Simple, fast tasks |

### Gemini Models

| Model | Context | Best For |
|-------|---------|----------|
| `gemini-2.0-flash-exp` | 1M | Fast, large context |
| `gemini-1.5-pro` | 2M | Largest context window |
| `gemini-1.5-flash` | 1M | Balanced |

## Budget Configuration

### Token Limits

```rust
BudgetLimits {
    max_tokens: Some(10_000),  // Total input + output tokens
    ..Default::default()
}
```

When exceeded: Agent completes current turn, then stops with partial results.

### Duration Limits

```rust
BudgetLimits {
    max_duration: Some(Duration::from_secs(60)),
    ..Default::default()
}
```

When exceeded: Agent completes current turn, then stops.

### Tool Call Limits

```rust
BudgetLimits {
    max_tool_calls: Some(20),
    ..Default::default()
}
```

When exceeded: Tool calls are rejected, agent continues without tools.

## Retry Configuration

### Default Policy

```rust
RetryPolicy {
    max_retries: 3,
    initial_delay: Duration::from_millis(500),
    max_delay: Duration::from_secs(10),
    multiplier: 2.0,
}
```

### Retryable Errors

| Error | Retried |
|-------|---------|
| `RateLimited` | Yes (with retry-after if provided) |
| `ServerOverloaded` | Yes |
| `NetworkTimeout` | Yes |
| `ConnectionReset` | Yes |
| `ServerError (5xx)` | Yes |
| `InvalidRequest` | No |
| `AuthenticationFailed` | No |
| `ContentFiltered` | No |
| `ContextLengthExceeded` | No |

### Delay Calculation

```
delay = min(initial_delay * multiplier^attempt, max_delay) * jitter
```

Where `jitter` is a random factor between 0.9 and 1.1.

## MCP Server Configuration

### Via JSON

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@anthropic/mcp-server-filesystem", "/tmp"],
      "env": {
        "DEBUG": "true"
      }
    }
  }
}
```

### Via Code

```rust
use raik::{McpRouter, McpServerConfig};
use std::collections::HashMap;

let config = McpServerConfig {
    name: "filesystem".to_string(),
    command: "npx".to_string(),
    args: vec![
        "-y".to_string(),
        "@anthropic/mcp-server-filesystem".to_string(),
        "/tmp".to_string(),
    ],
    env: HashMap::new(),
};

let router = McpRouter::new();
router.add_server(config).await?;
```

## REST API Configuration

### Environment Variables

```bash
export RAIK_HOST=0.0.0.0      # Bind to all interfaces
export RAIK_PORT=3000          # Custom port
export RAIK_MODEL=claude-opus-4-5  # Default model
export RAIK_MAX_TOKENS=8192    # Default max tokens
```

### Running the Server

```bash
# Development
cargo run --package raik-rest

# Production
cargo build --release --package raik-rest
./target/release/raik-rest
```

## Session Storage

### JSONL Store (Default)

Sessions are stored as JSONL files:

```
~/.local/share/raik/sessions/
├── 01936f8a-7b2c-7000-8000-000000000001.jsonl
├── 01936f8a-7b2c-7000-8000-000000000002.jsonl
└── ...
```

Each file contains the complete session state, updated after each turn.

### Custom Storage Path

```bash
export RAIK_STORE_PATH=/custom/path/to/sessions
```

Or programmatically:

```rust
use raik::JsonlStore;

let store = JsonlStore::new("/custom/path/to/sessions");
store.init().await?;
```

### Memory Store (Testing)

For tests or ephemeral usage:

```rust
use raik::MemoryStore;

let store = MemoryStore::new();
// Sessions exist only in memory
```

## Logging

RAIK uses the `tracing` crate. Configure via `RUST_LOG`:

```bash
# Basic info logging
export RUST_LOG=info

# Debug for RAIK, info for dependencies
export RUST_LOG=raik=debug,info

# Full trace logging
export RUST_LOG=trace
```

### Log Levels

| Level | What's Logged |
|-------|---------------|
| `error` | Failures only |
| `warn` | Warnings and above |
| `info` | Session lifecycle, turns, completions |
| `debug` | Tool calls, responses, retries |
| `trace` | Wire-level protocol details |
