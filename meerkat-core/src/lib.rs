//! meerkat-core - Core agent logic for Meerkat (no I/O deps)
//!
//! This crate contains all the core types, traits, and logic for Meerkat agents.
//! It is intentionally free of I/O dependencies to enable easy testing and embedding.

pub mod types;
pub mod event;
pub mod session;
pub mod budget;
pub mod retry;
pub mod config;
pub mod mcp_config;
pub mod prompt;
pub mod error;
pub mod ops;
pub mod state;
pub mod agent;
pub mod sub_agent;

// Re-export main types at crate root
pub use types::*;
pub use event::{AgentEvent, BudgetType};
pub use session::{Session, SessionMeta, SESSION_VERSION};
pub use budget::{Budget, BudgetPool, BudgetLimits};
pub use retry::RetryPolicy;
pub use config::{Config, AgentConfig, ProviderConfig, StorageConfig, BudgetConfig, RetryConfig};
pub use error::AgentError;
pub use ops::{
    WorkKind, ResultShape, OperationId, OperationSpec, OperationPolicy, OpEvent, OperationResult,
    ContextStrategy, ForkBudgetPolicy, ToolAccessPolicy, ConcurrencyLimits, SpawnSpec, ForkBranch,
    SteeringMessage, SteeringHandle, SteeringStatus, SubAgentState,
};
pub use state::LoopState;
pub use agent::{Agent, AgentBuilder, AgentLlmClient, AgentToolDispatcher, AgentSessionStore, LlmStreamResult};
pub use sub_agent::SubAgentManager;
pub use mcp_config::{McpConfig, McpServerConfig, McpScope, McpServerWithScope, McpConfigError};
pub use prompt::{SystemPromptConfig, DEFAULT_SYSTEM_PROMPT, AGENTS_MD_MAX_BYTES};
