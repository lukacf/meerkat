# Architecture Guide

This document explains Meerkat's internal architecture, design decisions, and extension points.

## Overview

Meerkat is designed as a **minimal, composable agent harness**. It provides the core execution loop for LLM-powered agents without opinions about prompts, tools, or output formatting.

### Design Philosophy

1. **Minimal** - Provides the core execution loop without opinions about prompts, tools, or output formatting
2. **Composable** - Clean trait boundaries allow swapping LLM providers, storage backends, and tool dispatchers
3. **No I/O in Core** - `meerkat-core` has no network or filesystem dependencies; all I/O is in satellite crates
4. **Streaming-First** - All LLM interactions use streaming for responsive user experiences
5. **Budget-Aware** - Built-in resource tracking for tokens, time, and tool calls

### High-Level Architecture

```
                                    +------------------+
                                    |     meerkat      |  <- Facade crate
                                    |  (re-exports &   |     SDK helpers
                                    |   SDK helpers)   |
                                    +--------+---------+
                                             |
         +-----------------------------------+-----------------------------------+
         |                   |               |               |                   |
+--------v-------+  +--------v-------+  +----v----+  +------v------+  +---------v---------+
| meerkat-client |  | meerkat-store  |  | meerkat |  | meerkat-mcp |  | meerkat-mcp-server|
| (LLM providers)|  | (persistence)  |  | -tools  |  |   -client   |  | (expose as MCP)   |
+--------+-------+  +--------+-------+  +----+----+  +------+------+  +---------+---------+
         |                   |               |               |                   |
         +-------------------+-------+-------+---------------+-------------------+
                                     |
                             +-------v-------+
                             | meerkat-core  |  <- No I/O dependencies
                             | (agent loop,  |     Pure logic
                             |  state, types)|
                             +---------------+
```

### Detailed Component View

```
+-----------------------------------------------------------------------------+
|                              Access Patterns                                 |
+---------------+---------------+---------------------+-----------------------+
|  meerkat-cli  |  meerkat-mcp  |   meerkat (SDK)     |  meerkat-rest (opt)   |
|  (headless)   |  (server)     |                     |  (HTTP API)           |
+-------+-------+-------+-------+----------+----------+-----------+-----------+
        |               |                  |                      |
        +---------------+---------+--------+----------------------+
                                  |
                                  v
+-----------------------------------------------------------------------------+
|                              meerkat-core                                    |
|                                                                              |
|  +------------------------------------------------------------------------+  |
|  |  Agent                                                                 |  |
|  |  +-- llm_client: Arc<dyn AgentLlmClient>     (provider abstraction)    |  |
|  |  +-- tool_dispatcher: Arc<dyn AgentToolDispatcher>  (tool routing)     |  |
|  |  +-- session_store: Arc<dyn AgentSessionStore>      (persistence)      |  |
|  |  +-- session: Session                               (conversation)     |  |
|  |  +-- budget: Budget                                 (resource limits)  |  |
|  |  +-- retry_policy: RetryPolicy                      (error handling)   |  |
|  |  +-- state: LoopState                               (state machine)    |  |
|  |  +-- sub_agent_manager: SubAgentManager             (child agents)     |  |
|  +------------------------------------------------------------------------+  |
+-----------------------------------------------------------------------------+
                                  |
                    +-------------+-------------+
                    |             |             |
                    v             v             v
            +-------------+ +-------------+ +-------------+
            |meerkat-     | |meerkat-     | |meerkat-     |
            |client       | |tools        | |store        |
            |             | |             | |             |
            | Anthropic   | | Registry    | | JsonlStore  |
            | OpenAI      | | Dispatcher  | | MemoryStore |
            | Gemini      | | Validation  | |             |
            +-------------+ +------+------+ +-------------+
                                   |
                                   v
                           +-----------------+
                           | meerkat-mcp-    |
                           | client          |
                           |                 |
                           | MCP Protocol    |
                           | Tool Discovery  |
                           | Tool Dispatch   |
                           +-----------------+
```

## Crate Structure

### meerkat-core

The heart of Meerkat. Contains:

