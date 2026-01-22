//! RAIK - Rust Agentic Interface Kit
//!
//! A minimal, high-performance agent harness for LLM-powered applications.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use raik::prelude::*;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create an agent with Anthropic
//!     let result = raik::with_anthropic(std::env::var("ANTHROPIC_API_KEY")?)
//!         .model("claude-sonnet-4")
//!         .system_prompt("You are a helpful assistant.")
//!         .run("What is 2 + 2?")
//!         .await?;
//!
//!     println!("{}", result.text);
//!     Ok(())
//! }
//! ```

// Re-export core types
pub use raik_core::{
    // Types
    Message, UserMessage, AssistantMessage, SystemMessage,
    ToolCall, ToolResult, ToolDef,
    StopReason, Usage, RunResult,
    SessionId, ArtifactRef,
    // Session
    Session, SessionMeta, SESSION_VERSION,
    // Events
    AgentEvent, BudgetType,
    // Operations
    WorkKind, ResultShape, OperationId, OperationSpec, OperationPolicy, OpEvent,
    ContextStrategy, ForkBudgetPolicy, ToolAccessPolicy, ConcurrencyLimits,
    OperationResult, SpawnSpec, ForkBranch,
    SteeringMessage, SteeringHandle, SteeringStatus, SubAgentState,
    // State
    LoopState,
    // Config
    Config, AgentConfig, ProviderConfig, StorageConfig, BudgetConfig, RetryConfig,
    // Budget
    Budget, BudgetPool, BudgetLimits,
    // Retry
    RetryPolicy,
    // Errors
    AgentError,
    // Agent
    Agent, AgentBuilder, AgentLlmClient, AgentToolDispatcher, AgentSessionStore, LlmStreamResult,
    // Sub-agents
    SubAgentManager,
};

// Re-export client types
pub use raik_client::{
    LlmClient, LlmEvent, LlmError, LlmRequest, LlmResponse,
};

#[cfg(feature = "anthropic")]
pub use raik_client::AnthropicClient;

#[cfg(feature = "openai")]
pub use raik_client::OpenAiClient;

#[cfg(feature = "gemini")]
pub use raik_client::GeminiClient;

// Re-export store types
pub use raik_store::{
    SessionStore, SessionFilter, StoreError,
};

#[cfg(feature = "jsonl-store")]
pub use raik_store::JsonlStore;

#[cfg(feature = "memory-store")]
pub use raik_store::MemoryStore;

// Re-export tools
pub use raik_tools::{
    ToolRegistry, ToolDispatcher,
    ToolError, ToolValidationError, DispatchError,
};

// Re-export MCP client
pub use raik_mcp_client::{
    McpRouter, McpError, McpServerConfig, McpConnection,
};

// SDK module
mod sdk;
pub use sdk::*;

/// Prelude module for convenient imports
pub mod prelude {
    pub use super::{
        Message, UserMessage, AssistantMessage, SystemMessage,
        ToolCall, ToolResult, ToolDef,
        StopReason, Usage, RunResult,
        SessionId, Session, SessionMeta,
        AgentEvent, BudgetType,
        Config, AgentConfig,
        Budget, BudgetLimits,
        RetryPolicy,
        AgentError,
        LlmClient, LlmEvent, LlmError, LlmRequest,
        SessionStore, SessionFilter,
    };

    #[cfg(feature = "anthropic")]
    pub use super::AnthropicClient;

    #[cfg(feature = "openai")]
    pub use super::OpenAiClient;

    #[cfg(feature = "gemini")]
    pub use super::GeminiClient;

    // SDK helpers
    #[cfg(feature = "anthropic")]
    pub use super::with_anthropic;

    #[cfg(feature = "openai")]
    pub use super::with_openai;

    #[cfg(feature = "gemini")]
    pub use super::with_gemini;
}
