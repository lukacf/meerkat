# Configuration Guide

Meerkat supports configuration through configuration files and programmatic settings. Environment variables are used only for secrets (API keys).

## Environment Variables

### API Keys

| Variable | Provider | Required |
|----------|----------|----------|
| `ANTHROPIC_API_KEY` | Anthropic Claude | For Anthropic models |
| `OPENAI_API_KEY` | OpenAI GPT | For OpenAI models |
| `GOOGLE_API_KEY` | Google Gemini | For Gemini models |

### Testing Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `ANTHROPIC_MODEL` | Model for Anthropic E2E tests | `claude-opus-4-6` |
| `OPENAI_MODEL` | Model for OpenAI E2E tests | `gpt-5.2` |
| `GEMINI_MODEL` | Model for Gemini E2E tests | `gemini-3-flash-preview` |

## Configuration Files

Edit `~/.rkat/config.toml` (global) or `.rkat/config.toml` (project) for non-secret settings.

## CLI Options

### rkat run

```bash
rkat run [OPTIONS] <PROMPT>
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
| `--hooks-override-json` | Inline JSON for run-scoped hook overrides | None |
| `--hooks-override-file` | Path to JSON file for run-scoped hook overrides | None |

### rkat resume

```bash
rkat resume [OPTIONS] <SESSION_ID> <PROMPT>
```

| Option | Description | Default |
|--------|-------------|---------|
| `--model`, `-m` | Model to use | `claude-sonnet-4` |
| `--max-tokens`, `-t` | Maximum tokens | `4096` |
| `--output`, `-o` | Output format | `text` |
| `--hooks-override-json` | Inline JSON for run-scoped hook overrides | None |
| `--hooks-override-file` | Path to JSON file for run-scoped hook overrides | None |

## Hook Configuration

Meerkat supports first-class hooks in config and per-run overrides.

### Global and Project Config

Hook entries live under `[hooks]`:

```toml
[hooks]
default_timeout_ms = 5000
payload_max_bytes = 131072
background_max_concurrency = 32

[[hooks.entries]]
id = "pre_tool_guard"
enabled = true
point = "pre_tool_execution"
mode = "foreground"
capability = "guardrail"
priority = 10
failure_policy = "fail_closed"
timeout_ms = 800

[hooks.entries.runtime]
type = "in_process"
name = "pre_tool_guard"
```

Supported points:
- `run_started`
- `run_completed`
- `run_failed`
- `pre_llm_request`
- `post_llm_response`
- `pre_tool_execution`
- `post_tool_execution`
- `turn_boundary`

Supported runtimes:
- `in_process`
- `command`
- `http`

Security warning:
- `command` and `http` hooks are treated as trusted code for the project where they are configured.
- Do not run hook-enabled projects from untrusted sources without reviewing `.rkat/config.toml`.

### Layering and Precedence

Hook entries are merged with deterministic registration order:
1. global config (`~/.rkat/config.toml`)
2. project config (`.rkat/config.toml`)
3. run override entries

Run overrides can also disable configured hooks by id.

### Run Override JSON Schema

All control surfaces accept `HookRunOverrides`:

```json
{
  "disable": ["global_observer"],
  "entries": [
    {
      "id": "run_pre_tool_rewrite",
      "enabled": true,
      "point": "pre_tool_execution",
      "mode": "foreground",
      "capability": "rewrite",
      "priority": 10,
      "runtime": {
        "type": "in_process",
        "name": "run_pre_tool_rewrite"
      }
    }
  ]
}
```

The same payload shape is used by:
- CLI: `--hooks-override-json` / `--hooks-override-file`
- REST: `CreateSessionRequest.hooks_override` / `ContinueSessionRequest.hooks_override`
- MCP server: `MeerkatRunInput.hooks_override` / `MeerkatResumeInput.hooks_override`

### rkat sessions

```bash
rkat sessions list [--limit N]
rkat sessions show <SESSION_ID>
```

## Feature Flags

Configure features in your `Cargo.toml`:

```toml
[dependencies]
meerkat = { version = "0.1", default-features = false, features = ["anthropic"] }
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
meerkat = { version = "0.1", features = ["anthropic", "jsonl-store"] }

# All providers
meerkat = { version = "0.1", features = ["all-providers", "jsonl-store"] }

# Testing (in-memory storage)
meerkat = { version = "0.1", features = ["anthropic", "memory-store"] }
```

## Programmatic Configuration

### Quick Builder (SDK Helpers)

```rust
use meerkat::BudgetLimits;
use std::time::Duration;

let result = meerkat::with_anthropic(api_key)
    .model("claude-opus-4-6")           // Model selection
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
use meerkat::{AgentBuilder, BudgetLimits, RetryPolicy};

let mut agent = AgentBuilder::new()
    // Model settings
    .model("claude-opus-4-6")
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

| Model | Context | Max Output | Best For |
|-------|---------|------------|----------|
| `claude-opus-4-6` | 200K / 1M (beta) | 128K | Complex reasoning, highest quality (default) |
| `claude-sonnet-4-5` | 200K / 1M (beta) | 64K | Balanced performance and cost |
| `claude-opus-4-5` | 200K | 64K | Legacy Opus (still supported) |
| `claude-haiku-4-5` | 200K | 64K | Fast, simple tasks |

### OpenAI Models

| Model | Context | Best For |
|-------|---------|----------|
| `gpt-5.2` | 1M | General purpose, advanced reasoning (default) |
| `gpt-5.2-pro` | 1M | Highest quality reasoning |
| `gpt-5.1-codex-max` | 1M | Code generation, agentic coding |

### Gemini Models

