# Meerkat Design Document

**Rust Agentic Interface Kit**

Version: 0.1.0-draft
Last Updated: 2026-01-20

---

## Table of Contents

1. [Overview](#1-overview)
2. [Goals and Non-Goals](#2-goals-and-non-goals)
3. [Architecture](#3-architecture)
4. [Crate Structure](#4-crate-structure)
5. [Core Types](#5-core-types)
6. [Agent Loop](#6-agent-loop)
7. [LLM Client Abstraction](#7-llm-client-abstraction)
8. [Tool System](#8-tool-system)
9. [Session Management](#9-session-management)
10. [Budget Enforcement](#10-budget-enforcement)
11. [Configuration](#11-configuration)
12. [Access Patterns](#12-access-patterns)
13. [Error Taxonomy](#13-error-taxonomy)
14. [Observability](#14-observability)
15. [Security Considerations](#15-security-considerations)
16. [Design Decisions](#16-design-decisions)
17. [Prior Art](#17-prior-art)
18. [Async Operations & Sub-Agents](#18-async-operations--sub-agents)
19. [Future Considerations](#19-future-considerations)

---

## 1. Overview

Meerkat is a minimal, high-performance agent harness written in Rust. It provides the core execution loop for LLM-powered agents without the overhead of IDE integration, interactive prompts, or complex output formatting.

### What Meerkat Is

- A library for building and running LLM agents
- A headless executor for agentic workloads
- A bridge between LLM providers and MCP tool servers
- Embeddable in other applications via SDK, CLI, MCP, or REST

### What Meerkat Is Not

- An interactive CLI tool (use Claude Code, Codex CLI, or Gemini CLI for that)
- An IDE extension or editor integration
- A prompt engineering framework
- A vector database or RAG system

### Core Responsibilities

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

---

## 2. Goals and Non-Goals

### Goals

| Goal | Rationale |
|------|-----------|
| **Minimal footprint** | Single binary, fast startup, low memory |
| **Embeddable** | Use as a library without pulling in server dependencies |
| **Provider-agnostic** | Support Anthropic, OpenAI, Gemini, and others |
| **MCP-native** | Tools come from MCP servers, not built-in implementations |
| **Correct first** | Handle edge cases (parallel tools, streaming, retries) properly |
| **Observable** | Structured events for monitoring and debugging |
| **Resumable** | Checkpoint and resume long-running sessions |

### Non-Goals

| Non-Goal | Rationale |
|----------|-----------|
| Interactive TUI | Other tools do this well; Meerkat is headless |
| Built-in tools | MCP provides tool abstraction; no reason to duplicate |
| Prompt templates | Keep core focused; users bring their own prompts |
| Complex multi-agent orchestration | Sub-agents supported (§18); higher-level patterns are a layer above |
| File watching / hot reload | Batch execution model, not continuous |

---

## 3. Architecture

### High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Access Patterns                                 │
├───────────────┬───────────────┬─────────────────────┬───────────────────────┤
│   meerkat-cli    │  meerkat-mcp     │     meerkat (SDK)      │   meerkat-rest (opt)     │
│   (headless)  │  (server)     │                     │   (HTTP API)          │
│               │               │                     │                       │
│  $ rkat run   │  Expose as    │  Agent::new(cfg)    │   POST /sessions      │
│    "prompt"   │  MCP tool     │    .run(prompt)     │   POST /sessions/:id  │
└───────┬───────┴───────┬───────┴──────────┬──────────┴───────────┬───────────┘
        │               │                  │                      │
        └───────────────┴─────────┬────────┴──────────────────────┘
                                  │
                                  ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                              meerkat-core                                       │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │  Agent                                                                  │ │
│  │  ├── client: Arc<dyn LlmClient>        (provider abstraction)          │ │
│  │  ├── dispatcher: ToolDispatcher        (validation + timeout + MCP)    │ │
│  │  ├── session: Session                  (conversation history)          │ │
│  │  ├── store: Arc<dyn SessionStore>      (persistence)                   │ │
│  │  ├── budget: Budget                    (resource limits)               │ │
│  │  ├── retry_policy: RetryPolicy         (backoff config)                │ │
│  │  └── config: AgentConfig               (behavior settings)             │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌──────────────────┐   │
│  │   Session   │  │   Budget    │  │ RetryPolicy │  │  ToolDispatcher  │   │
│  │             │  │             │  │             │  │                  │   │
│  │ messages[]  │  │ max_tokens  │  │ max_retries │  │ registry         │   │
│  │ metadata    │  │ max_time    │  │ delays      │  │ router           │   │
│  │ created_at  │  │ max_calls   │  │ multiplier  │  │ default_timeout  │   │
│  └─────────────┘  └─────────────┘  └─────────────┘  └──────────────────┘   │
│                                                                              │
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
            │ Gemini      │ │ Validation  │ │ (trait)     │
            └─────────────┘ └──────┬──────┘ └─────────────┘
                                   │
                                   ▼
                           ┌─────────────┐
                           │ meerkat-mcp    │
                           │   -client   │
                           │             │
                           │ McpRouter   │
                           │ Connection  │
                           └─────────────┘
```

### Data Flow

```
User Prompt
     │
     ▼
┌─────────────────────────────────────────────────────────────────┐
│                         Agent Loop                               │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │  loop {                                                     │ │
│  │      budget.check()?;                                       │ │
│  │                                                             │ │
│  │      let response = retry_policy.execute(||                 │ │
│  │          client.stream(session.messages(), tools)           │ │
│  │      ).await?;                                              │ │
│  │                                                             │ │
│  │      session.push(response.to_assistant_message());         │ │
│  │                                                             │ │
│  │      match response.stop_reason {                           │ │
│  │          EndTurn => break Ok(response.text),                │ │
│  │          ToolUse => {                                       │ │
│  │              let results = dispatcher                       │ │
│  │                  .dispatch_parallel(&response.tool_calls)   │ │
│  │                  .await;                                    │ │
│  │              session.push(ToolResults(results));            │ │
│  │              budget.record_calls(results.len());            │ │
│  │          }                                                  │ │
│  │          MaxTokens => return Err(MaxTokensReached),         │ │
│  │          ContentFilter => return Err(ContentFiltered),      │ │
│  │      }                                                      │ │
│  │                                                             │ │
│  │      store.save_if_threshold(&session).await?;              │ │
│  │  }                                                          │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
     │
     ▼
RunResult { text, usage, session_id }
```

---

## 4. Crate Structure

### Workspace Layout

```
meerkat/
├── Cargo.toml                    # Workspace root
├── DESIGN.md                     # This document
├── README.md                     # User-facing documentation
│
├── meerkat-core/                    # Core agent logic (no I/O deps)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── agent.rs              # Agent struct and run loop
│       ├── session.rs            # Conversation history
│       ├── budget.rs             # Resource enforcement
│       ├── retry.rs              # Retry policy
│       ├── types.rs              # Shared types (Message, ToolCall, etc.)
│       └── event.rs              # Agent events for streaming
│
├── meerkat-client/                  # LLM provider abstraction
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                # LlmClient trait
│       ├── types.rs              # LlmEvent, LlmError, Request
│       ├── anthropic.rs          # Anthropic Messages API
│       ├── openai.rs             # OpenAI Chat Completions API
│       └── gemini.rs             # Google Gemini API
│
├── meerkat-tools/                   # Tool validation and dispatch
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── registry.rs           # Tool schema registry + validation
│       ├── dispatcher.rs         # Parallel dispatch with timeouts
│       └── error.rs              # Tool-specific errors
│
├── meerkat-store/                   # Session persistence
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                # SessionStore trait
│       ├── jsonl.rs              # JSONL file store
│       └── memory.rs             # In-memory store (tests)
│
├── meerkat-mcp-client/              # MCP client (connect to tool servers)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── router.rs             # Route tool calls to MCP servers
│       ├── connection.rs         # MCP connection management
│       └── discovery.rs          # Tool discovery from servers
│
├── meerkat-mcp-server/              # MCP server (expose Meerkat as tool)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       └── server.rs             # meerkat:run MCP tool
│
├── meerkat-cli/                     # Headless CLI
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── commands/
│       │   ├── run.rs            # rkat run "prompt"
│       │   ├── resume.rs         # rkat resume <session-id>
│       │   └── list.rs           # rkat sessions list
│       └── output.rs             # JSON/streaming output
│
├── meerkat-rest/                    # Optional REST API
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── routes.rs
│       └── handlers.rs
│
└── meerkat/                         # SDK facade crate
    ├── Cargo.toml
    └── src/
        └── lib.rs                # pub use re-exports
```

### Dependency Graph

```
                    ┌─────────────────────────────────────────┐
                    │            Application Layer             │
                    ├──────────┬──────────┬──────────┬────────┤
                    │ meerkat-cli │ meerkat-mcp │ meerkat-rest│  user  │
                    │          │  -server │ (opt)    │  code  │
                    └────┬─────┴────┬─────┴────┬─────┴───┬────┘
                         │          │          │         │
                         └──────────┴────┬─────┴─────────┘
                                         │
                                         ▼
                    ┌─────────────────────────────────────────┐
                    │               meerkat (SDK)                 │
                    │  pub use meerkat_core::*;                   │
                    │  pub use meerkat_client::*;                 │
                    │  pub use meerkat_tools::*;                  │
                    │  pub use meerkat_store::*;                  │
                    │  pub use meerkat_mcp_client::*;             │
                    └────────────────────┬────────────────────┘
                                         │
              ┌──────────────────────────┼──────────────────────────┐
              │                          │                          │
              ▼                          ▼                          ▼
┌─────────────────────────┐ ┌─────────────────────────┐ ┌─────────────────────┐
│       meerkat-core         │ │      meerkat-client        │ │      meerkat-store     │
│                         │ │                         │ │                     │
│ - Agent                 │ │ - LlmClient trait       │ │ - SessionStore trait│
│ - Session               │ │ - AnthropicClient       │ │ - JsonlStore        │
│ - Budget                │ │ - OpenAiClient          │ │ - MemoryStore       │
│ - RetryPolicy           │ │ - GeminiClient          │ │                     │
│ - Types                 │ │ - LlmEvent, LlmError    │ │                     │
└────────────┬────────────┘ └─────────────────────────┘ └─────────────────────┘
             │
             ▼
┌─────────────────────────┐
│       meerkat-tools        │
│                         │
│ - ToolRegistry          │
│ - ToolDispatcher        │
│ - Validation            │
└────────────┬────────────┘
             │
             ▼
┌─────────────────────────┐
│     meerkat-mcp-client     │
│                         │
│ - McpRouter             │
│ - Connection mgmt       │
│ - Tool discovery        │
└─────────────────────────┘
             │
             ▼
┌─────────────────────────┐
│    rmcp (external)      │
│                         │
│ MCP protocol impl       │
└─────────────────────────┘
```

### Feature Flags

```toml
# meerkat/Cargo.toml
[features]
default = ["anthropic", "jsonl-store"]

# LLM providers
anthropic = ["meerkat-client/anthropic"]
openai = ["meerkat-client/openai"]
gemini = ["meerkat-client/gemini"]
all-providers = ["anthropic", "openai", "gemini"]

# Storage backends
jsonl-store = ["meerkat-store/jsonl"]
memory-store = ["meerkat-store/memory"]

# Optional components
rest = ["meerkat-rest"]
mcp-server = ["meerkat-mcp-server"]
```

---

## 5. Core Types

### Message Types

```rust
// meerkat-core/src/types.rs

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::SystemTime;
use uuid::Uuid;

/// Unique identifier for a session
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

/// A message in the conversation history
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    /// System prompt (injected at start)
    System(SystemMessage),
    /// User input
    User(UserMessage),
    /// Assistant response (may include tool calls)
    Assistant(AssistantMessage),
    /// Results from tool execution
    #[serde(rename = "tool_results")]
    ToolResults { results: Vec<ToolResult> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMessage {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    /// Text content (may be empty if only tool calls)
    pub content: String,
    /// Tool calls requested by the model
    pub tool_calls: Vec<ToolCall>,
    /// How the turn ended
    pub stop_reason: StopReason,
    /// Token usage for this turn
    pub usage: Usage,
}

/// A tool call requested by the model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique ID for this tool use (from the model)
    pub id: String,
    /// Name of the tool to invoke
    pub name: String,
    /// Arguments as JSON
    pub args: Value,
}

/// Result of executing a tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Matches the tool_call.id
    pub tool_use_id: String,
    /// Content returned by the tool
    pub content: String,
    /// Whether this is an error result
    pub is_error: bool,
}

/// Why the model stopped generating
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Model finished naturally
    EndTurn,
    /// Model wants to call tools
    ToolUse,
    /// Hit max output tokens
    MaxTokens,
    /// Hit a stop sequence
    StopSequence,
    /// Blocked by content filter
    ContentFilter,
    /// Request was cancelled
    Cancelled,
}

/// Token usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
}

impl Usage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Accumulate usage from another
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        if let Some(c) = other.cache_creation_tokens {
            *self.cache_creation_tokens.get_or_insert(0) += c;
        }
        if let Some(c) = other.cache_read_tokens {
            *self.cache_read_tokens.get_or_insert(0) += c;
        }
    }
}

/// Tool definition for the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}
```

### Agent Events

```rust
// meerkat-core/src/event.rs

use crate::types::*;
use std::path::PathBuf;

/// Events emitted during agent execution
///
/// These events form the streaming API for consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    // === Session Lifecycle ===

    /// Agent run started
    RunStarted {
        session_id: SessionId,
        prompt: String,
    },

    /// Agent run completed successfully
    RunCompleted {
        session_id: SessionId,
        result: String,
        usage: Usage,
    },

    /// Agent run failed
    RunFailed {
        session_id: SessionId,
        error: String,
    },

    // === LLM Interaction ===

    /// New turn started (calling LLM)
    TurnStarted {
        turn_number: u32,
    },

    /// Streaming text from the model
    TextDelta {
        delta: String,
    },

    /// Text generation complete for this turn
    TextComplete {
        content: String,
    },

    /// Model requested a tool call
    ToolCallRequested {
        id: String,
        name: String,
        args: Value,
    },

    /// Tool result received (injected into conversation)
    ToolResultReceived {
        id: String,
        name: String,
        is_error: bool,
    },

    /// Turn completed
    TurnCompleted {
        stop_reason: StopReason,
        usage: Usage,
    },

    // === Tool Execution ===

    /// Starting tool execution
    ToolExecutionStarted {
        id: String,
        name: String,
    },

    /// Tool execution completed
    ToolExecutionCompleted {
        id: String,
        name: String,
        result: String,
        is_error: bool,
        duration_ms: u64,
    },

    /// Tool execution timed out
    ToolExecutionTimedOut {
        id: String,
        name: String,
        timeout_ms: u64,
    },

    // === Budget & Checkpointing ===

    /// Budget warning (approaching limits)
    BudgetWarning {
        budget_type: BudgetType,
        used: u64,
        limit: u64,
        percent: f32,
    },

    /// Session checkpoint saved
    CheckpointSaved {
        session_id: SessionId,
        path: Option<PathBuf>,
    },

    // === Retry Events ===

    /// Retrying after error
    Retrying {
        attempt: u32,
        max_attempts: u32,
        error: String,
        delay_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetType {
    Tokens,
    Time,
    ToolCalls,
}
```

---

## 6. Agent Loop

### Agent Structure

```rust
// meerkat-core/src/agent.rs

use crate::{
    event::AgentEvent,
    session::Session,
    budget::Budget,
    retry::RetryPolicy,
    types::*,
};
use meerkat_client::LlmClient;
use meerkat_tools::ToolDispatcher;
use meerkat_store::SessionStore;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Configuration for agent behavior
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// System prompt to prepend
    pub system_prompt: Option<String>,
    /// Model identifier (provider-specific)
    pub model: String,
    /// Maximum tokens to generate per turn
    pub max_tokens_per_turn: u32,
    /// Temperature for sampling
    pub temperature: Option<f32>,
    /// Checkpoint after this many turns
    pub checkpoint_interval: Option<u32>,
    /// Warning threshold for budget (0.0-1.0)
    pub budget_warning_threshold: f32,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: None,
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens_per_turn: 8192,
            temperature: None,
            checkpoint_interval: Some(5),
            budget_warning_threshold: 0.8,
        }
    }
}

/// Result of a successful agent run
#[derive(Debug, Clone)]
pub struct RunResult {
    /// Final text response
    pub text: String,
    /// Session ID for resumption
    pub session_id: SessionId,
    /// Total token usage
    pub usage: Usage,
    /// Number of turns taken
    pub turns: u32,
    /// Number of tool calls made
    pub tool_calls: u32,
}

/// The main agent executor
pub struct Agent {
    client: Arc<dyn LlmClient>,
    dispatcher: ToolDispatcher,
    store: Arc<dyn SessionStore>,
    budget: Budget,
    retry_policy: RetryPolicy,
    config: AgentConfig,
}

impl Agent {
    /// Create a new agent with the given components
    pub fn new(
        client: Arc<dyn LlmClient>,
        dispatcher: ToolDispatcher,
        store: Arc<dyn SessionStore>,
        budget: Budget,
        retry_policy: RetryPolicy,
        config: AgentConfig,
    ) -> Self {
        Self {
            client,
            dispatcher,
            store,
            budget,
            retry_policy,
            config,
        }
    }

    /// Run the agent to completion, returning final result
    pub async fn run(&mut self, prompt: &str) -> Result<RunResult, AgentError> {
        let (tx, _rx) = mpsc::channel(128);
        self.run_with_events(prompt, tx).await
    }

    /// Run the agent with event streaming
    pub async fn run_with_events(
        &mut self,
        prompt: &str,
        events: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        let mut session = Session::new();

        // Add system prompt if configured
        if let Some(ref system) = self.config.system_prompt {
            session.push(Message::System(SystemMessage {
                content: system.clone(),
            }));
        }

        // Add user prompt
        session.push(Message::User(UserMessage {
            content: prompt.to_string(),
        }));

        let _ = events.send(AgentEvent::RunStarted {
            session_id: session.id().clone(),
            prompt: prompt.to_string(),
        }).await;

        let result = self.run_loop(&mut session, &events).await;

        match &result {
            Ok(r) => {
                let _ = events.send(AgentEvent::RunCompleted {
                    session_id: session.id().clone(),
                    result: r.text.clone(),
                    usage: r.usage.clone(),
                }).await;
            }
            Err(e) => {
                let _ = events.send(AgentEvent::RunFailed {
                    session_id: session.id().clone(),
                    error: e.to_string(),
                }).await;
            }
        }

        // Final save
        self.store.save(&session).await?;

        result
    }

    /// Resume a previous session
    pub async fn resume(
        &mut self,
        session_id: &SessionId,
        prompt: &str,
    ) -> Result<RunResult, AgentError> {
        let mut session = self.store.load(session_id).await?
            .ok_or(AgentError::SessionNotFound(session_id.clone()))?;

        session.push(Message::User(UserMessage {
            content: prompt.to_string(),
        }));

        let (tx, _rx) = mpsc::channel(128);
        self.run_loop(&mut session, &tx).await
    }

    /// The core agent loop
    async fn run_loop(
        &mut self,
        session: &mut Session,
        events: &mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        let mut turn_number = 0u32;
        let mut total_usage = Usage::default();
        let mut total_tool_calls = 0u32;

        loop {
            // Check budget before each turn
            self.check_budget_with_warning(events).await?;

            turn_number += 1;
            let _ = events.send(AgentEvent::TurnStarted { turn_number }).await;

            // Build request
            let request = self.build_request(session);

            // Call LLM with retry
            let response = self.call_llm_with_retry(&request, events).await?;

            // Accumulate usage
            total_usage.add(&response.usage);
            self.budget.record_tokens(response.usage.total_tokens());

            // Stream text deltas were already sent; send complete
            if !response.content.is_empty() {
                let _ = events.send(AgentEvent::TextComplete {
                    content: response.content.clone(),
                }).await;
            }

            // Record assistant message
            session.push(Message::Assistant(AssistantMessage {
                content: response.content.clone(),
                tool_calls: response.tool_calls.clone(),
                stop_reason: response.stop_reason,
                usage: response.usage.clone(),
            }));

            let _ = events.send(AgentEvent::TurnCompleted {
                stop_reason: response.stop_reason,
                usage: response.usage.clone(),
            }).await;

            // Handle stop reason
            match response.stop_reason {
                StopReason::EndTurn => {
                    return Ok(RunResult {
                        text: response.content,
                        session_id: session.id().clone(),
                        usage: total_usage,
                        turns: turn_number,
                        tool_calls: total_tool_calls,
                    });
                }

                StopReason::ToolUse => {
                    let results = self.dispatch_tools(&response.tool_calls, events).await;
                    total_tool_calls += results.len() as u32;
                    self.budget.record_calls(results.len());
                    session.push(Message::ToolResults(results));
                }

                StopReason::MaxTokens => {
                    return Err(AgentError::MaxTokensReached {
                        turn: turn_number,
                        partial: response.content,
                    });
                }

                StopReason::ContentFilter => {
                    return Err(AgentError::ContentFiltered {
                        turn: turn_number,
                    });
                }

                StopReason::StopSequence => {
                    return Ok(RunResult {
                        text: response.content,
                        session_id: session.id().clone(),
                        usage: total_usage,
                        turns: turn_number,
                        tool_calls: total_tool_calls,
                    });
                }

                StopReason::Cancelled => {
                    return Err(AgentError::Cancelled);
                }
            }

            // Periodic checkpoint
            if let Some(interval) = self.config.checkpoint_interval {
                if turn_number % interval == 0 {
                    self.store.save(session).await?;
                    let _ = events.send(AgentEvent::CheckpointSaved {
                        session_id: session.id().clone(),
                        path: None, // Store impl knows the path
                    }).await;
                }
            }
        }
    }

    /// Call LLM with retry policy
    async fn call_llm_with_retry(
        &self,
        request: &LlmRequest,
        events: &mpsc::Sender<AgentEvent>,
    ) -> Result<LlmResponse, AgentError> {
        self.retry_policy.execute(|| async {
            self.call_llm_streaming(request, events).await
        }).await.map_err(AgentError::LlmError)
    }

    /// Call LLM and collect streaming response
    async fn call_llm_streaming(
        &self,
        request: &LlmRequest,
        events: &mpsc::Sender<AgentEvent>,
    ) -> Result<LlmResponse, LlmError> {
        use futures::StreamExt;

        let mut stream = self.client.stream(request);
        let mut response = LlmResponse::default();
        let mut tool_buffers: HashMap<String, ToolCallBuffer> = HashMap::new();

        while let Some(event) = stream.next().await {
            match event? {
                LlmEvent::TextDelta(delta) => {
                    response.content.push_str(&delta);
                    let _ = events.send(AgentEvent::TextDelta { delta }).await;
                }

                LlmEvent::ToolCallDelta { id, name, args_delta } => {
                    let buffer = tool_buffers.entry(id.clone()).or_insert_with(|| {
                        ToolCallBuffer {
                            id: id.clone(),
                            name: name.clone(),
                            args_json: String::new(),
                        }
                    });
                    if let Some(n) = name {
                        buffer.name = Some(n);
                    }
                    buffer.args_json.push_str(&args_delta);
                }

                LlmEvent::ToolCallComplete { id, name, args } => {
                    let _ = events.send(AgentEvent::ToolCallRequested {
                        id: id.clone(),
                        name: name.clone(),
                        args: args.clone(),
                    }).await;

                    response.tool_calls.push(ToolCall { id, name, args });
                }

                LlmEvent::Usage(usage) => {
                    response.usage = usage;
                }

                LlmEvent::Done(stop_reason) => {
                    response.stop_reason = stop_reason;
                }
            }
        }

        Ok(response)
    }

    /// Dispatch tool calls in parallel with timeouts
    async fn dispatch_tools(
        &self,
        calls: &[ToolCall],
        events: &mpsc::Sender<AgentEvent>,
    ) -> Vec<ToolResult> {
        let futures = calls.iter().map(|call| {
            let events = events.clone();
            async move {
                let _ = events.send(AgentEvent::ToolExecutionStarted {
                    id: call.id.clone(),
                    name: call.name.clone(),
                }).await;

                let start = std::time::Instant::now();
                let result = self.dispatcher.dispatch_one(call).await;
                let duration_ms = start.elapsed().as_millis() as u64;

                match &result {
                    r if r.is_error && r.content.contains("timed out") => {
                        let _ = events.send(AgentEvent::ToolExecutionTimedOut {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            timeout_ms: duration_ms,
                        }).await;
                    }
                    r => {
                        let _ = events.send(AgentEvent::ToolExecutionCompleted {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            result: r.content.clone(),
                            is_error: r.is_error,
                            duration_ms,
                        }).await;
                    }
                }

                result
            }
        });

        futures::future::join_all(futures).await
    }

    /// Check budget and emit warning if approaching limit
    async fn check_budget_with_warning(
        &self,
        events: &mpsc::Sender<AgentEvent>,
    ) -> Result<(), AgentError> {
        // Check for hard limits
        self.budget.check()?;

        // Check for warnings
        let threshold = self.config.budget_warning_threshold;

        if let Some((used, limit)) = self.budget.token_usage() {
            let percent = used as f32 / limit as f32;
            if percent >= threshold {
                let _ = events.send(AgentEvent::BudgetWarning {
                    budget_type: BudgetType::Tokens,
                    used,
                    limit,
                    percent,
                }).await;
            }
        }

        if let Some((used, limit)) = self.budget.time_usage() {
            let percent = used as f32 / limit as f32;
            if percent >= threshold {
                let _ = events.send(AgentEvent::BudgetWarning {
                    budget_type: BudgetType::Time,
                    used,
                    limit,
                    percent,
                }).await;
            }
        }

        if let Some((used, limit)) = self.budget.call_usage() {
            let percent = used as f32 / limit as f32;
            if percent >= threshold {
                let _ = events.send(AgentEvent::BudgetWarning {
                    budget_type: BudgetType::ToolCalls,
                    used: used as u64,
                    limit: limit as u64,
                    percent,
                }).await;
            }
        }

        Ok(())
    }

    fn build_request(&self, session: &Session) -> LlmRequest {
        LlmRequest {
            model: self.config.model.clone(),
            messages: session.messages().to_vec(),
            tools: self.dispatcher.tool_defs(),
            max_tokens: self.config.max_tokens_per_turn,
            temperature: self.config.temperature,
        }
    }
}
```

### Agent Errors

```rust
// meerkat-core/src/error.rs

use crate::types::SessionId;
use meerkat_client::LlmError;
use meerkat_store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("LLM error: {0}")]
    LlmError(#[from] LlmError),

    #[error("Storage error: {0}")]
    StoreError(#[from] StoreError),

    #[error("Session not found: {0:?}")]
    SessionNotFound(SessionId),

    #[error("Token budget exceeded: used {used}, limit {limit}")]
    TokenBudgetExceeded { used: u64, limit: u64 },

    #[error("Time budget exceeded: {elapsed_secs}s > {limit_secs}s")]
    TimeBudgetExceeded { elapsed_secs: u64, limit_secs: u64 },

    #[error("Tool call budget exceeded: {count} calls > {limit} limit")]
    ToolCallBudgetExceeded { count: usize, limit: usize },

    #[error("Max tokens reached on turn {turn}, partial output: {partial}")]
    MaxTokensReached { turn: u32, partial: String },

    #[error("Content filtered on turn {turn}")]
    ContentFiltered { turn: u32 },

    #[error("Run was cancelled")]
    Cancelled,
}
```

---

## 7. LLM Client Abstraction

### Trait Definition

```rust
// meerkat-client/src/lib.rs

use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

/// Abstraction over LLM providers
///
/// Each provider implementation normalizes its streaming response
/// to the common `LlmEvent` type, hiding provider-specific quirks.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Stream a completion request
    ///
    /// Returns a stream of normalized events. The stream completes
    /// when the model finishes (either with EndTurn or ToolUse).
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>;

    /// Get the provider name (for logging/debugging)
    fn provider(&self) -> &'static str;

    /// Check if the client is healthy/connected
    async fn health_check(&self) -> Result<(), LlmError>;
}

/// Request to the LLM
#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub stop_sequences: Option<Vec<String>>,
}

/// Normalized streaming events from LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LlmEvent {
    /// Incremental text output
    TextDelta { delta: String },

    /// Incremental tool call (may arrive in pieces)
    ToolCallDelta {
        id: String,
        name: Option<String>,  // Only present in first delta
        args_delta: String,
    },

    /// Complete tool call (after buffering deltas)
    ToolCallComplete {
        id: String,
        name: String,
        args: serde_json::Value,
    },

    /// Token usage update
    UsageUpdate { usage: Usage },

    /// Stream completed with stop reason
    Done { stop_reason: StopReason },
}
```

### Error Taxonomy

```rust
// meerkat-client/src/error.rs

use std::time::Duration;

/// Errors from LLM providers
///
/// Categorized by whether they're retryable.
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
pub enum LlmError {
    // === Retryable Errors ===

    #[error("Rate limited, retry after {retry_after_ms:?}ms")]
    RateLimited { retry_after_ms: Option<u64> },

    #[error("Server overloaded (503)")]
    ServerOverloaded,

    #[error("Network timeout after {duration_ms}ms")]
    NetworkTimeout { duration_ms: u64 },

    #[error("Connection reset")]
    ConnectionReset,

    #[error("Server error: {status} - {message}")]
    ServerError { status: u16, message: String },

    // === Non-Retryable Errors ===

    #[error("Invalid request: {message}")]
    InvalidRequest { message: String },

    #[error("Authentication failed: {message}")]
    AuthenticationFailed { message: String },

    #[error("Content filtered: {reason}")]
    ContentFiltered { reason: String },

    #[error("Context length exceeded: {requested} > {max}")]
    ContextLengthExceeded { max: usize, requested: usize },

    #[error("Model not found: {model}")]
    ModelNotFound { model: String },

    #[error("Invalid API key")]
    InvalidApiKey,

    // === Streaming Errors ===

    #[error("Stream parsing error: {message}")]
    StreamParseError { message: String },

    #[error("Incomplete response: {message}")]
    IncompleteResponse { message: String },

    // === Unknown ===

    #[error("Unknown error: {message}")]
    Unknown { message: String },
}

impl LlmError {
    /// Whether this error should trigger a retry
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::ServerOverloaded
                | Self::NetworkTimeout { .. }
                | Self::ConnectionReset
                | Self::ServerError { status, .. } if *status >= 500
        )
    }

    /// Get retry-after hint if available
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after_ms } => {
                retry_after_ms.map(Duration::from_millis)
            }
            _ => None,
        }
    }
}
```

### Anthropic Client Implementation (Sketch)

```rust
// meerkat-client/src/anthropic.rs

use crate::{LlmClient, LlmEvent, LlmError, LlmRequest};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use std::pin::Pin;

pub struct AnthropicClient {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl AnthropicClient {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://api.anthropic.com".to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        Box::pin(async_stream::try_stream! {
            let body = self.build_request_body(request);

            let response = self.http
                .post(format!("{}/v1/messages", self.base_url))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| LlmError::NetworkTimeout {
                    duration: std::time::Duration::from_secs(30)
                })?;

            // Check for error responses
            if !response.status().is_success() {
                let status = response.status().as_u16();
                let text = response.text().await.unwrap_or_default();

                Err(match status {
                    401 => LlmError::AuthenticationFailed { message: text },
                    429 => LlmError::RateLimited { retry_after: None },
                    503 => LlmError::ServerOverloaded,
                    _ => LlmError::ServerError { status, message: text },
                })?;
            }

            // Stream SSE events
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut tool_buffers: HashMap<String, ToolCallBuffer> = HashMap::new();

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| LlmError::ConnectionReset)?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Parse SSE events from buffer
                while let Some(event) = self.parse_sse_event(&mut buffer) {
                    match event.event_type.as_str() {
                        "content_block_delta" => {
                            if let Some(delta) = event.data.get("delta") {
                                if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                                    yield LlmEvent::TextDelta(text.to_string());
                                }
                                if let Some(partial_json) = delta.get("partial_json").and_then(|t| t.as_str()) {
                                    // This is a tool call argument delta
                                    if let Some(index) = event.data.get("index").and_then(|i| i.as_u64()) {
                                        let id = format!("tool_{}", index);
                                        yield LlmEvent::ToolCallDelta {
                                            id,
                                            name: None,
                                            args_delta: partial_json.to_string(),
                                        };
                                    }
                                }
                            }
                        }
                        "content_block_start" => {
                            if let Some(content_block) = event.data.get("content_block") {
                                if content_block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                                    let id = content_block.get("id")
                                        .and_then(|i| i.as_str())
                                        .unwrap_or("unknown")
                                        .to_string();
                                    let name = content_block.get("name")
                                        .and_then(|n| n.as_str())
                                        .map(|s| s.to_string());

                                    tool_buffers.insert(id.clone(), ToolCallBuffer {
                                        id: id.clone(),
                                        name,
                                        args_json: String::new(),
                                    });
                                }
                            }
                        }
                        "content_block_stop" => {
                            if let Some(index) = event.data.get("index").and_then(|i| i.as_u64()) {
                                let id = format!("tool_{}", index);
                                if let Some(buffer) = tool_buffers.remove(&id) {
                                    if let Some(name) = buffer.name {
                                        let args: serde_json::Value = serde_json::from_str(&buffer.args_json)
                                            .unwrap_or(serde_json::Value::Object(Default::default()));
                                        yield LlmEvent::ToolCallComplete { id: buffer.id, name, args };
                                    }
                                }
                            }
                        }
                        "message_delta" => {
                            if let Some(delta) = event.data.get("delta") {
                                if let Some(stop) = delta.get("stop_reason").and_then(|s| s.as_str()) {
                                    let stop_reason = match stop {
                                        "end_turn" => StopReason::EndTurn,
                                        "tool_use" => StopReason::ToolUse,
                                        "max_tokens" => StopReason::MaxTokens,
                                        "stop_sequence" => StopReason::StopSequence,
                                        _ => StopReason::EndTurn,
                                    };
                                    yield LlmEvent::Done(stop_reason);
                                }
                            }
                            if let Some(usage) = event.data.get("usage") {
                                yield LlmEvent::Usage(Usage {
                                    input_tokens: usage.get("input_tokens").and_then(|t| t.as_u64()).unwrap_or(0),
                                    output_tokens: usage.get("output_tokens").and_then(|t| t.as_u64()).unwrap_or(0),
                                    cache_creation_tokens: usage.get("cache_creation_input_tokens").and_then(|t| t.as_u64()),
                                    cache_read_tokens: usage.get("cache_read_input_tokens").and_then(|t| t.as_u64()),
                                });
                            }
                        }
                        "message_stop" => {
                            // Final event, stream will close
                        }
                        "error" => {
                            let message = event.data.get("error")
                                .and_then(|e| e.get("message"))
                                .and_then(|m| m.as_str())
                                .unwrap_or("Unknown error")
                                .to_string();
                            Err(LlmError::Unknown { message })?;
                        }
                        _ => {}
                    }
                }
            }
        })
    }

    fn provider(&self) -> &'static str {
        "anthropic"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        // Simple connectivity check
        self.http
            .get(format!("{}/v1/models", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
            .map_err(|_| LlmError::NetworkTimeout {
                duration: std::time::Duration::from_secs(5)
            })?;
        Ok(())
    }
}
```

---

## 8. Tool System

### Tool Registry

```rust
// meerkat-tools/src/registry.rs