- **Agent**: The main execution engine
- **Types**: `Message`, `Session`, `ToolCall`, `ToolResult`, `Usage`, etc.
- **Budget**: Resource tracking and enforcement
- **Retry**: Exponential backoff with jitter
- **State Machine**: `LoopState` for agent lifecycle management
- **Sub-agents**: Fork and spawn operations

### meerkat-client

LLM provider implementations:

- **AnthropicClient**: Claude models via Messages API
- **OpenAiClient**: GPT models via Chat Completions API
- **GeminiClient**: Gemini models via GenerateContent API

All providers implement the `LlmClient` trait and normalize responses to `LlmEvent`:

```rust
pub trait LlmClient: Send + Sync {
    fn stream(&self, request: &LlmRequest) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send>>;
    fn provider(&self) -> &'static str;
}

pub enum LlmEvent {
    TextDelta { delta: String },
    ToolCallDelta { id: String, name: Option<String>, args_delta: String },
    ToolCallComplete { id: String, name: String, args: Value },
    UsageUpdate { usage: Usage },
    Done { stop_reason: StopReason },
}
```

### meerkat-store

Session persistence:

- **JsonlStore**: Production storage using append-only JSONL files
- **MemoryStore**: In-memory storage for testing
- **SessionStore trait**: Implement for custom backends

### meerkat-tools

Tool management:

- **ToolRegistry**: Register and validate tool definitions
- **ToolDispatcher**: Route tool calls with timeout handling
- **Schema validation**: JSON Schema validation of tool arguments

### meerkat-mcp-client

MCP protocol implementation:

- **McpConnection**: Manages stdio connection to MCP server
- **McpRouter**: Routes tool calls to appropriate MCP servers
- **Protocol handling**: Initialize, tools/list, tools/call

### meerkat (facade)

The main entry point. Re-exports types and provides SDK helpers:

```rust
// SDK helpers
pub fn with_anthropic(api_key: impl Into<String>) -> QuickBuilder<AnthropicClient>;
pub fn with_openai(api_key: impl Into<String>) -> QuickBuilder<OpenAiClient>;
pub fn with_gemini(api_key: impl Into<String>) -> QuickBuilder<GeminiClient>;

// Re-exports all public types
pub use meerkat_core::{Agent, AgentBuilder, Session, Message, ...};
pub use meerkat_client::{LlmClient, LlmEvent, ...};
pub use meerkat_store::{SessionStore, ...};
```

## Agent Loop

The agent executes a state machine loop defined by `LoopState` in `meerkat-core/src/state.rs`:

```
                              +------------------+
                              |   CallingLlm     |<-----------+
                              | (waiting for LLM)|            |
                              +--------+---------+            |
                                       |                      |
            +-----------+   tool_use   |  end_turn    +-------+------+
            |           |<-------------+------------->|   Completed  |
            v           |                             | (terminal)   |
    +-------+-------+   |                             +--------------+
    | WaitingForOps |   |                                    ^
    | (ops pending) |   |                                    |
    +-------+-------+   |                                    |
            |           |                             +------+-----+
            | ops done  |                             |  Cancelling |
            v           |                             | (cleanup)   |
    +-------+-------+   |                             +------+-----+
    | DrainingEvents|---+                                    ^
    | (process      |   | error                              |
    |  buffered)    |   +------------->+-------------+       |
    +---------------+                  |ErrorRecovery+-------+
                                       | (retry)     |
                                       +-------------+
```

### States

| State | Description |
|-------|-------------|
| `CallingLlm` | Sending request to LLM, streaming response |
| `WaitingForOps` | No LLM work, waiting for tool operations to complete |
| `DrainingEvents` | Processing buffered operation events at turn boundary |
| `Cancelling` | Gracefully stopping after cancel signal or budget exhaustion |
| `ErrorRecovery` | Attempting recovery from transient LLM error |
| `Completed` | Terminal state - agent has finished (success or failure) |

### Valid State Transitions

