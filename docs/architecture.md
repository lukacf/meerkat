# Architecture Guide

This document explains Meerkat's internal architecture, design decisions, and extension points.

## Overview

Meerkat is designed as a **minimal, composable agent harness**. It provides the core execution loop for LLM-powered agents without opinions about prompts, tools, or output formatting.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Access Patterns                                 │
├───────────────┬───────────────┬─────────────────────┬───────────────────────┤
│   meerkat-cli    │  meerkat-mcp     │     meerkat (SDK)      │   meerkat-rest (opt)     │
│   (headless)  │  (server)     │                     │   (HTTP API)          │
└───────┬───────┴───────┬───────┴──────────┬──────────┴───────────┬───────────┘
        │               │                  │                      │
        └───────────────┴─────────┬────────┴──────────────────────┘
                                  │
                                  ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                              meerkat-core                                       │
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  Agent                                                                  │ │
│  │  ├── llm_client: Arc<dyn AgentLlmClient>     (provider abstraction)    │ │
│  │  ├── tool_dispatcher: Arc<dyn AgentToolDispatcher>  (tool routing)     │ │
│  │  ├── session_store: Arc<dyn AgentSessionStore>      (persistence)      │ │
│  │  ├── session: Session                               (conversation)     │ │
│  │  ├── budget: Budget                                 (resource limits)  │ │
│  │  └── retry_policy: RetryPolicy                      (error handling)   │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────────────┘
                                  │
                    ┌─────────────┼─────────────┐
                    │             │             │
                    ▼             ▼             ▼
            ┌─────────────┐ ┌─────────────┐ ┌─────────────┐
            │ meerkat-client │ │ meerkat-tools  │ │ meerkat-store  │
            │             │ │             │ │             │
            │ Anthropic   │ │ Registry    │ │ JsonlStore  │
            │ OpenAI      │ │ Dispatcher  │ │ MemoryStore │
            │ Gemini      │ │ Validation  │ │             │
            └─────────────┘ └──────┬──────┘ └─────────────┘
                                   │
                                   ▼
                           ┌─────────────────┐
                           │ meerkat-mcp-client │
                           │                 │
                           │ MCP Protocol    │
                           │ Tool Discovery  │
                           │ Tool Dispatch   │
                           └─────────────────┘
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

The agent executes a state machine loop:

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Agent Loop State Machine                     │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│                        ┌──────────────┐                             │
│                        │  CallingLlm  │◄────────────────────┐       │
│                        └──────┬───────┘                     │       │
│                               │                             │       │
│              ┌────────────────┼────────────────┐           │       │
│              │                │                │           │       │
│              ▼                ▼                ▼           │       │
│       ┌──────────┐    ┌─────────────┐   ┌───────────┐     │       │
│       │ Completed│    │WaitingForOps│   │ErrorRecov │     │       │
│       └──────────┘    └──────┬──────┘   └─────┬─────┘     │       │
│                              │                 │           │       │
│                              ▼                 │           │       │
│                       ┌─────────────┐         │           │       │
│                       │DrainingEvts │─────────┼───────────┘       │
│                       └──────┬──────┘         │                   │
│                              │                 │                   │
│                              ▼                 ▼                   │
│                        ┌──────────┐     ┌──────────┐              │
│                        │ Completed│     │ Completed│              │
│                        └──────────┘     └──────────┘              │
│                                                                      │
│  Any State ──► Cancelling ──► Completed                            │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### States

| State | Description |
|-------|-------------|
| `CallingLlm` | Sending request to LLM, streaming response |
| `WaitingForOps` | Waiting for tool operations to complete |
| `DrainingEvents` | Processing buffered events at turn boundary |
| `Cancelling` | Gracefully stopping after cancel signal |
| `ErrorRecovery` | Attempting recovery from transient error |
| `Completed` | Agent has finished (success or failure) |

### Turn Boundaries

Turn boundaries are critical moments where:

1. Tool results are injected into the session
2. Steering messages (from sub-agents) are applied
3. Budget is checked
4. Session is checkpointed

Events that happen at turn boundaries:
- `TurnCompleted`
- `ToolResultReceived`
- `CheckpointSaved`

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

Implement `AgentLlmClient`:

```rust
#[async_trait]
pub trait AgentLlmClient: Send + Sync {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
    ) -> Result<LlmStreamResult, AgentError>;

    fn provider(&self) -> &'static str;
}
```

### Custom Tool Dispatcher

Implement `AgentToolDispatcher`:

```rust
#[async_trait]
pub trait AgentToolDispatcher: Send + Sync {
    fn tools(&self) -> Vec<ToolDef>;
    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String>;
}
```

### Custom Session Store

Implement `AgentSessionStore`:

```rust
#[async_trait]
pub trait AgentSessionStore: Send + Sync {
    async fn save(&self, session: &Session) -> Result<(), AgentError>;
    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError>;
}
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

See [DESIGN.md §15](../DESIGN.md#15-security-considerations) for full security considerations.