| Model | Context | Best For |
|-------|---------|----------|
| `gemini-3-pro-preview` | 1M | Advanced reasoning, complex tasks |
| `gemini-3-flash-preview` | 1M | Fast, balanced performance |

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
use meerkat::{McpRouter, McpServerConfig};
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

### Config File

```toml
[rest]
host = "0.0.0.0"
port = 3000

[agent]
model = "claude-opus-4-6"
max_tokens_per_turn = 8192
```

### Running the Server

```bash
# Development
cargo run --package meerkat-rest

# Production
cargo build --release --package meerkat-rest
./target/release/meerkat-rest
```

## Session Storage

### JSONL Store (Default)

Sessions are stored as JSONL files:

```
~/.local/share/meerkat/sessions/
├── 01936f8a-7b2c-7000-8000-000000000001.jsonl
├── 01936f8a-7b2c-7000-8000-000000000002.jsonl
└── ...
```

Each file contains the complete session state, updated after each turn.

### Custom Storage Path

```toml
[store]
sessions_path = "/custom/path/to/sessions"
```

Or programmatically:

```rust
use meerkat::JsonlStore;

let store = JsonlStore::new("/custom/path/to/sessions");
store.init().await?;
```

### Memory Store (Testing)

For tests or ephemeral usage:

```rust
use meerkat::MemoryStore;

let store = MemoryStore::new();
// Sessions exist only in memory
```

## Logging

Meerkat uses the `tracing` crate. Configure via `RUST_LOG`:

```bash
# Basic info logging
export RUST_LOG=info

# Debug for Meerkat, info for dependencies
export RUST_LOG=meerkat=debug,info

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

## Memory and Compaction Configuration

### Semantic Memory

When the `memory-store-session` feature is enabled and memory is activated
(`enable_memory` on `AgentFactory` or `override_memory` on `AgentBuildConfig`),
the agent gains a `memory_search` tool backed by `HnswMemoryStore` (hnsw_rs + redb).
Compacted conversation turns are automatically indexed into memory for later recall.

Memory data is stored under `<store_path>/memory/`.

### Compaction Config

Compaction is controlled by `CompactionConfig` (defined in `meerkat-core/src/compact.rs`).
The `DefaultCompactor` in `meerkat-session` uses these defaults:

```toml
# These values are compiled defaults; override programmatically via CompactionConfig.
auto_compact_threshold = 100000   # Trigger when last_input_tokens >= this value
recent_turn_budget = 4            # Number of recent complete turns to retain
max_summary_tokens = 4096         # Max tokens for the compaction summary
min_turns_between_compactions = 3 # Minimum turns between consecutive compactions
```

Compaction requires the `session-compaction` feature flag. When disabled,
`SessionService` returns `SessionError::CompactionDisabled`.

## Sub-Agent Configuration

Sub-agent policy is configured under `[sub_agents]` in config files:

```toml
[sub_agents]
default_provider = "inherit"   # "inherit" = use parent's provider
default_model = "inherit"      # "inherit" = use parent's model

[sub_agents.allowed_models]
anthropic = ["claude-opus-4-6", "claude-sonnet-4-5"]
openai = ["gpt-5.2"]
gemini = ["gemini-3-flash-preview", "gemini-3-pro-preview"]
```

### Concurrency Limits

`ConcurrencyLimits` (defined in `meerkat-core/src/ops.rs`) controls sub-agent
resource usage. Defaults:

| Parameter | Default | Description |
|-----------|---------|-------------|
| `max_depth` | `3` | Maximum sub-agent nesting depth |
| `max_concurrent_ops` | `32` | Maximum concurrent operations (all types) |
| `max_concurrent_agents` | `8` | Maximum concurrent sub-agents |
| `max_children_per_agent` | `5` | Maximum children per parent agent |

Sub-agents require the `sub-agents` feature flag. Enable per-request via
`override_subagents` on `AgentBuildConfig` or `enable_subagents` on `AgentFactory`.

## Comms Configuration

Agent-to-agent communication is configured under `[comms]`:

```toml
[comms]
mode = "inproc"                    # Transport: "inproc", "tcp", or "uds"
address = "127.0.0.1:4200"        # Listen address (tcp/uds only)
auto_enable_for_subagents = false  # Auto-enable comms for spawned sub-agents
```

Comms requires the `comms` feature flag. Enable at the factory level with
`AgentFactory::comms(true)` and per-request with `host_mode` + `comms_name`
on `AgentBuildConfig`.

## Environment Variables (Complete Reference)

### API Keys (RKAT_* takes precedence)

| Variable | Fallback | Provider |
|----------|----------|----------|
| `RKAT_ANTHROPIC_API_KEY` | `ANTHROPIC_API_KEY` | Anthropic Claude |
| `RKAT_OPENAI_API_KEY` | `OPENAI_API_KEY` | OpenAI GPT |
| `RKAT_GEMINI_API_KEY` | `GEMINI_API_KEY`, `GOOGLE_API_KEY` | Google Gemini |

The `RKAT_*` variants take precedence over the provider-native names. This allows
running Meerkat with dedicated keys separate from other tools.

### Testing Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `RKAT_TEST_CLIENT` | Set to `1` to enable test client mode | unset |
| `ANTHROPIC_MODEL` | Model for Anthropic E2E tests | `claude-opus-4-6` |
| `OPENAI_MODEL` | Model for OpenAI E2E tests | `gpt-5.2` |
| `GEMINI_MODEL` | Model for Gemini E2E tests | `gemini-3-flash-preview` |

### Runtime Variables

| Variable | Description |
|----------|-------------|
| `RUST_LOG` | Logging level filter (see [Logging](#logging) above) |
| `HOME` | Used to locate user-level config and skills (`~/.rkat/`) |