```
CallingLlm     -> WaitingForOps   (ops pending after tool dispatch)
CallingLlm     -> DrainingEvents  (tool_use stop reason)
CallingLlm     -> Completed       (end_turn and no ops)
CallingLlm     -> ErrorRecovery   (LLM error)
CallingLlm     -> Cancelling      (cancel signal)

WaitingForOps  -> DrainingEvents  (when ops complete)
WaitingForOps  -> Cancelling      (cancel signal)

DrainingEvents -> CallingLlm      (more work needed)
DrainingEvents -> Completed       (done)
DrainingEvents -> Cancelling      (cancel signal)

Cancelling     -> Completed       (after drain)

ErrorRecovery  -> CallingLlm      (after recovery)
ErrorRecovery  -> Completed       (if unrecoverable)
ErrorRecovery  -> Cancelling      (cancel during recovery)

Completed      -> (no transitions, terminal state)
```

### Turn Boundaries

Turn boundaries are critical moments where:

1. Tool results are injected into the session
2. Steering messages (from parent agents) are applied
3. Comms inbox is drained and messages injected (for inter-agent communication)
4. Sub-agent results are collected and injected
5. Budget is checked
6. Session is checkpointed

Events that happen at turn boundaries:
- `TurnCompleted`
- `ToolResultReceived`
- `CheckpointSaved`

### Data Flow Through Agent Loop

```
User Input
    |
    v
+---+---+
| Agent |---------------------------+
+---+---+                           |
    |                               |
    | 1. Add user message           |
    v                               |
+--------+                          |
| Session|                          |
+---+----+                          |
    |                               |
    | 2. Call LLM with retry        |
    v                               |
+--------+     messages + tools     |
|  LLM   |<-------------------------+
| Client |                          |
+---+----+                          |
    |                               |
    | 3. Stream response            |
    v                               |
+--------+                          |
|Response|                          |
|--------|                          |
|content |                          |
|tools[] |                          |
|usage   |                          |
+---+----+                          |
    |                               |
    | 4. If tool_calls              |
    v                               |
+--------+     name + args          |
| Tool   |<-------------------------+
|Dispatch|                          |
+---+----+                          |
    |                               |
    | 5. Execute & return           |
    v                               |
+--------+                          |
|Results |                          |
+---+----+                          |
    |                               |
    | 6. Add to session             |
    | 7. Apply steering messages    |
    | 8. Drain comms inbox          |
    | 9. Collect sub-agent results  |
    | 10. Loop back to step 2       |
    +-------------------------------+
```

## Type System

### Messages

```rust
pub enum Message {
    System(SystemMessage),      // System prompt
    User(UserMessage),          // User input
    Assistant(AssistantMessage), // LLM response
    ToolResults { results: Vec<ToolResult> }, // Tool outputs
}

pub struct AssistantMessage {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: StopReason,
    pub usage: Usage,
}
```

### Stop Reasons

```rust
pub enum StopReason {
    EndTurn,        // Natural completion
    ToolUse,        // Wants to call tools
    MaxTokens,      // Hit token limit
    StopSequence,   // Hit stop sequence
    ContentFilter,  // Content was filtered
    Cancelled,      // User cancelled
}
```

### Events

```rust
pub enum AgentEvent {
    RunStarted { session_id: SessionId },
    TurnStarted { turn: u32 },
    TextDelta { delta: String },
    ToolCallRequested { id: String, name: String },
    ToolResultReceived { id: String, result: String },
    TurnCompleted { turn: u32, usage: Usage },
    RunCompleted { result: RunResult },
    RunFailed { error: String },
    CheckpointSaved { session_id: SessionId },
}
```

## Extension Points

### Custom LLM Provider

Implement `AgentLlmClient` (defined in `meerkat-core/src/agent.rs`):

```rust
#[async_trait]
pub trait AgentLlmClient: Send + Sync {
    /// Stream a response from the LLM
    ///
    /// # Arguments
    /// * `messages` - Conversation history
    /// * `tools` - Available tool definitions
    /// * `max_tokens` - Maximum tokens to generate
    /// * `temperature` - Sampling temperature
    /// * `provider_params` - Provider-specific parameters (e.g., thinking config)
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&Value>,
    ) -> Result<LlmStreamResult, AgentError>;

    /// Get the provider name
    fn provider(&self) -> &'static str;
}
```

