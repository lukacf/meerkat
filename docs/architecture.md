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
                                    | (FactoryAgent-   |     FactoryAgentBuilder
                                    |  Builder, SDK)   |     build_ephemeral_service
                                    +--------+---------+
                                             |
   +----------+----------+----------+--------+--------+----------+----------+
   |          |          |          |        |        |          |          |
+--v---+ +---v----+ +---v---+ +----v---+ +--v---+ +--v---+ +----v---+ +----v-----+
|client| |session | | store | |  mcp   | |tools | |memory| | hooks  | |mcp-server|
+------+ +--------+ +-------+ +--------+ +------+ +------+ +--------+ +----------+
|Anthr.| |Ephemera| | JSONL | |McpRoutr| |Regist| |Hnsw  | |In-proc | |meerkat_  |
|OpenAI| |Persist.| | redb  | |Stdio   | |Valid. | |Simple| |Command | |run/resume|
|Gemini| |Compact.| |Memory | |HTTP/SSE| |      | |      | |HTTP    | |          |
+--+---+ +---+----+ +---+---+ +---+----+ +--+---+ +--+---+ +---+----+ +----+-----+
   |         |          |         |          |        |          |          |
   +---------+----------+---------+----------+--------+----------+----------+
                                     |
                             +-------v-------+
                             | meerkat-core  |  <- No I/O dependencies
                             | Traits, loop  |     Pure logic
                             | SessionError  |     SessionService trait
                             +---------------+
```

**SessionService routing:** All four surfaces (CLI, REST, MCP Server, JSON-RPC) route through
`SessionService` for the full session lifecycle. `AgentFactory::build_agent()` centralizes all
agent construction. Zero `AgentBuilder::new()` calls in surface crates.

### Detailed Component View

```
+-----------------------------------------------------------------------------+
|                              Access Patterns                                 |
+----------+-----------+-------------+---------------+----------+-------------+
|meerkat-  |meerkat-mcp|meerkat (SDK)|  meerkat-rest | meerkat- |             |
|cli       |(server)   |             |  (HTTP)       | rpc      |             |
|(headless)|(expose as |             |               | (stdio)  |             |
|          | MCP tools)|             |               |          |             |
+----+-----+-----+-----+------+------+-------+------+----+-----+             |
     |           |            |              |           |                    |
     +-----------+-----+------+--------------+-----------+                    |
                       |                                                      |
                       v                                                      |
          +---------------------------+                                       |
          |  SessionService (trait)   | <-- All surfaces route through this   |
          | create / turn / interrupt |                                       |
          | read / list / archive     |                                       |
          +------------+--------------+                                       |
                       |                                                      |
                       v                                                      |
          +---------------------------+                                       |
          |  AgentFactory             |                                       |
          |  ::build_agent()          | <-- Centralized agent construction    |
          +------------+--------------+                                       |
                       |                                                      |
+----------------------v------------------------------------------------------+
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
|  |  +-- compactor: Option<Arc<dyn Compactor>>          (context compact)  |  |
|  |  +-- sub_agent_manager: SubAgentManager             (child agents)     |  |
|  +------------------------------------------------------------------------+  |
+-----------------------------------------------------------------------------+
                                  |
          +-----------+-----------+-----------+-----------+
          |           |           |           |           |
          v           v           v           v           v
   +----------+ +----------+ +--------+ +----------+ +--------+
   | client   | |  tools   | | store  | | session  | | memory |
   | Anthropic| | Registry | | JSONL  | | Ephemeral| | Hnsw   |
   | OpenAI   | | Dispatch | | redb   | | Persist. | | Simple |
   | Gemini   | | Valid.   | | Memory | | Compact. | |        |
   +----------+ +----+-----+ +--------+ +----------+ +--------+
                     |
                     v
              +-----------+
              |meerkat-mcp|
              | McpRouter |
              | Stdio/HTTP|
              +-----------+