use meerkat_core::types::{ToolCall, ToolDef, ToolResult};
use jsonschema::Validator;
use serde_json::Value;
use std::collections::HashMap;

/// Registry of available tools with schema validation
pub struct ToolRegistry {
    tools: HashMap<String, RegisteredTool>,
}

struct RegisteredTool {
    def: ToolDef,
    validator: Validator,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool with its schema
    pub fn register(&mut self, def: ToolDef) -> Result<(), RegistryError> {
        let validator = Validator::new(&def.input_schema)
            .map_err(|e| RegistryError::InvalidSchema {
                tool: def.name.clone(),
                error: e.to_string(),
            })?;

        self.tools.insert(def.name.clone(), RegisteredTool { def, validator });
        Ok(())
    }

    /// Register multiple tools (e.g., from MCP discovery)
    pub fn register_many(&mut self, defs: Vec<ToolDef>) -> Result<(), RegistryError> {
        for def in defs {
            self.register(def)?;
        }
        Ok(())
    }

    /// Get tool definitions for LLM request
    pub fn tool_defs(&self) -> Vec<ToolDef> {
        self.tools.values().map(|t| t.def.clone()).collect()
    }

    /// Validate a tool call's arguments against schema
    pub fn validate(&self, call: &ToolCall) -> Result<(), ToolValidationError> {
        let tool = self.tools.get(&call.name)
            .ok_or_else(|| ToolValidationError::UnknownTool {
                name: call.name.clone(),
                available: self.tools.keys().cloned().collect(),
            })?;

        let result = tool.validator.validate(&call.args);
        if let Err(errors) = result {
            let error_messages: Vec<String> = errors
                .map(|e| format!("{}: {}", e.instance_path, e))
                .collect();

            return Err(ToolValidationError::SchemaViolation {
                tool: call.name.clone(),
                errors: error_messages,
            });
        }

        Ok(())
    }