**Example Implementation:**

```rust
use meerkat_core::{AgentLlmClient, AgentError, LlmStreamResult, Message, ToolDef};
use serde_json::Value;

pub struct CustomClient { /* ... */ }

#[async_trait]
impl AgentLlmClient for CustomClient {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&Value>,
    ) -> Result<LlmStreamResult, AgentError> {
        // 1. Convert messages to provider format
        // 2. Make API request
        // 3. Parse streaming response
        // 4. Return normalized LlmStreamResult
        todo!()
    }

    fn provider(&self) -> &'static str {
        "custom"
    }
}
```

### Custom Tool Dispatcher

Implement `AgentToolDispatcher` (defined in `meerkat-core/src/agent.rs`):

```rust
#[async_trait]
pub trait AgentToolDispatcher: Send + Sync {
    /// Get available tool definitions
    fn tools(&self) -> Vec<ToolDef>;

    /// Execute a tool call
    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String>;
}
```

**Example Implementation:**

```rust
use meerkat_core::{AgentToolDispatcher, ToolDef};
use serde_json::Value;

pub struct CustomDispatcher { /* ... */ }

#[async_trait]
impl AgentToolDispatcher for CustomDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "my_tool".to_string(),
                description: "Does something useful".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "arg": { "type": "string" }
                    },
                    "required": ["arg"]
                }),
            }
        ]
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "my_tool" => {
                // Execute tool logic
                let arg = args.get("arg").and_then(|v| v.as_str()).unwrap_or("");
                Ok(format!("Processed: {}", arg))
            }
            _ => Err(format!("Unknown tool: {}", name))
        }
    }
}
```

### Custom Session Store

Implement `AgentSessionStore` (defined in `meerkat-core/src/agent.rs`):

```rust
#[async_trait]
pub trait AgentSessionStore: Send + Sync {
    /// Save a session
    async fn save(&self, session: &Session) -> Result<(), AgentError>;

    /// Load a session by ID
    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError>;
}
```

**Example Implementation:**

```rust
use meerkat_core::{AgentSessionStore, AgentError, Session};

pub struct DatabaseStore {
    pool: sqlx::PgPool,
}

#[async_trait]
impl AgentSessionStore for DatabaseStore {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        let json = serde_json::to_string(session)
            .map_err(|e| AgentError::Internal(e.to_string()))?;

        sqlx::query("INSERT INTO sessions (id, data) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET data = $2")
            .bind(session.id().to_string())
            .bind(json)
            .execute(&self.pool)
            .await
            .map_err(|e| AgentError::Internal(e.to_string()))?;

        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        let row: Option<(String,)> = sqlx::query_as("SELECT data FROM sessions WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AgentError::Internal(e.to_string()))?;

        match row {
            Some((json,)) => {
                let session: Session = serde_json::from_str(&json)
                    .map_err(|e| AgentError::Internal(e.to_string()))?;
                Ok(Some(session))
            }
            None => Ok(None),
        }
    }
}
```

### MCP Integration

Add MCP servers for tool discovery using `McpRouter` (defined in `meerkat-mcp-client/src/router.rs`):

```rust
use meerkat_mcp_client::{McpRouter, McpServerConfig};
use std::collections::HashMap;

let mut router = McpRouter::new();

// Add stdio server
router.add_server(McpServerConfig::stdio(
    "my-server",
    "/path/to/server",
    vec!["arg1".to_string(), "arg2".to_string()],
    HashMap::new(),
)).await?;

// Tools are automatically discovered
let tools = router.list_tools().await?;

// Call a tool
let result = router.call_tool("tool_name", &serde_json::json!({"arg": "value"})).await?;

// Graceful shutdown
router.shutdown().await;
```

## Sub-Agents

Meerkat supports spawning child agents for parallel work:

### Fork

Creates a child with **full conversation history**:

