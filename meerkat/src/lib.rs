//! Meerkat - Rust Agentic Interface Kit
//!
//! A minimal, high-performance agent harness for LLM-powered applications.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use meerkat::prelude::*;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create an agent with Anthropic
//!     let result = meerkat::with_anthropic(std::env::var("ANTHROPIC_API_KEY")?)
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
pub use meerkat_core::{
    // Agent
    Agent,
    AgentBuilder,
    AgentConfig,
    // Errors
    AgentError,
    // Events
    AgentEvent,
    AgentLlmClient,
    AgentSessionStore,
    AgentToolDispatcher,
    ArtifactRef,
    AssistantMessage,
    // Budget
    Budget,
    BudgetConfig,
    BudgetLimits,
    BudgetPool,
    BudgetType,
    ConcurrencyLimits,
    // Config
    Config,
    ContextStrategy,
    ForkBranch,
    ForkBudgetPolicy,
    LlmStreamResult,
    // State
    LoopState,
    // Types
    Message,
    OpEvent,
    OperationId,
    OperationPolicy,
    OperationResult,
    OperationSpec,
    ProviderConfig,
    ResultShape,
    RetryConfig,
    // Retry
    RetryPolicy,
    RunResult,
    SESSION_VERSION,
    // Session
    Session,
    SessionId,
    SessionMeta,
    SpawnSpec,
    SteeringHandle,
    SteeringMessage,
    SteeringStatus,
    StopReason,
    StorageConfig,
    // Sub-agents
    SubAgentManager,
    SubAgentState,
    SystemMessage,
    ToolAccessPolicy,
    ToolCall,
    ToolDef,
    ToolError,
    ToolResult,
    Usage,
    UserMessage,
    // Operations
    WorkKind,
};

// Re-export client types
pub use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest, LlmResponse};

#[cfg(feature = "anthropic")]
pub use meerkat_client::AnthropicClient;

#[cfg(feature = "openai")]
pub use meerkat_client::OpenAiClient;

#[cfg(feature = "gemini")]
pub use meerkat_client::GeminiClient;

// Re-export store types
pub use meerkat_store::{SessionFilter, SessionStore, StoreError};

#[cfg(feature = "jsonl-store")]
pub use meerkat_store::JsonlStore;

#[cfg(feature = "memory-store")]
pub use meerkat_store::MemoryStore;

// Re-export tools
pub use meerkat_tools::{DispatchError, ToolDispatcher, ToolRegistry, ToolValidationError};

// Re-export builtin tools infrastructure
pub use meerkat_tools::{
    BuiltinTool, BuiltinToolConfig, BuiltinToolEntry, BuiltinToolError, CompositeDispatcher,
    CompositeDispatcherError, EnforcedToolPolicy, FileTaskStore, MemoryTaskStore,
    ResolvedToolPolicy, TaskStore, ToolMode, ToolPolicyLayer, ensure_rkat_dir, find_project_root,
};

// Re-export MCP client
pub use meerkat_mcp_client::{McpConnection, McpError, McpRouter, McpServerConfig};

// SDK module
mod sdk;
pub use sdk::*;

/// Prelude module for convenient imports
pub mod prelude {
    pub use super::{
        AgentConfig, AgentError, AgentEvent, AssistantMessage, Budget, BudgetLimits, BudgetType,
        Config, LlmClient, LlmError, LlmEvent, LlmRequest, Message, RetryPolicy, RunResult,
        Session, SessionFilter, SessionId, SessionMeta, SessionStore, StopReason, SystemMessage,
        ToolCall, ToolDef, ToolResult, Usage, UserMessage,
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