    /// Check if a tool exists
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Get a tool's schema
    pub fn get_schema(&self, name: &str) -> Option<&Value> {
        self.tools.get(name).map(|t| &t.def.input_schema)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("Invalid schema for tool {tool}: {error}")]
    InvalidSchema { tool: String, error: String },
}

#[derive(Debug, thiserror::Error)]
pub enum ToolValidationError {
    #[error("Unknown tool: {name}. Available: {available:?}")]
    UnknownTool {
        name: String,
        available: Vec<String>,
    },

    #[error("Schema violation for {tool}: {errors:?}")]
    SchemaViolation {
        tool: String,
        errors: Vec<String>,
    },
}

impl ToolValidationError {
    /// Convert to a tool result that can be sent back to the model
    pub fn to_tool_result(&self, tool_use_id: &str) -> ToolResult {
        ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: self.to_string(),
            is_error: true,
        }
    }
}
```

### Tool Dispatcher

```rust
// meerkat-tools/src/dispatcher.rs

use crate::registry::{ToolRegistry, ToolValidationError};
use meerkat_core::types::{ToolCall, ToolDef, ToolResult};
use meerkat_mcp_client::McpRouter;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

/// Dispatches tool calls to MCP servers with validation and timeouts
pub struct ToolDispatcher {
    registry: ToolRegistry,
    router: Arc<McpRouter>,
    default_timeout: Duration,
    tool_timeouts: HashMap<String, Duration>,
}