```rust
let branches = vec![
    ForkBranch {
        id: "branch_a".to_string(),
        prompt: "Analyze approach A".to_string(),
    },
    ForkBranch {
        id: "branch_b".to_string(),
        prompt: "Analyze approach B".to_string(),
    },
];

agent.fork(branches).await?;
```

### Spawn

Creates a child with **minimal context** per `ContextStrategy`:

```rust
let spec = SpawnSpec {
    id: "worker".to_string(),
    prompt: "Process this data".to_string(),
    context_strategy: ContextStrategy::LastTurns(3),
    tool_access: ToolAccessPolicy::AllowList(vec!["read_file".to_string()]),
    budget_policy: ForkBudgetPolicy::Fixed(1000),
};

agent.spawn(spec).await?;
```

### Context Strategies

| Strategy | Description |
|----------|-------------|
| `FullHistory` | Copy entire conversation |
| `LastTurns(n)` | Copy last N turns |
| `Summary { max_tokens }` | Fit messages within token budget |
| `Custom { messages }` | Provide explicit message list |

## Error Handling

### LLM Errors

```rust
pub enum LlmError {
    RateLimited { retry_after: Option<Duration> },
    ServerOverloaded,
    NetworkTimeout,
    ConnectionReset,
    ServerError { status: u16, message: String },
    InvalidRequest { message: String },
    AuthenticationFailed,
    ContentFiltered,
    ContextLengthExceeded,
    ModelNotFound { model: String },
    InvalidApiKey,
    Unknown { message: String },
}

impl LlmError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            LlmError::RateLimited { .. }
                | LlmError::ServerOverloaded
                | LlmError::NetworkTimeout
                | LlmError::ConnectionReset
                | LlmError::ServerError { .. }
        )
    }
}
```

### Retry Policy

Retryable errors trigger exponential backoff:

```rust
pub struct RetryPolicy {
    pub max_retries: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
}

// Delay calculation with ±10% jitter
delay = min(initial_delay * multiplier^attempt, max_delay) * random(0.9, 1.1)
```

## Budget Enforcement

Budget tracks three dimensions:

```rust
pub struct BudgetLimits {
    pub max_tokens: Option<u64>,      // Total input + output tokens
    pub max_duration: Option<Duration>, // Wall clock time
    pub max_tool_calls: Option<u32>,   // Number of tool invocations
}
```

When a limit is exceeded:
1. Current turn completes
2. `BudgetType::*Exhausted` event is emitted
3. Agent returns with partial results
4. Exit code 2 indicates budget exhaustion

## Performance Considerations

### Streaming

All providers use streaming responses. Text deltas are emitted as they arrive:

```rust
while let Some(event) = stream.next().await {
    match event {
        LlmEvent::TextDelta { delta } => {
            // Emit immediately for real-time display
            emit(AgentEvent::TextDelta { delta });
        }
        // ...
    }
}
```

### Parallel Tool Execution

When the LLM requests multiple tools, they execute in parallel:

```rust
// Multiple tool calls from LLM
let tool_calls = response.tool_calls;

// Execute all in parallel
let futures = tool_calls.iter().map(|tc| {
    dispatcher.dispatch(&tc.name, &tc.args)
});
let results = futures::future::join_all(futures).await;
```

### Session Checkpointing

Sessions are checkpointed after each turn to enable resumption:

```rust
// After each turn completes
store.save(&session).await?;
emit(AgentEvent::CheckpointSaved { session_id });
```

## Security Model

1. **API Key Isolation**: Keys are passed explicitly, never stored
2. **Tool Sandboxing**: Tools execute in MCP server processes
3. **Input Validation**: JSON Schema validation for tool arguments
4. **No Arbitrary Execution**: Meerkat doesn't execute code directly
5. **Sub-agent Isolation**: Sub-agents cannot have comms enabled (no network exposure)