```

## Crate Structure

### meerkat-core

The heart of Meerkat. Contains:

- **Agent**: The main execution engine
- **Types**: `Message`, `Session`, `ToolCall`, `ToolResult`, `Usage`, `ToolCallView`, etc.
- **Trait contracts**: `AgentLlmClient`, `AgentToolDispatcher`, `AgentSessionStore`, `SessionService`, `Compactor`, `MemoryStore`
- **Service types**: `SessionError`, `CreateSessionRequest`, `StartTurnRequest`, `SessionView`, `SessionInfo`, `SessionUsage`
- **Budget**: Resource tracking and enforcement
- **Retry**: Exponential backoff with jitter
- **State Machine**: `LoopState` for agent lifecycle management
- **Compaction**: `Compactor` trait, `CompactionConfig`, compaction flow wired into agent loop
- **Sub-agents**: Fork and spawn operations
- **Hook contracts**: typed hook points, decisions, patches, invocation/outcome envelopes

### meerkat-hooks

Hook runtime adapters and the default deterministic hook engine:

- **In-process runtime**: register Rust handlers by name
- **Command runtime**: execute external processes via stdin/stdout JSON
- **HTTP runtime**: invoke remote hook endpoints
- **Deterministic execution**: foreground hooks execute in `(priority ASC, registration_index ASC)` order
- **Guardrail semantics**: first deny short-circuits remaining hooks; deny always wins over allow
- **Patch semantics**: patches from executed hooks are applied in execution order (last patch to same field wins)
- **Failure policy defaults**:
  - `observe` => fail-open
  - `guardrail` / `rewrite` => fail-closed
- **Background behavior**:
  - `pre_*` background hooks are observe-only
  - `post_*` background hooks publish `HookPatchEnvelope` events

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
- **RedbSessionStore**: redb-backed storage (feature-gated by `session-store`)
- **MemoryStore**: In-memory storage for testing
- **SessionStore trait**: Implement for custom backends

### meerkat-session

Session service orchestration:

- **EphemeralSessionService**: In-memory session lifecycle (always available)
- **PersistentSessionService**: Durable sessions backed by `RedbEventStore` (feature: `session-store`)
- **DefaultCompactor**: Context compaction implementation (feature: `session-compaction`)
- **EventStore trait / RedbEventStore**: Append-only event log for sessions
- **SessionProjector**: Materializes `.rkat/sessions/` files from events (derived, not canonical)

Feature gates: `session-store`, `session-compaction`.

### meerkat-memory

Semantic memory indexing:

- **HnswMemoryStore**: Production memory store using hnsw_rs + redb (feature: `memory-store-session`)
- **SimpleMemoryStore**: In-memory implementation for testing
- **MemoryStore trait** (defined in meerkat-core): indexing, retrieval, similarity search

### meerkat-tools

Tool management:

- **ToolRegistry**: Register and validate tool definitions
- **ToolDispatcher**: Route tool calls with timeout handling
- **Schema validation**: JSON Schema validation of tool arguments

### meerkat-mcp

MCP protocol client implementation:

- **McpConnection**: Manages stdio connection to MCP server
- **McpRouter**: Routes tool calls to appropriate MCP servers (implements `AgentToolDispatcher`)
- **Protocol handling**: Initialize, tools/list, tools/call

### meerkat-rpc

JSON-RPC 2.0 stdio server for IDE and desktop app integration:

- **SessionRuntime**: Stateful agent manager -- keeps agents alive between turns
- **RpcServer**: JSONL transport multiplexing requests and notifications via `tokio::select!`
- **MethodRouter**: Maps JSON-RPC methods to SessionRuntime/ConfigStore operations
- **Handlers**: Typed param parsing and response construction for each method group

Each session gets a dedicated tokio task with exclusive `Agent` ownership,
enabling `cancel(&mut self)` without mutex. Commands flow through channels;
events stream back as JSON-RPC notifications.

### meerkat (facade)

The main entry point. Re-exports types and provides:

- **AgentFactory**: Centralized agent construction pipeline shared across all surfaces
- **FactoryAgentBuilder**: Bridges `AgentFactory` into `SessionAgentBuilder` for `EphemeralSessionService`
- **FactoryAgent**: Wraps `DynAgent` implementing `SessionAgent`
- **build_ephemeral_service()**: Convenience constructor for `EphemeralSessionService<FactoryAgentBuilder>`
- **AgentBuildConfig**: Per-request configuration (model, system prompt, tools, `override_builtins`, `override_shell`)

The `FactoryAgentBuilder` pattern works as follows:

1. Surface crate stages an `AgentBuildConfig` into the `build_config_slot`
2. Surface calls `service.create_session(req)`
3. `EphemeralSessionService` calls `builder.build_agent(req, event_tx)`
4. `FactoryAgentBuilder` checks the slot, picks up the staged config, delegates to `AgentFactory::build_agent()`
5. If no config was staged, a minimal config is built from `CreateSessionRequest` fields

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
7. `turn_boundary` hooks run before the next state transition

Events that happen at turn boundaries:
- `TurnCompleted`
- `ToolResultReceived`
- `CheckpointSaved`
- `HookStarted` / `HookCompleted` / `HookFailed`
- `HookDenied`
- `HookRewriteApplied`
- `HookPatchPublished`

## Hook Insertion Points

The core loop executes hooks at these points:

- `run_started`
- `run_completed`
- `run_failed`
- `pre_llm_request`
- `post_llm_response`
- `pre_tool_execution`
- `post_tool_execution`
- `turn_boundary`

Synchronous (`foreground`) patches are applied in-loop.
Asynchronous (`background`) post-hook rewrites are event-only (`HookPatchPublished`) and do not retroactively mutate persisted session history.

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
    /// * `tools` - Available tool definitions (Arc for zero-copy sharing)
    /// * `max_tokens` - Maximum tokens to generate
    /// * `temperature` - Sampling temperature
    /// * `provider_params` - Provider-specific parameters (e.g., thinking config)
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
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
use std::sync::Arc;

pub struct CustomClient { /* ... */ }

#[async_trait]
impl AgentLlmClient for CustomClient {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
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
    fn tools(&self) -> Arc<[Arc<ToolDef>]>;

    /// Execute a tool call
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError>;
}
```