impl ToolDispatcher {
    pub fn new(router: Arc<McpRouter>, default_timeout: Duration) -> Self {
        Self {
            registry: ToolRegistry::new(),
            router,
            default_timeout,
            tool_timeouts: HashMap::new(),
        }
    }

    /// Set a custom timeout for a specific tool
    pub fn set_tool_timeout(&mut self, tool: &str, timeout: Duration) {
        self.tool_timeouts.insert(tool.to_string(), timeout);
    }

    /// Discover and register tools from MCP servers
    pub async fn discover_tools(&mut self) -> Result<(), DispatchError> {
        let tools = self.router.list_tools().await?;
        self.registry.register_many(tools)?;
        Ok(())
    }

    /// Get tool definitions for LLM
    pub fn tool_defs(&self) -> Vec<ToolDef> {
        self.registry.tool_defs()
    }

    /// Dispatch multiple tool calls in parallel
    pub async fn dispatch_parallel(&self, calls: &[ToolCall]) -> Vec<ToolResult> {
        let futures = calls.iter().map(|call| self.dispatch_one(call));
        futures::future::join_all(futures).await
    }

    /// Dispatch a single tool call with validation and timeout
    pub async fn dispatch_one(&self, call: &ToolCall) -> ToolResult {
        // Step 1: Validate arguments
        if let Err(e) = self.registry.validate(call) {
            return e.to_tool_result(&call.id);
        }

        // Step 2: Get timeout for this tool
        let tool_timeout = self.tool_timeouts
            .get(&call.name)
            .copied()
            .unwrap_or(self.default_timeout);

        // Step 3: Dispatch with timeout
        match timeout(tool_timeout, self.router.call(&call.name, &call.args)).await {
            Ok(Ok(content)) => ToolResult {
                tool_use_id: call.id.clone(),
                content,
                is_error: false,
            },
            Ok(Err(e)) => ToolResult {
                tool_use_id: call.id.clone(),
                content: format!("Tool error: {e}"),
                is_error: true,
            },
            Err(_) => ToolResult {
                tool_use_id: call.id.clone(),
                content: format!(
                    "Tool '{}' timed out after {}s",
                    call.name,
                    tool_timeout.as_secs()
                ),
                is_error: true,
            },
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("MCP error: {0}")]
    Mcp(#[from] meerkat_mcp_client::McpError),

    #[error("Registry error: {0}")]
    Registry(#[from] crate::registry::RegistryError),
}
```

---

## 9. Session Management

### Session Structure

```rust
// meerkat-core/src/session.rs

use crate::types::*;
use std::time::SystemTime;

/// Current session format version
pub const SESSION_VERSION: u32 = 1;

/// A conversation session with full history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Format version for migrations
    #[serde(default = "default_version")]
    version: u32,
    /// Unique identifier
    id: SessionId,
    /// All messages in order
    messages: Vec<Message>,
    /// When the session was created
    created_at: SystemTime,
    /// When the session was last updated
    updated_at: SystemTime,
    /// Arbitrary metadata
    metadata: serde_json::Map<String, serde_json::Value>,
}

fn default_version() -> u32 { SESSION_VERSION }

impl Session {
    /// Create a new empty session
    pub fn new() -> Self {
        let now = SystemTime::now();
        Self {
            version: SESSION_VERSION,
            id: SessionId::new(),
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
            metadata: serde_json::Map::new(),
        }
    }

    /// Create a session with a specific ID (for loading)
    pub fn with_id(id: SessionId) -> Self {
        let mut session = Self::new();
        session.id = id;
        session
    }

    /// Get the session ID
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    /// Get all messages
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Add a message to the session
    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
        self.updated_at = SystemTime::now();
    }

    /// Get the last N messages
    pub fn last_n(&self, n: usize) -> &[Message] {
        let start = self.messages.len().saturating_sub(n);
        &self.messages[start..]
    }

    /// Count total tokens used (approximation from stored usage)
    pub fn total_tokens(&self) -> u64 {
        self.messages
            .iter()
            .filter_map(|m| match m {
                Message::Assistant(a) => Some(a.usage.total_tokens()),
                _ => None,
            })
            .sum()
    }

    /// Count tool calls made
    pub fn tool_call_count(&self) -> usize {
        self.messages
            .iter()
            .filter_map(|m| match m {
                Message::Assistant(a) => Some(a.tool_calls.len()),
                _ => None,
            })
            .sum()
    }

    /// Get/set metadata
    pub fn metadata(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.metadata
    }

    pub fn set_metadata(&mut self, key: &str, value: serde_json::Value) {
        self.metadata.insert(key.to_string(), value);
        self.updated_at = SystemTime::now();
    }

    /// Fork the session at a specific message index
    pub fn fork_at(&self, index: usize) -> Self {
        let mut forked = Self::new();
        forked.messages = self.messages[..index.min(self.messages.len())].to_vec();
        forked.metadata = self.metadata.clone();
        forked
    }
}

/// Summary metadata for listing sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: SessionId,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub message_count: usize,
    pub total_tokens: u64,
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

impl From<&Session> for SessionMeta {
    fn from(session: &Session) -> Self {
        Self {
            id: session.id.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
            message_count: session.messages.len(),
            total_tokens: session.total_tokens(),
            metadata: session.metadata.clone(),
        }
    }
}
```

### Session Store Trait

```rust
// meerkat-store/src/lib.rs

use async_trait::async_trait;
use meerkat_core::{Session, SessionId, SessionMeta};

/// Filter for listing sessions
#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    /// Only sessions created after this time
    pub created_after: Option<SystemTime>,
    /// Only sessions updated after this time
    pub updated_after: Option<SystemTime>,
    /// Maximum number of results
    pub limit: Option<usize>,
    /// Offset for pagination
    pub offset: Option<usize>,
}

/// Abstraction over session storage backends
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Save a session (create or update)
    async fn save(&self, session: &Session) -> Result<(), StoreError>;

    /// Load a session by ID
    async fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError>;

    /// List sessions matching filter
    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError>;

    /// Delete a session
    async fn delete(&self, id: &SessionId) -> Result<(), StoreError>;

    /// Check if a session exists
    async fn exists(&self, id: &SessionId) -> Result<bool, StoreError> {
        Ok(self.load(id).await?.is_some())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Session corrupted: {id:?}")]
    Corrupted { id: SessionId },
}
```

### JSONL Store Implementation

```rust
// meerkat-store/src/jsonl.rs

use super::{SessionStore, SessionFilter, StoreError};
use meerkat_core::{Session, SessionId, SessionMeta, Message};
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// File-based session store using JSONL format
///
/// Each session is stored as a single file with one JSON object per line.
/// This allows append-only writes for incremental saves.
pub struct JsonlStore {
    dir: PathBuf,
}

impl JsonlStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Ensure storage directory exists
    pub async fn init(&self) -> Result<(), StoreError> {
        fs::create_dir_all(&self.dir).await?;
        Ok(())
    }

    fn session_path(&self, id: &SessionId) -> PathBuf {
        self.dir.join(format!("{}.jsonl", id.0))
    }
}