See [DESIGN.md §15](../DESIGN.md#15-security-considerations) for full security considerations.

## Crate Dependencies

```
+----------------+
|  meerkat-cli   |
+-------+--------+
        |
        v
+-------+--------+
|    meerkat     |
+-------+--------+
        |
        +---------------------+---------------------+
        |                     |                     |
        v                     v                     v
+-------+--------+   +--------+-------+   +--------+-------+
| meerkat-client |   | meerkat-tools  |   | meerkat-store  |
+-------+--------+   +--------+-------+   +--------+-------+
        |                     |                     |
        |            +--------+-------+             |
        |            | meerkat-mcp-   |             |
        |            |    client      |             |
        |            +--------+-------+             |
        |                     |                     |
        +----------+----------+----------+----------+
                   |
                   v
           +-------+-------+
           | meerkat-core  |
           +---------------+
```

**Dependency Rules:**
1. `meerkat-core` depends on nothing in the workspace (only external crates)
2. All other crates depend on `meerkat-core` for types and traits
3. `meerkat-tools` depends on `meerkat-mcp-client` for MCP routing
4. `meerkat` (facade) re-exports from all crates
5. `meerkat-cli` is the top-level binary crate

**Key External Dependencies:**

| Dependency | Purpose |
|------------|---------|
| `tokio` | Async runtime |
| `reqwest` | HTTP client for LLM APIs |
| `serde` / `serde_json` | Serialization |
| `async-trait` | Async trait support |
| `uuid` | Session IDs (v7 for time-ordering) |
| `rmcp` | MCP protocol implementation |
| `jsonschema` | Tool argument validation |
| `thiserror` | Error type derivation |
| `tracing` | Structured logging |

## Configuration System

Configuration supports layered loading with precedence:

```
defaults < user config < project config < env vars < CLI args
```

**Configuration Files:**
- User: `~/.config/meerkat/config.toml`
- Project: `.rkat/config.toml` (searched up from cwd)
- Local: `rkat.toml` (current directory)

**Environment Variables:**
- `RKAT_MODEL` - Default model
- `RKAT_MAX_TOKENS` - Token budget
- `RKAT_MAX_DURATION` - Time budget (humantime format: "5m", "1h30m")
- `RKAT_STORAGE_DIR` - Storage directory
- `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `GEMINI_API_KEY` - Provider keys

**Example Config (TOML):**

```toml
[agent]
model = "claude-sonnet-4-20250514"
max_tokens_per_turn = 8192
temperature = 0.7

[budget]
max_tokens = 100000
max_duration = "30m"
max_tool_calls = 100

[retry]
max_retries = 3
initial_delay = "500ms"
max_delay = "30s"
multiplier = 2.0

[storage]
backend = "jsonl"
directory = "~/.local/share/rkat/sessions"
```

## Budget Pool for Sub-Agents

The budget system supports allocation to sub-agents:

```
+------------------+
|   BudgetPool     |
|  (1000 tokens)   |
+--------+---------+
         |
    +----+----+
    |         |
    v         v
+-------+ +-------+
|Agent A| |Agent B|
|300 tok| |400 tok|
+-------+ +-------+
    |         |
    v         v
 (used 200) (used 350)
    |         |
    +----+----+
         |
         v
   Reclaim unused:
   100 + 50 = 150
   Available: 450
```

**Fork Budget Policies:**
- `EqualSplit` - Divide remaining budget equally among branches
- `Proportional(weights)` - Divide according to weights
- `Fixed(tokens)` - Fixed allocation per branch

## Session Management

A `Session` maintains the complete conversation state (defined in `meerkat-core/src/session.rs`):

```rust
struct Session {
    version: u32,                    // Format version for migrations (current: 1)
    id: SessionId,                   // UUID v7 (time-ordered)
    messages: Vec<Message>,          // Full conversation history
    created_at: SystemTime,
    updated_at: SystemTime,
    metadata: Map<String, Value>,    // Arbitrary metadata
}
```

**Session Features:**
- Fork sessions at any message index for branching conversations
- Automatic token counting from assistant message usage
- Metadata for custom application state
- Serializable for persistence
- Version field for future migrations

**Session Operations:**
- `push(message)` - Add a message to the session
- `fork_at(index)` - Fork at specific message index
- `fork()` - Fork with full history
- `set_system_prompt(prompt)` - Set or replace system prompt
- `total_tokens()` - Calculate total tokens used
- `set_metadata(key, value)` - Store custom metadata
