//! Meerkat - Rust Agentic Interface Kit
//!
//! A minimal, high-performance agent harness for LLM-powered applications.
//!
//! # Quick Start
//!
//! ```text
//! use meerkat::prelude::*;
//! use meerkat::AgentFactory;
//! use meerkat::AnthropicClient;
//! use meerkat_store::{JsonlStore, StoreAdapter};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let api_key = std::env::var("ANTHROPIC_API_KEY")?;
//!     let factory = AgentFactory::new(std::path::PathBuf::from(".rkat/sessions"));
//!
//!     let client = Arc::new(AnthropicClient::new(api_key));
//!     let llm = factory.build_llm_adapter(client, "claude-sonnet-4");
//!     let store = Arc::new(JsonlStore::new(&factory.store_path)?);
//!     let store = Arc::new(StoreAdapter::new(store));
//!     let tools = Arc::new(meerkat_tools::EmptyToolDispatcher::default());
//!
//!     let mut agent = AgentBuilder::new()
//!         .model("claude-sonnet-4")
//!         .build(Arc::new(llm), tools, store);
//!     let result = agent.run("What is 2 + 2?".to_string()).await?;
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
    // Gateway for composing dispatchers
    Availability,
    AvailabilityCheck,
    // Budget
    Budget,
    BudgetConfig,
    BudgetLimits,
    BudgetPool,
    BudgetType,
    ConcurrencyLimits,
    // Config
    Config,
    ConfigDelta,
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
    OutputSchema,
    SchemaCompat,
    SchemaError,
    SchemaFormat,
    SchemaWarning,
    MeerkatSchema,
    CompiledSchema,
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
    StopReason,
    StorageConfig,
    // Sub-agents
    SubAgentManager,
    SubAgentState,
    SystemMessage,
    ToolAccessPolicy,
    ToolCall,
    ToolDef,
    ToolGateway,
    ToolGatewayBuilder,
    ToolResult,
    Usage,
    UserMessage,
    // Operations
    WorkKind,
};

// Re-export comms types from meerkat_comms
pub use meerkat_comms::agent::{CommsContent, CommsMessage, CommsStatus};
pub use meerkat_comms::{CommsRuntime, CommsRuntimeError, CoreCommsConfig};

// Re-export client types
pub use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmResponse};
pub use meerkat_tools::ToolError;

// AgentFactory
mod factory;
pub use factory::AgentFactory;

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
    BuiltinTool, BuiltinToolConfig, BuiltinToolEntry, BuiltinToolError, CommsToolSurface,
    CompositeDispatcher, CompositeDispatcherError, EnforcedToolPolicy, FileTaskStore,
    MemoryTaskStore, ResolvedToolPolicy, TaskStore, ToolMode, ToolPolicyLayer, ensure_rkat_dir,
    find_project_root,
};

// Re-export MCP client
pub use meerkat_mcp::{McpConnection, McpError, McpRouter, McpServerConfig};

// SDK module
mod sdk;
pub use sdk::*;
mod sdk_config;
pub use sdk_config::SdkConfigStore;

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
}