#[async_trait]
impl SessionStore for JsonlStore {
    async fn save(&self, session: &Session) -> Result<(), StoreError> {
        let path = self.session_path(session.id());

        // Write complete session as JSONL (one line per message)
        let mut file = fs::File::create(&path).await?;

        // First line: session metadata
        let meta = SessionMeta::from(session);
        let meta_json = serde_json::to_string(&meta)
            .map_err(|e| StoreError::Serialization(e.to_string()))?;
        file.write_all(meta_json.as_bytes()).await?;
        file.write_all(b"\n").await?;

        // Subsequent lines: messages
        for message in session.messages() {
            let msg_json = serde_json::to_string(message)
                .map_err(|e| StoreError::Serialization(e.to_string()))?;
            file.write_all(msg_json.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }

        file.flush().await?;
        Ok(())
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        let path = self.session_path(id);

        if !path.exists() {
            return Ok(None);
        }

        let file = fs::File::open(&path).await?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        // First line: metadata (we use it to reconstruct session)
        let meta_line = lines.next_line().await?
            .ok_or(StoreError::Corrupted { id: id.clone() })?;
        let meta: SessionMeta = serde_json::from_str(&meta_line)
            .map_err(|e| StoreError::Serialization(e.to_string()))?;

        let mut session = Session::with_id(meta.id);

        // Subsequent lines: messages
        while let Some(line) = lines.next_line().await? {
            let message: Message = serde_json::from_str(&line)
                .map_err(|e| StoreError::Serialization(e.to_string()))?;
            session.push(message);
        }

        Ok(Some(session))
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError> {
        let mut entries = fs::read_dir(&self.dir).await?;
        let mut sessions = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            // Read just the first line (metadata)
            let file = fs::File::open(&path).await?;
            let reader = BufReader::new(file);
            let mut lines = reader.lines();

            if let Some(line) = lines.next_line().await? {
                if let Ok(meta) = serde_json::from_str::<SessionMeta>(&line) {
                    // Apply filters
                    if let Some(after) = filter.created_after {
                        if meta.created_at < after {
                            continue;
                        }
                    }
                    if let Some(after) = filter.updated_after {
                        if meta.updated_at < after {
                            continue;
                        }
                    }
                    sessions.push(meta);
                }
            }
        }

        // Sort by updated_at descending
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        // Apply pagination
        let offset = filter.offset.unwrap_or(0);
        let limit = filter.limit.unwrap_or(usize::MAX);

        Ok(sessions.into_iter().skip(offset).take(limit).collect())
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        let path = self.session_path(id);
        if path.exists() {
            fs::remove_file(&path).await?;
        }
        Ok(())
    }
}
```

### In-Memory Store (for testing)

```rust
// meerkat-store/src/memory.rs

use super::{SessionStore, SessionFilter, StoreError};
use meerkat_core::{Session, SessionId, SessionMeta};
use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::RwLock;

/// In-memory session store for testing
pub struct MemoryStore {
    sessions: RwLock<HashMap<SessionId, Session>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl SessionStore for MemoryStore {
    async fn save(&self, session: &Session) -> Result<(), StoreError> {
        let mut sessions = self.sessions.write().await;
        sessions.insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        let sessions = self.sessions.read().await;
        Ok(sessions.get(id).cloned())
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError> {
        let sessions = self.sessions.read().await;
        let mut metas: Vec<SessionMeta> = sessions
            .values()
            .filter(|s| {
                if let Some(after) = filter.created_after {
                    if s.created_at < after {
                        return false;
                    }
                }
                if let Some(after) = filter.updated_after {
                    if s.updated_at < after {
                        return false;
                    }
                }
                true
            })
            .map(SessionMeta::from)
            .collect();

        metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        let offset = filter.offset.unwrap_or(0);
        let limit = filter.limit.unwrap_or(usize::MAX);

        Ok(metas.into_iter().skip(offset).take(limit).collect())
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(id);
        Ok(())
    }
}
```

---

## 10. Budget Enforcement

```rust
// meerkat-core/src/budget.rs

use crate::error::AgentError;
use std::time::{Duration, Instant};

/// Resource budget for an agent run
#[derive(Debug, Clone)]
pub struct Budget {
    // Token limits
    max_tokens: Option<u64>,
    tokens_used: u64,

    // Time limits
    max_duration: Option<Duration>,
    start_time: Option<Instant>,

    // Tool call limits
    max_tool_calls: Option<usize>,
    tool_calls_made: usize,
}

impl Budget {
    /// Create a new budget with no limits
    pub fn unlimited() -> Self {
        Self {
            max_tokens: None,
            tokens_used: 0,
            max_duration: None,
            start_time: None,
            max_tool_calls: None,
            tool_calls_made: 0,
        }
    }

    /// Set maximum tokens
    pub fn with_max_tokens(mut self, max: u64) -> Self {
        self.max_tokens = Some(max);
        self
    }

    /// Set maximum duration
    pub fn with_max_duration(mut self, max: Duration) -> Self {
        self.max_duration = Some(max);
        self
    }

    /// Set maximum tool calls
    pub fn with_max_tool_calls(mut self, max: usize) -> Self {
        self.max_tool_calls = Some(max);
        self
    }

    /// Start the timer (call at beginning of run)
    pub fn start(&mut self) {
        self.start_time = Some(Instant::now());
    }

    /// Record tokens used
    pub fn record_tokens(&mut self, tokens: u64) {
        self.tokens_used += tokens;
    }

    /// Record tool calls made
    pub fn record_calls(&mut self, calls: usize) {
        self.tool_calls_made += calls;
    }

    /// Check if any budget is exceeded
    pub fn check(&self) -> Result<(), AgentError> {
        // Check tokens
        if let Some(max) = self.max_tokens {
            if self.tokens_used >= max {
                return Err(AgentError::TokenBudgetExceeded {
                    used: self.tokens_used,
                    limit: max,
                });
            }
        }

        // Check time
        if let (Some(max), Some(start)) = (self.max_duration, self.start_time) {
            let elapsed = start.elapsed();
            if elapsed >= max {
                return Err(AgentError::TimeBudgetExceeded {
                    elapsed_secs: elapsed.as_secs(),
                    limit_secs: max.as_secs(),
                });
            }
        }

        // Check tool calls
        if let Some(max) = self.max_tool_calls {
            if self.tool_calls_made >= max {
                return Err(AgentError::ToolCallBudgetExceeded {
                    count: self.tool_calls_made,
                    limit: max,
                });
            }
        }

        Ok(())
    }

    /// Get token usage as (used, limit) if limit is set
    pub fn token_usage(&self) -> Option<(u64, u64)> {
        self.max_tokens.map(|max| (self.tokens_used, max))
    }

    /// Get time usage as (elapsed_ms, limit_ms) if limit is set
    pub fn time_usage(&self) -> Option<(u64, u64)> {
        match (self.max_duration, self.start_time) {
            (Some(max), Some(start)) => {
                Some((start.elapsed().as_millis() as u64, max.as_millis() as u64))
            }
            _ => None,
        }
    }

    /// Get tool call usage as (made, limit) if limit is set
    pub fn call_usage(&self) -> Option<(usize, usize)> {
        self.max_tool_calls.map(|max| (self.tool_calls_made, max))
    }

    /// Get total usage summary
    pub fn usage_summary(&self) -> BudgetUsage {
        BudgetUsage {
            tokens_used: self.tokens_used,
            tokens_limit: self.max_tokens,
            elapsed: self.start_time.map(|s| s.elapsed()),
            time_limit: self.max_duration,
            tool_calls_made: self.tool_calls_made,
            tool_calls_limit: self.max_tool_calls,
        }
    }
}

/// Summary of budget usage
#[derive(Debug, Clone)]
pub struct BudgetUsage {
    pub tokens_used: u64,
    pub tokens_limit: Option<u64>,
    pub elapsed: Option<Duration>,
    pub time_limit: Option<Duration>,
    pub tool_calls_made: usize,
    pub tool_calls_limit: Option<usize>,
}
```

---

## 11. Configuration

### Configuration Structure

```rust
// meerkat-core/src/config.rs

use std::path::PathBuf;
use std::time::Duration;
use serde::{Deserialize, Serialize};

/// Complete configuration for Meerkat
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Agent behavior configuration
    pub agent: AgentConfig,

    /// LLM provider configuration
    pub provider: ProviderConfig,

    /// Tool/MCP configuration
    pub tools: ToolsConfig,

    /// Storage configuration
    pub storage: StorageConfig,

    /// Budget limits
    pub budget: BudgetConfig,

    /// Retry policy
    pub retry: RetryConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agent: AgentConfig::default(),
            provider: ProviderConfig::default(),
            tools: ToolsConfig::default(),
            storage: StorageConfig::default(),
            budget: BudgetConfig::default(),
            retry: RetryConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// System prompt
    pub system_prompt: Option<String>,
    /// Path to system prompt file
    pub system_prompt_file: Option<PathBuf>,
    /// Model identifier
    pub model: String,
    /// Max tokens per turn
    pub max_tokens_per_turn: u32,
    /// Temperature
    pub temperature: Option<f32>,
    /// Checkpoint interval (turns)
    pub checkpoint_interval: Option<u32>,
    /// Budget warning threshold (0.0-1.0)
    pub budget_warning_threshold: f32,
    /// Maximum turns before forced stop
    pub max_turns: Option<u32>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: None,
            system_prompt_file: None,
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens_per_turn: 8192,
            temperature: None,
            checkpoint_interval: Some(5),
            budget_warning_threshold: 0.8,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderConfig {
    Anthropic {
        api_key: Option<String>,  // Falls back to ANTHROPIC_API_KEY env
        base_url: Option<String>,
    },
    #[serde(rename = "openai")]
    OpenAI {
        api_key: Option<String>,  // Falls back to OPENAI_API_KEY env
        base_url: Option<String>,
    },
    Gemini {
        api_key: Option<String>,  // Falls back to GEMINI_API_KEY env
    },
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self::Anthropic {
            api_key: None,
            base_url: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    /// MCP server configurations
    pub mcp_servers: Vec<McpServerConfig>,
    /// Default timeout for tool calls (10 min default for agentic workflows)
    #[serde(with = "humantime_serde")]
    pub default_timeout: Duration,
    /// Per-tool timeout overrides
    pub tool_timeouts: HashMap<String, Duration>,
    /// Maximum concurrent tool executions
    pub max_concurrent: usize,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            mcp_servers: Vec::new(),
            default_timeout: Duration::from_secs(600),
            tool_timeouts: HashMap::new(),
            max_concurrent: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Server name (for logging)
    pub name: String,
    /// Command to spawn the server
    pub command: String,
    /// Arguments
    pub args: Vec<String>,
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Storage backend type
    pub backend: StorageBackend,
    /// Directory for file-based storage (None = use platform default)
    pub directory: Option<PathBuf>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: StorageBackend::Jsonl,
            directory: dirs::data_dir().map(|d| d.join("rkat/sessions")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageBackend {
    Jsonl,
    Memory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BudgetConfig {
    /// Maximum tokens (None = unlimited)
    pub max_tokens: Option<u64>,
    /// Maximum duration
    #[serde(with = "humantime_serde")]
    pub max_duration: Option<Duration>,
    /// Maximum tool calls
    pub max_tool_calls: Option<usize>,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_tokens: None,
            max_duration: None,
            max_tool_calls: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    /// Maximum retry attempts
    pub max_retries: u32,
    /// Initial retry delay
    #[serde(with = "humantime_serde")]
    pub initial_delay: Duration,
    /// Maximum retry delay
    #[serde(with = "humantime_serde")]
    pub max_delay: Duration,
    /// Backoff multiplier
    pub multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            multiplier: 2.0,
        }
    }
}
```

### Configuration Loading

Configuration follows a layered approach inspired by Codex CLI:

```
Priority (highest to lowest):
1. CLI arguments (--model, --max-tokens, etc.)
2. Environment variables (RKAT_MODEL, ANTHROPIC_API_KEY, etc.)
3. Project config (.rkat/config.toml in current directory or parents)
4. User config (~/.config/meerkat/config.toml)
5. Defaults
```

```rust
// meerkat-core/src/config/loader.rs

impl Config {
    /// Load configuration from all sources
    pub fn load() -> Result<Self, ConfigError> {
        let mut config = Config::default();

        // Load user config
        if let Some(path) = Self::user_config_path() {
            if path.exists() {
                let user_config = Self::load_file(&path)?;
                config.merge(user_config);
            }
        }

        // Load project config (walk up directories)
        if let Some(path) = Self::find_project_config() {
            let project_config = Self::load_file(&path)?;
            config.merge(project_config);
        }

        // Apply environment variables
        config.apply_env();

        Ok(config)
    }

    fn user_config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("meerkat").join("config.toml"))
    }

    fn find_project_config() -> Option<PathBuf> {
        let mut dir = std::env::current_dir().ok()?;
        loop {
            let config_path = dir.join(".rkat").join("config.toml");
            if config_path.exists() {
                return Some(config_path);
            }
            if !dir.pop() {
                return None;
            }
        }
    }

    fn apply_env(&mut self) {
        // Provider API keys
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            if let ProviderConfig::Anthropic { api_key, .. } = &mut self.provider {
                *api_key = Some(key);
            }
        }
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            if let ProviderConfig::OpenAI { api_key, .. } = &mut self.provider {
                *api_key = Some(key);
            }
        }

        // Model override
        if let Ok(model) = std::env::var("RKAT_MODEL") {
            self.agent.model = model;
        }

        // Budget overrides
        if let Ok(tokens) = std::env::var("RKAT_MAX_TOKENS") {
            if let Ok(n) = tokens.parse() {
                self.budget.max_tokens = Some(n);
            }
        }
    }
}
```

### Example Configuration File

```toml
# ~/.config/meerkat/config.toml

[agent]
model = "claude-sonnet-4-20250514"
max_tokens_per_turn = 8192
checkpoint_interval = 5

[provider]
type = "anthropic"
# api_key loaded from ANTHROPIC_API_KEY env var

[tools]
default_timeout = "30s"

[[tools.mcp_servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@anthropic/mcp-server-filesystem", "/home/user/projects"]

[[tools.mcp_servers]]
name = "github"
command = "npx"
args = ["-y", "@anthropic/mcp-server-github"]
env = { GITHUB_TOKEN = "${GITHUB_TOKEN}" }

[storage]
backend = "jsonl"
directory = "~/.local/share/meerkat/sessions"

[budget]
max_tokens = 100000
max_duration = "10m"
max_tool_calls = 50

[retry]
max_retries = 3
initial_delay = "500ms"
max_delay = "30s"
multiplier = 2.0
```

---

## 12. Access Patterns

### CLI (meerkat-cli)

```rust
// meerkat-cli/src/main.rs

use clap::{Parser, Subcommand};
use meerkat::{Agent, Config, RunResult};

#[derive(Parser)]
#[command(name = "meerkat")]
#[command(about = "Rust Agentic Interface Kit")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Configuration file path
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    /// Output format
    #[arg(short, long, global = true, default_value = "text")]
    output: OutputFormat,
}

#[derive(Subcommand)]
enum Commands {
    /// Run an agent with a prompt
    Run {
        /// The prompt to execute
        prompt: String,

        /// Model to use
        #[arg(short, long)]
        model: Option<String>,

        /// Maximum tokens
        #[arg(long)]
        max_tokens: Option<u64>,

        /// Maximum duration (e.g., "5m", "1h")
        #[arg(long)]
        max_duration: Option<humantime::Duration>,

        /// Stream events to stderr
        #[arg(long)]
        stream: bool,
    },

    /// Resume a previous session
    Resume {
        /// Session ID to resume
        session_id: String,

        /// Additional prompt
        prompt: String,
    },

    /// List sessions
    Sessions {
        #[command(subcommand)]
        command: SessionCommands,
    },
}

#[derive(Subcommand)]
enum SessionCommands {
    /// List all sessions
    List {
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Show session details
    Show { session_id: String },

    /// Delete a session
    Delete { session_id: String },
}

#[derive(Clone, Copy, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
    JsonStream,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Load configuration
    let mut config = if let Some(path) = cli.config {
        Config::load_file(&path)?
    } else {
        Config::load()?
    };

    match cli.command {
        Commands::Run { prompt, model, max_tokens, max_duration, stream } => {
            // Apply CLI overrides
            if let Some(m) = model {
                config.agent.model = m;
            }
            if let Some(t) = max_tokens {
                config.budget.max_tokens = Some(t);
            }
            if let Some(d) = max_duration {
                config.budget.max_duration = Some(d.into());
            }

            let mut agent = build_agent(config).await?;

            if stream {
                let (tx, mut rx) = tokio::sync::mpsc::channel(128);

                // Spawn event printer
                let output_format = cli.output;
                tokio::spawn(async move {
                    while let Some(event) = rx.recv().await {
                        print_event(&event, output_format);
                    }
                });

                let result = agent.run_with_events(&prompt, tx).await?;
                print_result(&result, cli.output);
            } else {
                let result = agent.run(&prompt).await?;
                print_result(&result, cli.output);
            }
        }

        Commands::Resume { session_id, prompt } => {
            let id = SessionId(session_id.parse()?);
            let mut agent = build_agent(config).await?;
            let result = agent.resume(&id, &prompt).await?;
            print_result(&result, cli.output);
        }

        Commands::Sessions { command } => {
            handle_sessions_command(command, config, cli.output).await?;
        }
    }

    Ok(())
}

fn print_result(result: &RunResult, format: OutputFormat) {
    match format {
        OutputFormat::Text => {
            println!("{}", result.text);
            eprintln!("\n---");
            eprintln!("Session: {}", result.session_id.0);
            eprintln!("Tokens: {}", result.usage.total_tokens());
            eprintln!("Turns: {}", result.turns);
            eprintln!("Tool calls: {}", result.tool_calls);
        }
        OutputFormat::Json | OutputFormat::JsonStream => {
            println!("{}", serde_json::to_string_pretty(result).unwrap());
        }
    }
}

fn print_event(event: &AgentEvent, format: OutputFormat) {
    match format {
        OutputFormat::Text => {
            // Human-readable event output
            match event {
                AgentEvent::TextDelta { delta } => {
                    eprint!("{}", delta);
                }
                AgentEvent::ToolExecutionStarted { name, .. } => {
                    eprintln!("\n[Calling tool: {}]", name);
                }
                AgentEvent::ToolExecutionCompleted { name, duration_ms, is_error, .. } => {
                    let status = if *is_error { "error" } else { "ok" };
                    eprintln!("[Tool {} completed: {} in {}ms]", name, status, duration_ms);
                }
                _ => {}
            }
        }
        OutputFormat::JsonStream => {
            println!("{}", serde_json::to_string(event).unwrap());
        }
        _ => {}
    }
}
```

**CLI Usage Examples:**

```bash
# Basic run
rkat run "Analyze the code in src/ and suggest improvements"

# With budget limits
rkat run "Refactor the auth module" --max-tokens 50000 --max-duration 5m

# Stream events
rkat run "Debug the failing tests" --stream --output json-stream

# Resume session
rkat resume abc123 "Now fix the issues you identified"

# List sessions
rkat sessions list --limit 20

# Show session
rkat sessions show abc123

# JSON output for scripting
rkat run "Generate a summary" --output json | jq '.text'
```

### MCP Server (meerkat-mcp-server)

Exposes Meerkat as an MCP tool that other agents can use.

```rust
// meerkat-mcp-server/src/server.rs

use rmcp::{Server, Tool, ToolResult};
use meerkat::{Agent, Config};
use serde::{Deserialize, Serialize};

/// MCP server that exposes Meerkat as a tool
pub struct RaikMcpServer {
    config: Config,
}

impl RaikMcpServer {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        let server = Server::new("meerkat", "0.1.0")
            .with_tool(RaikRunTool { config: self.config.clone() })
            .with_tool(RaikResumeTool { config: self.config.clone() });

        server.run_stdio().await
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct RaikRunInput {
    /// The prompt to execute
    prompt: String,
    /// Optional system prompt override
    system_prompt: Option<String>,
    /// Optional model override
    model: Option<String>,
    /// Maximum tokens to use
    max_tokens: Option<u64>,
}

struct RaikRunTool {
    config: Config,
}

#[async_trait]
impl Tool for RaikRunTool {
    fn name(&self) -> &str {
        "meerkat_run"
    }

    fn description(&self) -> &str {
        "Run an AI agent to complete a task. The agent can use tools and will work autonomously until the task is complete."
    }

    fn input_schema(&self) -> serde_json::Value {
        schemars::schema_for!(RaikRunInput).into()
    }

    async fn call(&self, input: serde_json::Value) -> ToolResult {
        let input: RaikRunInput = serde_json::from_value(input)
            .map_err(|e| format!("Invalid input: {e}"))?;

        let mut config = self.config.clone();
        if let Some(system) = input.system_prompt {
            config.agent.system_prompt = Some(system);
        }
        if let Some(model) = input.model {
            config.agent.model = model;
        }
        if let Some(tokens) = input.max_tokens {
            config.budget.max_tokens = Some(tokens);
        }

        let mut agent = build_agent(config).await
            .map_err(|e| format!("Failed to build agent: {e}"))?;

        let result = agent.run(&input.prompt).await
            .map_err(|e| format!("Agent error: {e}"))?;

        ToolResult::success(serde_json::json!({
            "result": result.text,
            "session_id": result.session_id.0.to_string(),
            "usage": {
                "tokens": result.usage.total_tokens(),
                "turns": result.turns,
                "tool_calls": result.tool_calls,
            }
        }))
    }
}
```

**MCP Server Usage:**

```bash
# Start the MCP server
rkat mcp-server

# Or in an MCP config file
{
  "mcpServers": {
    "meerkat": {
      "command": "meerkat",
      "args": ["mcp-server"]
    }
  }
}
```

### SDK (meerkat)

The SDK crate re-exports all public APIs for embedding.

```rust
// meerkat/src/lib.rs

//! Meerkat - Rust Agentic Interface Kit
//!
//! A minimal, high-performance agent harness for LLM-powered applications.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use meerkat::{Agent, AgentBuilder, Config};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let agent = AgentBuilder::new()
//!         .with_anthropic_from_env()?
//!         .with_mcp_server("filesystem", "npx", &["-y", "@anthropic/mcp-server-filesystem", "."])
//!         .build()
//!         .await?;
//!
//!     let result = agent.run("List the files in the current directory").await?;
//!     println!("{}", result.text);
//!     Ok(())
//! }
//! ```

// Re-export core types
pub use meerkat_core::{
    Agent, AgentConfig, AgentError, AgentEvent,
    Session, SessionId, SessionMeta,
    Message, UserMessage, AssistantMessage, SystemMessage,
    ToolCall, ToolResult, ToolDef,
    StopReason, Usage,
    Budget, BudgetUsage,
    RetryPolicy,
    Config,
};

// Re-export client types
pub use meerkat_client::{
    LlmClient, LlmEvent, LlmError, LlmRequest,
    AnthropicClient, OpenAiClient, GeminiClient,
};

// Re-export tools
pub use meerkat_tools::{
    ToolRegistry, ToolDispatcher,
    ToolValidationError, DispatchError,
};

// Re-export store
pub use meerkat_store::{
    SessionStore, SessionFilter, StoreError,
    JsonlStore, MemoryStore,
};

// Re-export MCP client
pub use meerkat_mcp_client::{
    McpRouter, McpError, McpServerConfig,
};

// Builder for convenient agent construction
mod builder;
pub use builder::AgentBuilder;
```

### Builder Pattern for SDK

```rust
// meerkat/src/builder.rs

use crate::*;
use std::sync::Arc;

/// Fluent builder for constructing agents
pub struct AgentBuilder {
    config: Config,
    client: Option<Arc<dyn LlmClient>>,
    store: Option<Arc<dyn SessionStore>>,
    mcp_servers: Vec<McpServerConfig>,
}

impl AgentBuilder {
    pub fn new() -> Self {
        Self {
            config: Config::default(),
            client: None,
            store: None,
            mcp_servers: Vec::new(),
        }
    }

    /// Load configuration from default locations
    pub fn with_config_from_env(mut self) -> Result<Self, ConfigError> {
        self.config = Config::load()?;
        Ok(self)
    }

    /// Use Anthropic with API key from environment
    pub fn with_anthropic_from_env(mut self) -> Result<Self, ConfigError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| ConfigError::MissingApiKey("ANTHROPIC_API_KEY"))?;
        self.client = Some(Arc::new(AnthropicClient::new(api_key)));
        Ok(self)
    }

    /// Use Anthropic with explicit API key
    pub fn with_anthropic(mut self, api_key: &str) -> Self {
        self.client = Some(Arc::new(AnthropicClient::new(api_key.to_string())));
        self
    }

    /// Use OpenAI
    pub fn with_openai(mut self, api_key: &str) -> Self {
        self.client = Some(Arc::new(OpenAiClient::new(api_key.to_string())));
        self
    }

    /// Use Gemini
    pub fn with_gemini(mut self, api_key: &str) -> Self {
        self.client = Some(Arc::new(GeminiClient::new(api_key.to_string())));
        self
    }

    /// Set the model
    pub fn with_model(mut self, model: &str) -> Self {
        self.config.agent.model = model.to_string();
        self
    }

    /// Set the system prompt
    pub fn with_system_prompt(mut self, prompt: &str) -> Self {
        self.config.agent.system_prompt = Some(prompt.to_string());
        self
    }

    /// Add an MCP server
    pub fn with_mcp_server(mut self, name: &str, command: &str, args: &[&str]) -> Self {
        self.mcp_servers.push(McpServerConfig {
            name: name.to_string(),
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: HashMap::new(),
        });
        self
    }

    /// Set token budget
    pub fn with_max_tokens(mut self, max: u64) -> Self {
        self.config.budget.max_tokens = Some(max);
        self
    }

    /// Set time budget
    pub fn with_max_duration(mut self, duration: Duration) -> Self {
        self.config.budget.max_duration = Some(duration);
        self
    }

    /// Use in-memory storage (for testing)
    pub fn with_memory_store(mut self) -> Self {
        self.store = Some(Arc::new(MemoryStore::new()));
        self
    }

    /// Use JSONL file storage
    pub fn with_jsonl_store(mut self, dir: PathBuf) -> Self {
        self.store = Some(Arc::new(JsonlStore::new(dir)));
        self
    }

    /// Build the agent
    pub async fn build(self) -> Result<Agent, BuildError> {
        let client = self.client
            .ok_or(BuildError::NoClient)?;

        let store = self.store
            .unwrap_or_else(|| Arc::new(MemoryStore::new()));

        // Initialize MCP router and discover tools
        let mut router = McpRouter::new();
        for server_config in &self.mcp_servers {
            router.add_server(server_config.clone()).await?;
        }

        let mut dispatcher = ToolDispatcher::new(
            Arc::new(router),
            self.config.tools.default_timeout,
        );
        dispatcher.discover_tools().await?;

        let budget = Budget::unlimited()
            .with_max_tokens(self.config.budget.max_tokens.unwrap_or(u64::MAX))
            .with_max_duration(self.config.budget.max_duration.unwrap_or(Duration::MAX))
            .with_max_tool_calls(self.config.budget.max_tool_calls.unwrap_or(usize::MAX));

        let retry_policy = RetryPolicy {
            max_retries: self.config.retry.max_retries,
            initial_delay: self.config.retry.initial_delay,
            max_delay: self.config.retry.max_delay,
            multiplier: self.config.retry.multiplier,
        };

        Ok(Agent::new(
            client,
            dispatcher,
            store,
            budget,
            retry_policy,
            self.config.agent.into(),
        ))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("No LLM client configured")]
    NoClient,

    #[error("MCP error: {0}")]
    Mcp(#[from] McpError),

    #[error("Tool discovery failed: {0}")]
    ToolDiscovery(#[from] DispatchError),
}
```

**SDK Usage Example:**

```rust
use meerkat::{AgentBuilder, AgentEvent};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Simple usage
    let mut agent = AgentBuilder::new()
        .with_anthropic_from_env()?
        .with_model("claude-sonnet-4-20250514")
        .with_mcp_server("fs", "npx", &["-y", "@anthropic/mcp-server-filesystem", "."])
        .with_max_tokens(50000)
        .build()
        .await?;

    let result = agent.run("Analyze the Rust code and find potential bugs").await?;
    println!("Result: {}", result.text);

    // With event streaming
    let (tx, mut rx) = mpsc::channel(128);

    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::TextDelta { delta } => print!("{}", delta),
                AgentEvent::ToolExecutionStarted { name, .. } => {
                    println!("\n[Calling {}...]", name);
                }
                _ => {}
            }
        }
    });

    let result = agent.run_with_events("Explain this codebase", tx).await?;
    println!("\n\nSession ID: {}", result.session_id.0);

    Ok(())
}
```

---

## 13. Error Taxonomy

### Error Hierarchy

```
AgentError (top-level)
├── LlmError (from LLM providers)
│   ├── Retryable
│   │   ├── RateLimited
│   │   ├── ServerOverloaded
│   │   ├── NetworkTimeout
│   │   └── ConnectionReset
│   └── Non-Retryable
│       ├── InvalidRequest
│       ├── AuthenticationFailed
│       ├── ContentFiltered
│       ├── ContextLengthExceeded
│       └── ModelNotFound
├── StoreError (from session storage)
│   ├── Io
│   ├── Serialization
│   └── Corrupted
├── ToolError (from tool execution)
│   ├── ValidationError (schema violation)
│   ├── UnknownTool
│   ├── Timeout
│   └── McpError
├── BudgetError
│   ├── TokenBudgetExceeded
│   ├── TimeBudgetExceeded
│   └── ToolCallBudgetExceeded
└── SessionError
    ├── NotFound
    └── Corrupted
