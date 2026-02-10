# Meerkat SDK Reference

This document provides comprehensive API reference documentation for using Meerkat as a Rust library.

## Table of Contents

- [Installation](#installation)
- [Quick Start](#quick-start)
- [SessionService](#sessionservice)
- [Core Types](#core-types)
- [AgentBuilder](#agentbuilder)
- [Agent](#agent)
- [LLM Clients](#llm-clients)
- [Tool System](#tool-system)
- [Session Stores](#session-stores)
- [MCP Integration](#mcp-integration)
- [Advanced Topics](#advanced-topics)

---

## Installation

Add Meerkat to your `Cargo.toml`:

```toml
[dependencies]
meerkat = "0.1"
tokio = { version = "1", features = ["full"] }
```

### Feature Flags

Meerkat uses feature flags to enable optional functionality:

```toml
[dependencies]
# Default: Anthropic client + JSONL storage
meerkat = "0.1"

# All LLM providers
meerkat = { version = "0.1", features = ["all-providers"] }

# Specific providers
meerkat = { version = "0.1", features = ["anthropic", "openai", "gemini"] }

# Storage backends
meerkat = { version = "0.1", features = ["jsonl-store", "memory-store"] }
```

| Feature | Description | Default |
|---------|-------------|---------|
| `anthropic` | Anthropic Claude API client | Yes |
| `openai` | OpenAI API client | No |
| `gemini` | Google Gemini API client | No |
| `all-providers` | All LLM providers | No |
| `jsonl-store` | File-based session persistence | Yes |
| `memory-store` | In-memory session storage | No |
| `session-store` | Persistent sessions (redb-backed `PersistentSessionService`) | No |
| `session-compaction` | Auto-compact long conversations (`DefaultCompactor`) | No |
| `memory-store-session` | Semantic memory indexing (`HnswMemoryStore`) | No |

---

## Quick Start

### Minimal Example (SessionService)

The recommended way to use Meerkat is through `SessionService` with `build_ephemeral_service`:

```rust
use meerkat::{AgentFactory, AgentBuildConfig, Config, build_ephemeral_service};
use meerkat::service::{CreateSessionRequest, SessionService};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load().await?;
    let factory = AgentFactory::new(std::env::current_dir()?);

    // Build an ephemeral (in-memory) session service
    let service = build_ephemeral_service(factory, config, 64);

    // Create a session and run the first turn
    let result = service.create_session(CreateSessionRequest {
        model: "claude-sonnet-4.5".into(),
        prompt: "What is the capital of France?".into(),
        system_prompt: Some("You are a helpful assistant.".into()),
        max_tokens: Some(1024),
        event_tx: None,
        host_mode: false,
    }).await?;

    println!("Response: {}", result.text);
    println!("Session ID: {}", result.session_id);

    Ok(())
}
```

### With AgentBuildConfig (Per-Request Configuration)

For per-request configuration (custom tools, overrides), stage an `AgentBuildConfig`:

```rust
use meerkat::{AgentFactory, AgentBuildConfig, Config, FactoryAgentBuilder, build_ephemeral_service};
use meerkat::service::{CreateSessionRequest, SessionService};
use std::sync::Arc;

let config = Config::load().await?;
let factory = AgentFactory::new(std::env::current_dir()?);
let service = build_ephemeral_service(factory, config, 64);

// Stage a full build config before creating the session
let build_config = AgentBuildConfig::new("claude-sonnet-4".into());
// ... configure tools, overrides, etc. on build_config ...

// The service picks up the staged config when building the agent
let result = service.create_session(CreateSessionRequest {
    model: "claude-sonnet-4".into(),
    prompt: "Hello!".into(),
    system_prompt: None,
    max_tokens: None,
    event_tx: None,
    host_mode: false,
}).await?;
```

### With Tools Example

Custom tools implement `AgentToolDispatcher`:

```rust
use async_trait::async_trait;
use meerkat::{AgentToolDispatcher, ToolCallView, ToolDef, ToolResult};
use meerkat::error::ToolError;
use serde_json::json;
use std::sync::Arc;

struct MathTools;

#[async_trait]
impl AgentToolDispatcher for MathTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef {
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
        })].into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        #[derive(serde::Deserialize)]
        struct AddArgs { a: f64, b: f64 }

        match call.name {
            "add" => {
                let args: AddArgs = call.parse_args()
                    .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
                Ok(ToolResult::success(call.id, format!("{}", args.a + args.b)))
            }
            _ => Err(ToolError::not_found(call.name)),
        }
    }
}
```

---

## SessionService

`SessionService` is the canonical lifecycle API for all session management. All surfaces (CLI,
REST, MCP Server, JSON-RPC) route through it.

### Creating a Service

```rust
use meerkat::{AgentFactory, Config, build_ephemeral_service};

let config = Config::load().await?;
let factory = AgentFactory::new(std::env::current_dir()?);

// Ephemeral service (in-memory, no persistence)
let service = build_ephemeral_service(factory, config, 64 /* max sessions */);
```

### Multi-Turn Conversations

```rust
use meerkat::service::{CreateSessionRequest, StartTurnRequest, SessionService};

// Turn 1: create session
let result = service.create_session(CreateSessionRequest {
    model: "claude-sonnet-4".into(),
    prompt: "My name is Alice.".into(),
    system_prompt: Some("You are a helpful assistant with memory.".into()),
    max_tokens: None,
    event_tx: None,
    host_mode: false,
}).await?;

let session_id = result.session_id;

// Turn 2: continue on same session
let result = service.start_turn(&session_id, StartTurnRequest {
    prompt: "What's my name?".into(),
    event_tx: None,
    host_mode: false,
}).await?;
// Agent remembers "Alice"

// Read session state
let view = service.read(&session_id).await?;
println!("Messages: {}", view.state.message_count);
println!("Tokens: {}", view.billing.total_tokens);

// Archive when done
service.archive(&session_id).await?;
```

### Error Handling

```rust
use meerkat::service::SessionError;

match service.start_turn(&id, req).await {
    Ok(result) => println!("Response: {}", result.text),
    Err(SessionError::NotFound { id }) => println!("Session {} not found", id),
    Err(SessionError::Busy { id }) => println!("Session {} is busy, retry later", id),
    Err(SessionError::PersistenceDisabled) => println!("Persistence feature not enabled"),
    Err(e) => println!("Error: {}", e),
}
```

---

## Core Types

### Message

Represents a message in the conversation history:

```rust
use meerkat::{Message, UserMessage, AssistantMessage, SystemMessage, ToolResult};

// System message (injected at start)
let system = Message::System(SystemMessage {
    content: "You are helpful.".to_string(),
});

// User message
let user = Message::User(UserMessage {
    content: "Hello!".to_string(),
});

// Assistant message (with potential tool calls)
let assistant = Message::Assistant(AssistantMessage {
    content: "Hello! How can I help?".to_string(),
    tool_calls: vec![],
    stop_reason: StopReason::EndTurn,
    usage: Usage::default(),
});

// Tool results
let tool_results = Message::ToolResults {
    results: vec![ToolResult {
        tool_use_id: "tc_123".to_string(),
        content: "Result data".to_string(),
        is_error: false,
    }],
};
```

### ToolCall

Represents a tool invocation requested by the model:

```rust
use meerkat::ToolCall;
use serde_json::json;

let tool_call = ToolCall {
    id: "tc_123".to_string(),        // Unique ID from the model
    name: "get_weather".to_string(), // Tool name
    args: json!({"city": "Tokyo"}),  // Arguments as JSON
};
```

### ToolResult

The result of executing a tool:

```rust
use meerkat::ToolResult;

let result = ToolResult {
    tool_use_id: "tc_123".to_string(), // Matches tool_call.id
    content: "Sunny, 25C".to_string(), // Result content
    is_error: false,                   // True if tool failed
};
```

### Session and SessionId

A conversation session with full history:

```rust
use meerkat::{Session, SessionId};

// Create new session
let session = Session::new();
println!("Session ID: {}", session.id());

// Parse existing session ID
let id = SessionId::parse("01234567-89ab-cdef-0123-456789abcdef")?;

// Session methods
session.messages();           // Get all messages
session.total_tokens();       // Total tokens used
session.total_usage();        // Detailed usage stats
session.last_assistant_text(); // Last assistant response
session.tool_call_count();    // Number of tool calls made
```

### Usage

Token usage statistics:

```rust
use meerkat::Usage;

let usage = Usage {
    input_tokens: 100,
    output_tokens: 50,
    cache_creation_tokens: Some(20),
    cache_read_tokens: Some(80),
};

println!("Total tokens: {}", usage.total_tokens()); // 150
```

### StopReason

Why the model stopped generating:

```rust
use meerkat::StopReason;

match stop_reason {
    StopReason::EndTurn => println!("Model finished naturally"),
    StopReason::ToolUse => println!("Model wants to call tools"),
    StopReason::MaxTokens => println!("Hit max output tokens"),
    StopReason::StopSequence => println!("Hit a stop sequence"),
    StopReason::ContentFilter => println!("Blocked by content filter"),
    StopReason::Cancelled => println!("Request was cancelled"),
}
```

### RunResult

Result of a successful agent run:

```rust
use meerkat::RunResult;

// Returned by agent.run()
let result: RunResult = agent.run("Hello".to_string()).await?;

println!("Response: {}", result.text);          // Final text response
println!("Session: {}", result.session_id);     // For resumption
println!("Tokens: {}", result.usage.total_tokens());
println!("Turns: {}", result.turns);            // Number of LLM calls
println!("Tool calls: {}", result.tool_calls);  // Number of tool invocations
```

---

## AgentBuilder

> **Note:** For most use cases, prefer `SessionService` via `build_ephemeral_service()` (see
> [SessionService](#sessionservice) above). `AgentBuilder` is used internally by
> `AgentFactory::build_agent()`. Direct usage is only needed for advanced scenarios where you
> bypass the session service entirely.

The `AgentBuilder` provides a fluent API for configuring agents:

```rust
use meerkat::AgentBuilder;

// Used internally by AgentFactory -- prefer SessionService for new code
let agent = AgentBuilder::new()
    .model("claude-sonnet-4.5")
    .system_prompt("You are a helpful assistant.")
    .max_tokens_per_turn(4096)
    .temperature(0.7)
    .build(llm_client, tool_dispatcher, session_store);
```

### All Builder Methods

| Method | Description | Default |
|--------|-------------|---------|
| `model(name)` | Set the model identifier | `"claude-opus-4-6"` |
| `system_prompt(prompt)` | Set the system prompt | None |
| `max_tokens_per_turn(n)` | Max tokens per LLM call | 8192 |
| `temperature(t)` | Sampling temperature (0.0-1.0) | None (model default) |
| `budget(limits)` | Set resource limits | Unlimited |
| `retry_policy(policy)` | Configure retry behavior | 3 retries with backoff |
| `resume_session(session)` | Resume from existing session | New session |
| `provider_params(json)` | Provider-specific parameters | None |
| `concurrency_limits(limits)` | Sub-agent limits | Default limits |
| `with_hook_engine(engine)` | Attach a hook engine implementation | None |
| `with_hook_run_overrides(overrides)` | Apply run-scoped hook overrides | Empty overrides |

### Hook Helpers

The SDK exposes helpers used internally by `AgentFactory` for hook resolution.
When using `SessionService`, hooks are resolved automatically during agent construction:

```rust
use meerkat::{create_default_hook_engine, resolve_layered_hooks_config};

let config = meerkat::Config::load().await?;
let cwd = std::env::current_dir()?;

// These are called internally by AgentFactory::build_agent()
let layered_hooks = resolve_layered_hooks_config(&cwd, &config).await;
let hook_engine = create_default_hook_engine(layered_hooks);
```

`HookRunOverrides` schema:
- `disable`: hook ids disabled for one run
- `entries`: additional hook entries appended after layered config hooks

### Budget Configuration

Budget limits can be set via `AgentBuildConfig` when using `SessionService`, or via `AgentBuilder` directly:

```rust
use meerkat::BudgetLimits;
use std::time::Duration;

let budget = BudgetLimits::default()
    .with_max_tokens(100_000)
    .with_max_duration(Duration::from_secs(300))
    .with_max_tool_calls(50);

// Via AgentBuildConfig (preferred):
let mut build_config = AgentBuildConfig::new("claude-sonnet-4".into());
build_config.budget = Some(budget);
```

### Retry Configuration

```rust
use meerkat::RetryPolicy;
use std::time::Duration;

let retry = RetryPolicy {
    max_retries: 5,
    initial_delay: Duration::from_millis(500),
    max_delay: Duration::from_secs(30),
    multiplier: 2.0,
};
```

### Provider Parameters

Pass provider-specific options like thinking budgets via `AgentBuildConfig`:

```rust
use serde_json::json;

// Anthropic: Enable extended thinking
let mut build_config = AgentBuildConfig::new("claude-sonnet-4".into());
build_config.provider_params = Some(json!({
    "thinking_budget": 10000
}));

// OpenAI: Set reasoning effort
let mut build_config = AgentBuildConfig::new("gpt-5.2".into());
build_config.provider_params = Some(json!({
    "reasoning_effort": "high",
    "seed": 42
}));

// Gemini: Configure thinking
let mut build_config = AgentBuildConfig::new("gemini-3-flash-preview".into());
build_config.provider_params = Some(json!({
    "thinking_budget": 8000,
    "top_k": 40
}));
```

---

## Agent

The `Agent` struct is the main orchestrator that runs the agent loop.

### Running the Agent

```rust
// Simple run
let result = agent.run("What is 2 + 2?".to_string()).await?;
println!("Answer: {}", result.text);

// Run with event streaming
use tokio::sync::mpsc;
use meerkat::AgentEvent;

let (tx, mut rx) = mpsc::channel::<AgentEvent>(100);

// Spawn event handler
tokio::spawn(async move {
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::TextDelta { delta } => print!("{}", delta),
            AgentEvent::ToolExecutionStarted { name, .. } => {
                println!("[Calling {}...]", name);
            }
            AgentEvent::TurnCompleted { usage, .. } => {
                println!("\n[Tokens: {}]", usage.total_tokens());
            }
            _ => {}
        }
    }
});

let result = agent.run_with_events("Tell me a story".to_string(), tx).await?;
```

### Agent Methods

| Method | Description |
|--------|-------------|
| `run(prompt)` | Run agent with user input |
| `run_with_events(prompt, tx)` | Run with event streaming |
| `session()` | Get current session (read-only) |
| `session_mut()` | Get mutable session access |
| `budget()` | Get current budget tracker |
| `state()` | Get current loop state |
| `cancel()` | Cancel the current run |

### Event Types

Events emitted during agent execution:

```rust
use meerkat::AgentEvent;

match event {
    // Session lifecycle
    AgentEvent::RunStarted { session_id, prompt } => {}
    AgentEvent::RunCompleted { session_id, result, usage } => {}
    AgentEvent::RunFailed { session_id, error } => {}

    // LLM interaction
    AgentEvent::TurnStarted { turn_number } => {}
    AgentEvent::TextDelta { delta } => {}
    AgentEvent::TextComplete { content } => {}
    AgentEvent::ToolCallRequested { id, name, args } => {}
    AgentEvent::ToolResultReceived { id, name, is_error } => {}
    AgentEvent::TurnCompleted { stop_reason, usage } => {}

    // Tool execution
    AgentEvent::ToolExecutionStarted { id, name } => {}
    AgentEvent::ToolExecutionCompleted { id, name, result, is_error, duration_ms } => {}
    AgentEvent::ToolExecutionTimedOut { id, name, timeout_ms } => {}

    // Budget
    AgentEvent::BudgetWarning { budget_type, used, limit, percent } => {}
    AgentEvent::CheckpointSaved { session_id, path } => {}

    // Retry
    AgentEvent::Retrying { attempt, max_attempts, error, delay_ms } => {}
}
```

### Error Handling

```rust
use meerkat::AgentError;

match agent.run("prompt".to_string()).await {
    Ok(result) => println!("Success: {}", result.text),
    Err(AgentError::LlmError(msg)) => println!("LLM error: {}", msg),
    Err(AgentError::TokenBudgetExceeded { used, limit }) => {
        println!("Token budget exceeded: {} / {}", used, limit);
    }
    Err(AgentError::TimeBudgetExceeded { elapsed_secs, limit_secs }) => {
        println!("Time budget exceeded: {}s / {}s", elapsed_secs, limit_secs);
    }
    Err(AgentError::ToolCallBudgetExceeded { count, limit }) => {
        println!("Tool call limit exceeded: {} / {}", count, limit);
    }
    Err(e) => println!("Other error: {}", e),
}
```

---

## LLM Clients

Meerkat provides built-in clients for major LLM providers.

### AnthropicClient

```rust
use meerkat::AnthropicClient;

// Create from API key
let client = AnthropicClient::new("sk-ant-...".to_string());

// Create from environment variable (ANTHROPIC_API_KEY)
let client = AnthropicClient::from_env()?;

// Custom base URL (for proxies)
let client = AnthropicClient::new("key".to_string())
    .with_base_url("https://my-proxy.example.com".to_string());
```

### OpenAiClient

```rust
use meerkat::OpenAiClient;

// Create from API key
let client = OpenAiClient::new("sk-...".to_string());

// Create from environment variable (OPENAI_API_KEY)
let client = OpenAiClient::from_env()?;

// Custom base URL (for Azure or compatible APIs)
let client = OpenAiClient::new("key".to_string())
    .with_base_url("https://my-deployment.openai.azure.com".to_string());
```

### GeminiClient

```rust
use meerkat::GeminiClient;

// Create from API key
let client = GeminiClient::new("...".to_string());

// Create from environment variable (GOOGLE_API_KEY)
let client = GeminiClient::from_env()?;
```

### Implementing Custom LLM Client

To integrate a custom LLM provider, implement the `AgentLlmClient` trait:

```rust
use async_trait::async_trait;
use meerkat::{
    AgentLlmClient, AgentError, LlmStreamResult, Message, ToolDef,
    StopReason, ToolCall, Usage,
};
use serde_json::Value;

struct MyCustomClient {
    api_key: String,
}

#[async_trait]
impl AgentLlmClient for MyCustomClient {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&Value>,
    ) -> Result<LlmStreamResult, AgentError> {
        // Call your LLM API here
        // Parse the response into the normalized format

        Ok(LlmStreamResult {
            content: "Response text".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 20,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        })
    }

    fn provider(&self) -> &'static str {
        "my-provider"
    }
}
```

---

## Tool System

### ToolDef Schema

Define tools using JSON Schema:

```rust
use meerkat::ToolDef;
use serde_json::json;

let tool = ToolDef {
    name: "get_weather".to_string(),
    description: "Get current weather for a city".to_string(),
    input_schema: json!({
        "type": "object",
        "properties": {
            "city": {
                "type": "string",
                "description": "City name"
            },
            "units": {
                "type": "string",
                "enum": ["celsius", "fahrenheit"],
                "default": "celsius"
            }
        },
        "required": ["city"]
    }),
};
```

### Implementing AgentToolDispatcher

The `AgentToolDispatcher` trait connects your tools to the agent:

```rust
use async_trait::async_trait;
use meerkat::{AgentToolDispatcher, ToolCallView, ToolDef, ToolResult};
use meerkat::error::ToolError;
use serde_json::json;
use std::sync::Arc;

struct MyToolDispatcher;

#[async_trait]
impl AgentToolDispatcher for MyToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![
            Arc::new(ToolDef {
                name: "search".to_string(),
                description: "Search the web".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"}
                    },
                    "required": ["query"]
                }),
            }),
            Arc::new(ToolDef {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"}
                    },
                    "required": ["path"]
                }),
            }),
        ].into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        match call.name {
            "search" => {
                #[derive(serde::Deserialize)]
                struct Args { query: String }

                let args: Args = call.parse_args()
                    .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
                Ok(ToolResult::success(call.id, format!("Search results for: {}", args.query)))
            }
            "read_file" => {
                #[derive(serde::Deserialize)]
                struct Args { path: String }

                let args: Args = call.parse_args()
                    .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
                let content = std::fs::read_to_string(&args.path)
                    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
                Ok(ToolResult::success(call.id, content))
            }
            _ => Err(ToolError::not_found(call.name)),
        }
    }
}
```

### ToolRegistry

For dynamic tool registration:

```rust
use meerkat::ToolRegistry;

let mut registry = ToolRegistry::new();

// Register tools dynamically
registry.register(tool_def, Box::new(|args| {
    Box::pin(async move {
        // Tool implementation
        Ok("result".to_string())
    })
}));

// Get all registered tools
let tools = registry.tools();
```

---

## Session Stores

### JsonlStore

File-based persistence using JSONL format:

```rust
use meerkat::JsonlStore;
use std::path::PathBuf;

// Create store in a directory
let store = JsonlStore::new(PathBuf::from("./sessions"));

// Initialize (creates directory if needed)
store.init().await?;

// Save session
store.save(&session).await?;

// Load session
let session = store.load(&session_id).await?;

// List sessions with filtering
use meerkat::SessionFilter;

let sessions = store.list(SessionFilter {
    limit: Some(10),
    offset: Some(0),
    created_after: None,
    updated_after: None,
}).await?;

// Delete session
store.delete(&session_id).await?;
```

### MemoryStore

In-memory storage for testing:

```rust
use meerkat::MemoryStore;

let store = MemoryStore::new();

// Same API as JsonlStore
store.save(&session).await?;
let loaded = store.load(&session_id).await?;
```

### Implementing Custom Store

Implement the `AgentSessionStore` trait:

```rust
use async_trait::async_trait;
use meerkat::{AgentSessionStore, AgentError, Session};

struct MyStore {
    // Your storage backend
}

#[async_trait]
impl AgentSessionStore for MyStore {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        // Persist session
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        // Load session by ID
        Ok(None)
    }
}
```

---

## MCP Integration

Meerkat supports the Model Context Protocol (MCP) for connecting to external tool servers.

### McpRouter

Route tool calls across multiple MCP servers:

```rust
use meerkat::{McpRouter, McpServerConfig};
use std::collections::HashMap;

// Create router
let mut router = McpRouter::new();

// Add stdio-based MCP server
let config = McpServerConfig::stdio(
    "my-server",                    // Server name
    "/path/to/mcp-server".to_string(), // Command
    vec!["--arg".to_string()],      // Arguments
    HashMap::new(),                 // Environment variables
);
router.add_server(config).await?;

// Add HTTP/SSE-based MCP server
let config = McpServerConfig::http(
    "remote-server",
    "https://mcp.example.com/sse".to_string(),
);
router.add_server(config).await?;

// List all available tools
let tools = router.list_tools().await?;

// Call a tool
let result = router.call_tool("tool_name", &args).await?;

// Graceful shutdown
router.shutdown().await;
```

### MCP Server Configuration

```rust
use meerkat::McpServerConfig;
use std::collections::HashMap;

// Stdio transport (subprocess)
let config = McpServerConfig::stdio(
    "name",
    "command",
    vec!["args"],
    HashMap::from([("ENV_VAR".to_string(), "value".to_string())]),
);

// HTTP/SSE transport
let config = McpServerConfig::http(
    "name",
    "https://server.example.com/sse",
);
```

---

## Advanced Topics

### Sub-Agent Spawning

Spawn parallel sub-agents for concurrent work:

```rust
use meerkat::{SpawnSpec, ContextStrategy, ToolAccessPolicy, BudgetLimits};

// Define spawn specification
let spec = SpawnSpec {
    prompt: "Analyze this data...".to_string(),
    system_prompt: Some("You are a data analyst.".to_string()),
    context: ContextStrategy::LastN(5),  // Last 5 messages
    tool_access: ToolAccessPolicy::AllowList(vec!["read_file".to_string()]),
    budget: BudgetLimits::default().with_max_tokens(10000),
};

// Spawn sub-agent
let op_id = agent.spawn(spec).await?;

// Collect results at turn boundaries
let results = agent.collect_sub_agent_results().await;
```

### Forking Conversations

Fork into parallel branches:

```rust
use meerkat::{ForkBranch, ForkBudgetPolicy, ToolAccessPolicy};

let branches = vec![
    ForkBranch {
        name: "approach_a".to_string(),
        prompt: "Try approach A...".to_string(),
        tool_access: None,  // Inherit all tools
    },
    ForkBranch {
        name: "approach_b".to_string(),
        prompt: "Try approach B...".to_string(),
        tool_access: Some(ToolAccessPolicy::DenyList(vec!["dangerous_tool".to_string()])),
    },
];

let op_ids = agent.fork(branches, ForkBudgetPolicy::Split).await?;
```

### Budget Management

```rust
use meerkat::{Budget, BudgetLimits, BudgetPool};
use std::time::Duration;

// Create budget limits
let limits = BudgetLimits::default()
    .with_max_tokens(100_000)
    .with_max_duration(Duration::from_secs(600))
    .with_max_tool_calls(100);

// Create budget tracker
let budget = Budget::new(limits);

// Check budget status
if budget.is_exhausted() {
    println!("Budget exhausted!");
}

// Get remaining resources
if let Some(remaining) = budget.remaining_tokens() {
    println!("Remaining tokens: {}", remaining);
}

// Budget pool for sub-agents
let pool = BudgetPool::new(limits);
let allocated = pool.reserve(&child_limits)?;
// ... run child agent ...
pool.reclaim(&allocated, tokens_used);
```

### Streaming Events

Process events in real-time:

```rust
use tokio::sync::mpsc;
use meerkat::{AgentEvent, BudgetType};

let (tx, mut rx) = mpsc::channel::<AgentEvent>(100);

// Process events concurrently
let handle = tokio::spawn(async move {
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::TextDelta { delta } => {
                // Stream text to user
                print!("{}", delta);
                std::io::stdout().flush().ok();
            }
            AgentEvent::ToolExecutionStarted { name, .. } => {
                println!("\n[Executing {}...]", name);
            }
            AgentEvent::ToolExecutionCompleted { name, duration_ms, .. } => {
                println!("[{} completed in {}ms]", name, duration_ms);
            }
            AgentEvent::BudgetWarning { budget_type, percent, .. } => {
                match budget_type {
                    BudgetType::Tokens => println!("[Warning: {}% of token budget used]", percent * 100.0),
                    BudgetType::Time => println!("[Warning: {}% of time budget used]", percent * 100.0),
                    BudgetType::ToolCalls => println!("[Warning: {}% of tool calls used]", percent * 100.0),
                }
            }
            _ => {}
        }
    }
});

// Run agent with event streaming
let result = agent.run_with_events("prompt".to_string(), tx).await?;
handle.await?;
```

### Provider Parameters

Pass provider-specific options via `AgentBuildConfig.provider_params`:

```rust
use serde_json::json;

// Anthropic: Extended thinking
let mut config = AgentBuildConfig::new("claude-sonnet-4".into());
config.provider_params = Some(json!({
    "thinking_budget": 10000,
    "top_k": 40
}));

// OpenAI: Reasoning effort
let mut config = AgentBuildConfig::new("gpt-5.2".into());
config.provider_params = Some(json!({
    "reasoning_effort": "high",
    "seed": 42,
    "frequency_penalty": 0.5,
    "presence_penalty": 0.3
}));

// Gemini: Thinking configuration
let mut config = AgentBuildConfig::new("gemini-3-flash-preview".into());
config.provider_params = Some(json!({
    "thinking_budget": 8000,
    "top_k": 40
}));
```

### Session Resumption via SessionService

Use `SessionService` for multi-turn conversations:

```rust
use meerkat::service::{CreateSessionRequest, StartTurnRequest, SessionService};

// First turn: creates the session
let result = service.create_session(CreateSessionRequest {
    model: "claude-sonnet-4".into(),
    prompt: "Remember: the password is 'secret123'".into(),
    system_prompt: None,
    max_tokens: None,
    event_tx: None,
    host_mode: false,
}).await?;

let session_id = result.session_id;

// Later: continue the conversation on the same session
let result = service.start_turn(&session_id, StartTurnRequest {
    prompt: "What was the password I told you?".into(),
    event_tx: None,
    host_mode: false,
}).await?;
// Agent remembers the previous context
```

### Multi-Turn Conversations

Use `SessionService` to maintain conversation state across turns (see the
[SessionService section](#sessionservice) for the full API).

---

## Complete Example

Here is a complete example using `SessionService` for session lifecycle:

```rust
use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat::service::{CreateSessionRequest, StartTurnRequest, SessionService};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load().await?;
    let factory = AgentFactory::new(std::env::current_dir()?);

    let service = build_ephemeral_service(factory, config, 64);

    // Create a session with the first turn
    let result = service.create_session(CreateSessionRequest {
        model: "claude-sonnet-4".into(),
        prompt: "What is 25 * 17?".into(),
        system_prompt: Some("You are a helpful math assistant.".into()),
        max_tokens: Some(2048),
        event_tx: None,
        host_mode: false,
    }).await?;

    println!("Response: {}", result.text);
    println!("Session ID: {}", result.session_id);

    // Continue the conversation
    let result = service.start_turn(&result.session_id, StartTurnRequest {
        prompt: "Now divide that result by 5.".into(),
        event_tx: None,
        host_mode: false,
    }).await?;

    println!("Follow-up: {}", result.text);

    // Read session state
    let view = service.read(&result.session_id).await?;
    println!("Total tokens: {}", view.billing.total_tokens);
    println!("Messages: {}", view.state.message_count);

    Ok(())
}
```

---

## Python and TypeScript SDKs

In addition to the Rust library documented above, Meerkat provides first-party SDKs for Python
and TypeScript. Both SDKs communicate with a local `rkat rpc` subprocess over JSON-RPC 2.0
(newline-delimited JSON on stdin/stdout) -- no native bindings are required.

### Python SDK (`meerkat-sdk`)

- **Package:** `meerkat-sdk` (install via `pip install sdks/python`)
- **Python:** >= 3.10, zero runtime dependencies
- **Docs:** [sdks/python/README.md](../sdks/python/README.md)

Provides `MeerkatClient` with async session lifecycle (create, turn, interrupt, list, archive),
event streaming, and capability discovery.

### TypeScript SDK (`@meerkat/sdk`)

- **Package:** `@meerkat/sdk` (install via `npm install @meerkat/sdk`)
- **Node.js:** >= 18
- **Docs:** [sdks/typescript/README.md](../sdks/typescript/README.md)

Provides `MeerkatClient` with the same session lifecycle API, typed event streaming via
async iterators, and full TypeScript type definitions.

---

## See Also

- [Architecture](./architecture.md) - System design and internals
- [CAPABILITY_MATRIX.md](./CAPABILITY_MATRIX.md) - Build profiles, error codes, feature behavior
- [SESSION_CONTRACTS.md](./SESSION_CONTRACTS.md) - Session lifecycle operational contracts
- [Configuration](./configuration.md) - Configuration file reference