`ToolCallView` is a zero-allocation borrowed view: `{ id: &str, name: &str, args: &RawValue }`.
Use `call.parse_args::<T>()` to deserialize arguments into a concrete type.

**Example Implementation:**

```rust
use meerkat_core::{AgentToolDispatcher, ToolCallView, ToolDef, ToolResult};
use meerkat_core::error::ToolError;
use std::sync::Arc;

pub struct CustomDispatcher { /* ... */ }

#[async_trait]
impl AgentToolDispatcher for CustomDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![
            Arc::new(ToolDef {
                name: "my_tool".to_string(),
                description: "Does something useful".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "arg": { "type": "string" }
                    },
                    "required": ["arg"]
                }),
            })
        ].into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        match call.name {
            "my_tool" => {
                #[derive(serde::Deserialize)]
                struct Args { arg: String }

                let args: Args = call.parse_args()
                    .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
                Ok(ToolResult::success(call.id, format!("Processed: {}", args.arg)))
            }
            _ => Err(ToolError::not_found(call.name)),
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

Add MCP servers for tool discovery using `McpRouter` (defined in `meerkat-mcp/src/router.rs`):

```rust
use meerkat_mcp::{McpRouter, McpServerConfig};
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
|    meerkat     |  (facade: FactoryAgentBuilder, build_ephemeral_service)
+-------+--------+
        |
        +----------+----------+----------+----------+----------+
        |          |          |          |          |          |
        v          v          v          v          v          v
+-------+--+ +----+-----+ +--+-----+ +--+-----+ +--+------+ +--+------+
|  client  | |  tools   | | store  | |session | | memory  | |  hooks  |
+-------+--+ +----+-----+ +--+-----+ +--+-----+ +--+------+ +--+------+
        |         |           |          |          |          |
        |    +----+-----+    |          |          |          |
        |    | meerkat-  |   |          |          |          |
        |    |   mcp     |   |          |          |          |
        |    +----+------+   |          |          |          |
        |         |          |          |          |          |
        +---------+----+-----+----+-----+----+-----+----+-----+
                       |
                       v
               +-------+-------+
               | meerkat-core  |  (trait contracts, types, agent loop)
               +---------------+
```

**Crate Ownership:**

| Owner | Owns |
|-------|------|
| `meerkat-core` | Trait contracts (`SessionService`, `Compactor`, `MemoryStore`, `AgentLlmClient`, `AgentToolDispatcher`, `AgentSessionStore`), `SessionError`, agent loop, types |
| `meerkat-store` | `SessionStore` implementations (JSONL, redb, in-memory) |
| `meerkat-session` | Session orchestration (`EphemeralSessionService`, `PersistentSessionService`), `EventStore` |
| `meerkat-memory` | `HnswMemoryStore`, `SimpleMemoryStore` |
| `meerkat` (facade) | Feature wiring, re-exports, `FactoryAgentBuilder`, `FactoryAgent`, `build_ephemeral_service` |

**Dependency Rules:**
1. `meerkat-core` depends on nothing in the workspace (only external crates)
2. All other crates depend on `meerkat-core` for types and traits
3. `meerkat-tools` depends on `meerkat-mcp` for MCP routing
4. `meerkat` (facade) re-exports from all crates and provides the `FactoryAgentBuilder`
5. `meerkat-cli` is the top-level binary crate

**`.rkat/` as derived projection:** `.rkat/sessions/` files are materialized by `SessionProjector`
from the event store. They are derived output, not canonical state. Deleting them and replaying
from the event store produces identical content.

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
defaults < user config < project config < CLI args
```

**Configuration Files:**
- User: `~/.config/meerkat/config.toml`
- Project: `.rkat/config.toml` (searched up from cwd)
- Local: `rkat.toml` (current directory)

**Environment Variables (secrets only):**
- `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `GEMINI_API_KEY` - Provider keys

**Example Config (TOML):**

```toml
[agent]
model = "claude-opus-4-6"
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