```

### Error Handling Philosophy

1. **Retryable errors are retried automatically** by the retry policy
2. **Tool errors are returned to the model** as error tool results, allowing self-correction
3. **Budget errors abort the run** cleanly with partial results when possible
4. **Storage errors are logged** but don't abort (degraded mode)

---

## 14. Observability

### Structured Logging

```rust
// Use tracing for structured logging
use tracing::{info, warn, error, instrument, Span};

impl Agent {
    #[instrument(skip(self), fields(session_id, model = %self.config.model))]
    pub async fn run(&mut self, prompt: &str) -> Result<RunResult, AgentError> {
        let session_id = SessionId::new();
        Span::current().record("session_id", session_id.0.to_string());

        info!(prompt_len = prompt.len(), "Starting agent run");

        // ... execution ...

        info!(
            tokens = result.usage.total_tokens(),
            turns = result.turns,
            tool_calls = result.tool_calls,
            "Agent run completed"
        );

        Ok(result)
    }
}
```

### Metrics

Expose metrics for monitoring:

```rust
// meerkat-core/src/metrics.rs

use std::sync::atomic::{AtomicU64, Ordering};

pub struct Metrics {
    pub runs_started: AtomicU64,
    pub runs_completed: AtomicU64,
    pub runs_failed: AtomicU64,
    pub tokens_used: AtomicU64,
    pub tool_calls_made: AtomicU64,
    pub retries: AtomicU64,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            runs_started: AtomicU64::new(0),
            runs_completed: AtomicU64::new(0),
            runs_failed: AtomicU64::new(0),
            tokens_used: AtomicU64::new(0),
            tool_calls_made: AtomicU64::new(0),
            retries: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            runs_started: self.runs_started.load(Ordering::Relaxed),
            runs_completed: self.runs_completed.load(Ordering::Relaxed),
            runs_failed: self.runs_failed.load(Ordering::Relaxed),
            tokens_used: self.tokens_used.load(Ordering::Relaxed),
            tool_calls_made: self.tool_calls_made.load(Ordering::Relaxed),
            retries: self.retries.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricsSnapshot {
    pub runs_started: u64,
    pub runs_completed: u64,
    pub runs_failed: u64,
    pub tokens_used: u64,
    pub tool_calls_made: u64,
    pub retries: u64,
}
```

---

## 15. Security Considerations

### Tool Execution

1. **Schema validation**: All tool arguments are validated against JSON schemas before dispatch
2. **Timeouts**: Every tool call has a timeout to prevent hangs
3. **MCP isolation**: Tools run in separate MCP server processes

### Credentials

1. **Environment variables**: API keys should come from environment, not config files
2. **No logging of secrets**: API keys and credentials are never logged
3. **Secure storage**: Future: integrate with system keychains

### Input Validation

1. **Prompt limits**: Configurable maximum prompt length
2. **Output limits**: Maximum output tokens per turn
3. **Rate limiting**: Respect provider rate limits, implement client-side throttling

### Session Data

1. **Local storage**: Sessions stored locally, not sent to external services (beyond LLM provider)
2. **Optional encryption**: Future: encrypt session files at rest
3. **Retention policies**: Future: automatic session expiration

---

## 16. Design Decisions

### Why Rust?

| Reason | Benefit |
|--------|---------|
| **Performance** | No GC pauses, efficient memory usage |
| **Safety** | Memory safety, thread safety by default |
| **Single binary** | Easy distribution, no runtime dependencies |
| **Async ecosystem** | Tokio provides excellent async I/O |
| **Type system** | Catch errors at compile time |

### Why MCP-Only Tools?

| Reason | Benefit |
|--------|---------|
| **Separation of concerns** | Meerkat focuses on the loop, not tool implementation |
| **Flexibility** | Any MCP server works, including custom ones |
| **Ecosystem** | Leverage existing MCP servers |
| **Security** | Tools run in separate processes |
| **No bloat** | Don't ship tools you don't need |

### Why Streaming-First?

| Reason | Benefit |
|--------|---------|
| **Responsiveness** | See output as it's generated |
| **Cancellation** | Can abort mid-generation |
| **Memory efficiency** | Don't buffer entire responses |
| **Natural fit** | LLM APIs are streaming by nature |

### Why Trait-Based Abstractions?

| Trait | Why |
|-------|-----|
| `LlmClient` | Swap providers without changing core logic |
| `SessionStore` | Test with in-memory, deploy with files/DB |
| `Send + Sync` | Thread-safe by design |

### Sub-Agent Design Philosophy

Claude Code spawns sub-agents for complex tasks. Meerkat supports sub-agents with explicit design constraints (see §18):

1. **Turn-based control**: All operations synchronize at turn boundaries
2. **Resource control**: BudgetPool with explicit allocation and reclamation
3. **Limited orchestration**: Fork/spawn/steer primitives; complex patterns are a layer above
4. **Tool-based interface**: Sub-agents exposed as internal tools (`/fork`, `/spawn`, `/steer`)

---

## 17. Prior Art

### Claude Code

- Single-threaded master loop with async steering
- 14 built-in tools, sub-agent spawning via Task tool
- TodoWrite for structured planning
- Context compaction at ~92% usage
- JSONL persistence

**Lessons learned:**
- Simple loop is sufficient for high agency
- Tool call streaming needs careful buffering
- Budget enforcement is critical for long runs

### Codex CLI (codex-rs)

- Protocol-first design (Op/Event)
- 50 crates in workspace
- Multiple frontends sharing core
- Starlark-based approval policies
- Platform-specific sandboxing

**Lessons learned:**
- Clean protocol enables multiple access patterns
- Rust workspace scales well
- Session persistence enables resumption

### Gemini CLI

- Mono-agent with modular packages
- GEMINI.md for project context
- Skill injection into system prompt
- History compression with XML snapshots

**Lessons learned:**
- Project-specific context files are valuable
- Compression prompts need injection protection

---

## 18. Async Operations & Sub-Agents

Meerkat treats async operations and sub-agents as **first-class features**, not bolt-on additions. This section describes the unified operation model that enables parallel tool execution, sub-agent spawning, forking, and inter-agent communication.

### 18.1 Design Philosophy

**Key insight**: LLM APIs are stateless. We send the full message history every call. This means:
- **Forking is trivial** — just copy the message array
- **Parallelism is free** — independent operations make concurrent API calls
- **Turn-based control** — all operations synchronize at turn boundaries

**Everything is turn-based**: Operations are checked, steering is applied, and results are injected at turn boundaries (each iteration of the core loop). This provides predictable, deterministic behavior.

### 18.2 Operation Abstraction

All async work is unified under the `Operation` abstraction with a two-level type system:

```rust
/// What kind of work the operation performs
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    /// MCP or internal tool call
    ToolCall,
    /// Shell command execution
    ShellCommand,
    /// Sub-agent (spawn or fork)
    SubAgent,
}

/// Shape of the operation's result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultShape {
    /// Single result value
    Single,
    /// Streaming output (progress events)
    Stream,
    /// Multiple results (e.g., fork branches)
    Batch,
}

/// Complete operation specification
#[derive(Debug, Clone)]
pub struct OperationSpec {
    pub id: OperationId,
    pub kind: WorkKind,
    pub result_shape: ResultShape,
    pub policy: OperationPolicy,
    pub budget_reservation: BudgetLimits,
    pub depth: u32,
    pub depends_on: Vec<OperationId>,
    pub context: Option<ContextStrategy>,  // For SubAgent
    pub tool_access: Option<ToolAccessPolicy>,  // For SubAgent
}
```

### 18.3 Fork vs Spawn

Two patterns for creating sub-agents:

| Pattern | Context | Use Case |
|---------|---------|----------|
| **Fork** | Full conversation history | Branch current reasoning, try alternatives |
| **Spawn** | Minimal/fresh context | Independent subtask, clean slate |

```rust
/// How much context a sub-agent receives
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ContextStrategy {
    /// Complete conversation history (Fork default)
    FullHistory,
    /// Last N turns from parent
    LastTurns(u32),
    /// Compressed summary of conversation
    Summary { max_tokens: u32 },
    /// Explicit message list
    Custom { messages: Vec<Message> },
}
```

**Fork creates N operations** — each branch is its own operation with its own budget allocation.

**Nesting rules**:
- Spawned agents can fork (if depth allows)
- Forked agents can spawn (if depth allows)
- Depth limits enforced globally

### 18.4 State Machine

The core loop uses an explicit state machine:

```rust
/// States of the core agent loop
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopState {
    /// Waiting for LLM response
    CallingLlm,
    /// No LLM work, waiting for operation completions
    WaitingForOps,
    /// Processing buffered operation events
    DrainingEvents,
    /// Cleanup on interrupt or budget exhaustion
    Cancelling,
    /// Retry logic for transient LLM failures
    ErrorRecovery,
    /// Terminal state
    Completed,
}
```

**State transitions**:

```
                    ┌─────────────────────────────────────┐
                    │                                     │
                    ▼                                     │
┌─────────────┐   tools   ┌─────────────────┐           │
│ CallingLlm  │──────────►│ DrainingEvents  │───────────┤
└──────┬──────┘           └────────┬────────┘           │
       │                           │                     │
       │ EndTurn                   │ has_ops?           │
       │ (no ops)                  │                     │
       │                    ┌──────┴──────┐             │
       │                    │             │             │
       │                    ▼             ▼             │
       │            ┌─────────────┐ ┌─────────────┐     │
       │            │ CallingLlm  │ │WaitingForOps│     │
       │            └─────────────┘ └──────┬──────┘     │
       │                                   │             │
       │                                   │ op_complete │
       │                                   │             │
       │                            ┌──────┴──────┐     │
       │                            │DrainingEvents│────┘
       │                            └─────────────┘
       │
       ▼
┌─────────────┐
│  Completed  │
└─────────────┘

Interrupt/Budget ──► Cancelling ──► Completed
LLM Error ──► ErrorRecovery ──► CallingLlm (retry) or Completed (fail)
```

### 18.5 Operation Manager

Central component for tracking in-flight operations:

```rust
pub struct OpManager {
    /// In-flight operations
    inflight: HashMap<OperationId, OperationState>,
    /// Event channel receiver
    op_rx: mpsc::Receiver<OpEvent>,
    /// Event channel sender (cloned to operations)
    op_tx: mpsc::Sender<OpEvent>,
    /// Concurrency limits
    limits: ConcurrencyLimits,
    /// Budget pool for resource allocation
    budget_pool: BudgetPool,
}

#[derive(Debug, Clone)]
pub struct ConcurrencyLimits {
    /// Maximum sub-agent nesting depth
    pub max_depth: u32,
    /// Maximum concurrent operations (all types)
    pub max_concurrent_ops: usize,
    /// Maximum concurrent sub-agents specifically
    pub max_concurrent_agents: usize,
    /// Maximum children per parent agent
    pub max_children_per_agent: usize,
}

impl Default for ConcurrencyLimits {
    fn default() -> Self {
        Self {
            max_depth: 3,
            max_concurrent_ops: 32,
            max_concurrent_agents: 8,
            max_children_per_agent: 5,
        }
    }
}
```

### 18.6 Operation Events

Operations communicate via events:

```rust
/// Events from operations
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OpEvent {
    /// Operation started executing
    Started {
        id: OperationId,
        kind: WorkKind,
    },

    /// Progress update (for streaming operations)
    Progress {
        id: OperationId,
        message: String,
        percent: Option<f32>,
    },

    /// Operation completed successfully
    Completed {
        id: OperationId,
        result: OperationResult,
    },

    /// Operation failed
    Failed {
        id: OperationId,
        error: String,
    },

    /// Operation was cancelled
    Cancelled {
        id: OperationId,
    },

    /// Steering message was applied
    SteeringApplied {
        id: OperationId,
        steering_id: Uuid,
    },
}
```

**Event buffering**: Events are buffered during LLM calls and drained at turn boundaries.
- Buffer size: configurable (default 128)
- Overflow policy: backpressure (sender blocks, operation continues)

### 18.7 Dependency Management

Operations can depend on other operations:

```rust
pub struct OperationSpec {
    // ...
    /// Operations that must complete before this one starts
    pub depends_on: Vec<OperationId>,
    // ...
}

/// What happens when a dependency fails
#[derive(Debug, Clone, Default)]
pub enum DependencyFailurePolicy {
    /// Cancel dependent operations (synthetic cancelled result)
    CancelDependents,
    /// Fail dependent operations (synthetic error result)
    #[default]
    FailDependents,
}
```

**Rules**:
- Operations with dependencies don't start until all dependencies complete successfully
- If a dependency fails, the policy determines the dependent's fate
- If a dependency is cancelled, it's treated as failure
- Dependencies are checked at turn boundaries

### 18.8 Result Injection

How operation results enter the conversation:

```rust
/// Policy for injecting results into session
#[derive(Debug, Clone)]
pub enum InjectionPolicy {
    /// Inject as results complete (with causal metadata)
    CompletionOrder,
    /// Inject in submission order (may delay some results)
    SubmissionOrder,
    /// Batch all results, inject together
    BatchAfterAll,
}

/// Causal metadata attached to injected results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpCausality {
    pub op_id: OperationId,
    pub parent_op_id: Option<OperationId>,
    pub submitted_at: SystemTime,
    pub completed_at: SystemTime,
    pub depends_on: Vec<OperationId>,
    pub attempt: u32,
    pub status: OpStatus,
}
```

**Message types for results**:
- Tool operations: `Message::ToolResults` (preserves LLM expectations)
- Non-tool operations: `Message::OpResults` (fork, spawn, shell)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpResultMessage {
    pub id: OperationId,
    pub label: Option<String>,
    pub status: OpStatus,
    pub content: OpContent,
    pub causality: OpCausality,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OpContent {
    /// Full result inline
    Inline(String),
    /// Summary with artifact reference
    Summary { summary: String, artifact_id: String },
    /// Handle only (large results)
    Handle { artifact_id: String, hint: String },
}
```

### 18.9 Budget Pool

Dynamic budget allocation with reclamation:

```rust
pub struct BudgetPool {
    /// Total budget for the session
    total: BudgetLimits,
    /// Currently used (not reserved)
    used: BudgetUsage,
    /// Reserved for in-flight operations
    reservations: HashMap<OperationId, BudgetLimits>,
}

impl BudgetPool {
    /// Reserve budget for an operation
    pub fn reserve(&mut self, op_id: OperationId, limits: BudgetLimits) -> Result<(), BudgetError>;

    /// Release reservation when operation completes
    pub fn release(&mut self, op_id: &OperationId, actual_usage: BudgetUsage);

    /// Check if budget allows starting an operation
    pub fn can_reserve(&self, limits: &BudgetLimits) -> bool;
}
```

**Reclamation rules**:
- Budget is reclaimed **immediately on operation completion**
- Unused reservation returns to pool
- Time budget is wall-clock (cannot be reclaimed)
- Token/call budgets can be partially reclaimed

**Fork budget allocation**:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ForkBudgetPolicy {
    /// Split remaining budget equally among branches
    Equal,
    /// Allocate proportionally based on estimated work
    Proportional,
    /// Fixed token allocation per branch
    Fixed(u64),
    /// Give remaining budget to each branch (for sequential)
    Remaining,
}
```

### 18.10 Artifact Store

Large operation results are stored as artifacts:

```rust
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Store artifact, return reference
    async fn put(&self, data: Vec<u8>, ttl: Duration) -> Result<ArtifactRef, StoreError>;

    /// Retrieve artifact by ID
    async fn get(&self, id: &str) -> Result<Vec<u8>, StoreError>;

    /// Delete artifact
    async fn delete(&self, id: &str) -> Result<(), StoreError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ArtifactRef {
    /// Unique artifact identifier
    pub id: String,
    /// Session this artifact belongs to
    pub session_id: SessionId,
    /// Size in bytes
    pub size_bytes: u64,
    /// Time-to-live in seconds (None = permanent)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,
    /// Format version for migrations
    pub version: u32,
}
```

**Lifecycle**:
- Created at operation completion if result exceeds threshold
- Default TTL: 24 hours (configurable)
- Scoped to session (session_id prefix in artifact_id)
- Forks get read access to parent session's artifacts
- Background sweep for TTL expiry

**Limits**:
- `max_artifact_bytes`: Maximum size per artifact
- `max_artifacts_per_session`: Maximum artifacts per session
- `max_total_artifact_bytes`: Global storage limit

### 18.11 Tool Access for Sub-Agents

Control which tools sub-agents can use:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ToolAccessPolicy {
    /// Inherit parent's tool registry
    #[default]
    Inherit,
    /// Only these tools allowed
    AllowList(Vec<String>),
    /// All except these tools
    DenyList(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub prompt: String,
    pub context: ContextStrategy,
    pub tool_access: ToolAccessPolicy,
    /// If false, sub-agent cannot spawn/fork further
    pub allow_spawn: bool,
}
```

**Validation**: Tool access lists are validated at spawn time. Invalid tool names cause immediate spawn failure.

**Internal operation tools** (`spawn_agent`, `fork`, `await_ops`, `fetch_result`, `op_status`, `cancel_ops`) follow the same policy — they're in the tool registry like any other tool.

### 18.12 Sub-Agent Steering

Parents can send steering messages to running sub-agents:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                            Steering Flow                                     │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  Parent Agent                              Sub-Agent                         │
│       │                                         │                            │
│       │  steer(op_id, "Focus on auth")          │                            │
│       │────────────────────────────────────────►│ [Steering queued]          │
│       │                                         │                            │
│       │                                         │ ... working on turn ...    │
│       │                                         │ ... tool calls ...         │
│       │                                         │                            │
│       │                                         │ [Turn boundary]            │
│       │                                         │ Check steering queue       │
│       │                                         │ Inject as User message     │
│       │                                         │                            │
│       │  SteeringApplied event                  │                            │
│       │◄────────────────────────────────────────│                            │
│       │                                         │                            │
│       │                                         │ LLM sees:                  │
│       │                                         │ "[STEERING] Focus on auth" │
│       │                                         │ Adjusts behavior           │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

**API**:

```rust
/// Send steering message to a running sub-agent
pub fn steer(op_id: OperationId, message: String) -> Result<SteeringHandle, SteerError>;

pub struct SteeringHandle {
    pub id: Uuid,
    pub seq: u64,
}

impl SteeringHandle {
    pub fn status(&self) -> SteeringStatus;
}

#[derive(Debug, Clone, PartialEq)]
pub enum SteeringStatus {
    /// In sub-agent's queue, not yet applied
    Queued,
    /// Injected into session before LLM call
    Applied,
    /// Sub-agent completed before steering applied
    Expired,
    /// Sub-agent crashed or was cancelled
    Failed,
}
```

**Message injection**: Steering is injected as a `User` message with a prefix:

```rust
Message::User(UserMessage {
    content: format!("[STEERING] {}", steering.content),
})
```

**Sub-agent system prompt** includes:

```
Messages prefixed with [STEERING] are control directives from your parent agent.
Treat them as high-priority instructions and adjust your plan accordingly.
Do not ask follow-up questions unless required to comply.
```

**Timing**: Steering is applied at **turn boundaries only** (before next LLM call). If the sub-agent is mid-tool-execution, steering waits until tools complete.

**Queue semantics**:
- FIFO queue per sub-agent
- All pending messages applied in order
- Bounded queue depth (default 16)
- Overflow: drop oldest (configurable)

**Semantics**:
- **Best-effort**: LLM may choose to ignore steering
- **Not immediate**: Waits for turn boundary
- **Not enforced**: No compliance verification in v0.1

**For guaranteed redirection**, use cancel + respawn:

```rust
cancel_ops(vec![op_id]).await;
spawn_agent(SpawnSpec { prompt: "New focused task".into(), ... }).await;
```

### 18.13 Internal Operation Tools

Tools exposed to the LLM for async operations:

```rust
// Spawn a fresh sub-agent
pub fn spawn_agent(spec: SpawnSpec) -> OperationHandle;

// Fork current conversation into parallel branches
pub fn fork(branches: Vec<ForkBranch>) -> Vec<OperationHandle>;

// Wait for operations to complete
pub fn await_ops(
    op_ids: Vec<OperationId>,
    mode: AwaitMode,
    timeout: Option<Duration>,
) -> AwaitResult;

// Check operation status without waiting
pub fn op_status(op_ids: Vec<OperationId>) -> Vec<OpStatusResult>;

// Cancel running operations
pub fn cancel_ops(op_ids: Vec<OperationId>) -> Vec<CancelResult>;

// Fetch full result of artifact-stored operation
pub fn fetch_result(artifact_id: String) -> String;

// Send steering to running sub-agent
pub fn steer(op_id: OperationId, message: String) -> SteeringHandle;

#[derive(Debug, Clone)]
pub enum AwaitMode {
    /// Wait for all operations
    All,
    /// Wait for any one operation
    Any,
    /// Wait for first successful completion
    FirstSuccess,
}
```

### 18.14 Enhanced Core Loop

The complete loop with async operations:

```rust
impl Agent {
    async fn run_loop(&mut self, session: &mut Session) -> Result<RunResult, AgentError> {
        let mut state = LoopState::CallingLlm;

        loop {
            match state {
                LoopState::CallingLlm => {
                    // Check budget
                    self.budget_pool.check()?;

                    // Call LLM with retry
                    match self.call_llm_with_retry(session).await {
                        Ok(response) => {
                            session.push(response.to_assistant_message());

                            match response.stop_reason {
                                StopReason::EndTurn => {
                                    if self.op_manager.is_empty() {
                                        state = LoopState::Completed;
                                    } else {
                                        state = LoopState::WaitingForOps;
                                    }
                                }
                                StopReason::ToolUse => {
                                    self.dispatch_tool_calls(&response.tool_calls).await;
                                    state = LoopState::DrainingEvents;
                                }
                                StopReason::MaxTokens => return Err(AgentError::MaxTokensReached),
                                StopReason::ContentFilter => return Err(AgentError::ContentFiltered),
                                StopReason::Cancelled => return Err(AgentError::Cancelled),
                            }
                        }
                        Err(e) if e.is_retryable() => {
                            state = LoopState::ErrorRecovery;
                        }
                        Err(e) => return Err(e.into()),
                    }
                }

                LoopState::WaitingForOps => {
                    tokio::select! {
                        // Wait for operation events
                        Some(event) = self.op_manager.op_rx.recv() => {
                            self.buffer_event(event);
                            state = LoopState::DrainingEvents;
                        }
                        // Periodic timeout check
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {
                            self.op_manager.check_timeouts();
                        }
                        // External cancellation
                        _ = self.cancel_token.cancelled() => {
                            state = LoopState::Cancelling;
                        }
                    }
                }

                LoopState::DrainingEvents => {
                    // Drain all buffered events
                    while let Ok(event) = self.op_manager.op_rx.try_recv() {
                        self.buffer_event(event);
                    }

                    // Check steering for sub-agents we're running
                    self.apply_pending_steering();

                    // Inject results into session
                    let results = self.collect_completed_results();
                    if !results.is_empty() {
                        session.push(Message::OpResults(results));
                    }

                    // Decide next state
                    if self.has_pending_llm_work(session) {
                        state = LoopState::CallingLlm;
                    } else if self.op_manager.has_inflight() {
                        state = LoopState::WaitingForOps;
                    } else {
                        state = LoopState::Completed;
                    }
                }

                LoopState::Cancelling => {
                    self.op_manager.cancel_all().await;
                    return Err(AgentError::Cancelled);
                }

                LoopState::ErrorRecovery => {
                    // Apply retry policy
                    match self.retry_policy.wait_and_check().await {
                        RetryDecision::Retry => state = LoopState::CallingLlm,
                        RetryDecision::GiveUp(e) => return Err(e.into()),
                    }
                }

                LoopState::Completed => {
                    break;
                }
            }

            // Checkpoint if needed
            self.maybe_checkpoint(session).await?;
        }

        Ok(self.build_result(session))
    }
}
```

### 18.15 Limitations and Future Work

**v0.1 Limitations**:

| Limitation | Rationale |
|------------|-----------|
| Turn-boundary steering only | Mid-turn interruption requires complex cancel semantics |
| Best-effort steering compliance | Enforcing LLM compliance is non-trivial |
| No mid-tool cancellation for MCP | MCP protocol doesn't support cancellation |
| SessionScoped artifacts only | Global scope adds complexity |
| No composite operations (race, map) | Implement in SDK layer using spawn + await |

**Planned for v0.2+**:

| Feature | Description |
|---------|-------------|
| Composite operations | Built-in race, map, ensemble patterns |
| Bidirectional dialogue | Sub-agent asks parent for clarification |
| Priority preemption | Urgent steering interrupts tool dispatch |
| Streaming progress to session | Opt-in progress message injection |
| Global artifact scope | Shared artifacts across sessions |
| Compliance verification | Detect if sub-agent followed steering |

---

## 19. Future Considerations

### Planned Features

| Feature | Priority | Status | Notes |
|---------|----------|--------|-------|
| Context compaction | High | Partial | `ContextStrategy::Summary` designed (§18.3); automatic triggering TBD |
| Streaming tool results | Medium | Designed | `ResultShape::Stream` and event buffering in place (§18.6) |
| SQLite store | Medium | Future | Better for large session counts |
| OpenTelemetry | Medium | Future | Distributed tracing |
| Multi-agent orchestration | Medium | Designed | Fork/spawn/steering in place (§18); higher-level patterns TBD |
| WASM support | Low | Future | Run in browser |

### Potential Integrations

| Integration | Notes |
|-------------|-------|
| LangSmith | Tracing and evaluation |
| Weights & Biases | Experiment tracking |
| Prometheus | Metrics export |
| Grafana | Dashboards |

### API Stability

- `meerkat-core` types: Stable after 1.0
- `meerkat-client` trait: Stable after 1.0
- Event stream: May evolve, versioned
- Config format: Backward compatible migrations

---

## Appendix A: Glossary

| Term | Definition |
|------|------------|
| **Agent** | An LLM-powered executor that can use tools |
| **Turn** | One request-response cycle with the LLM (one iteration of the core loop) |
| **Tool** | A function the agent can invoke |
| **MCP** | Model Context Protocol, standard for tool servers |
| **Session** | A conversation with full history |
| **Checkpoint** | Saved session state for resumption |
| **Budget** | Resource limits (tokens, time, calls) |
| **Operation** | Any async work unit: tool call, shell command, or sub-agent |
| **Fork** | Create sub-agent(s) with full conversation history |
| **Spawn** | Create sub-agent with minimal/fresh context |
| **Steering** | Parent-to-child message redirecting a running sub-agent |
| **Turn Boundary** | Safe point between turns where steering and results are processed |
| **Artifact** | Large operation result stored externally with TTL |
| **BudgetPool** | Dynamic budget allocation system with reservation/reclamation |

## Appendix B: Example Session Flow

```
User: "Find all TODO comments in src/ and create issues for them"

Turn 1:
  Agent → LLM: [system prompt] + [user message] + [tools]
  LLM → Agent: "I'll search for TODO comments" + ToolCall(grep, {pattern: "TODO", path: "src/"})
  Agent → MCP: grep tool call
  MCP → Agent: [list of TODOs with file:line]

Turn 2:
  Agent → LLM: [history] + [tool result]
  LLM → Agent: "Found 5 TODOs, creating issues" + ToolCall(github_create_issue, {...}) x5
  Agent → MCP: github tool calls (parallel)
  MCP → Agent: [issue URLs]

Turn 3:
  Agent → LLM: [history] + [tool results]
  LLM → Agent: "Created 5 issues: [links]" + StopReason::EndTurn

Result: { text: "Created 5 issues...", turns: 3, tool_calls: 6 }
```

---

*End of Design Document*
