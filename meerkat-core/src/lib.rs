//! meerkat-core - Core agent logic for Meerkat (no I/O deps)
//!
//! This crate contains all the core types, traits, and logic for Meerkat agents.
//! It is intentionally free of I/O dependencies to enable easy testing and embedding.

pub mod agent;
pub mod budget;
pub mod compact;
pub mod config;
pub mod config_store;
pub mod error;
pub mod event;
pub mod gateway;
pub mod hooks;
pub mod mcp_config;
pub mod memory;
pub mod ops;
pub mod prompt;
pub mod provider;
pub mod retry;
pub mod schema;
pub mod service;
pub mod session;
pub mod state;
pub mod sub_agent;
pub mod turn_boundary;
pub mod types;

// Re-export main types at crate root
pub use agent::{
    Agent, AgentBuilder, AgentLlmClient, AgentRunner, AgentSessionStore, AgentToolDispatcher,
    CommsRuntime, FilteredToolDispatcher, LlmStreamResult,
};
pub use budget::{Budget, BudgetLimits, BudgetPool};
pub use compact::{CompactionConfig, CompactionContext, CompactionResult, Compactor};
pub use memory::{MemoryMetadata, MemoryResult, MemoryStore, MemoryStoreError};

pub use config::{
    AgentConfig, BudgetConfig, CommsRuntimeConfig, CommsRuntimeMode, Config, ConfigDelta,
    ConfigScope, HookEntryConfig, HookRunOverrides, HookRuntimeConfig, HooksConfig, LimitsConfig,
    ModelDefaults, ProviderConfig, ProviderSettings, ResolvedSubAgentConfig, RetryConfig,
    ShellDefaults, StorageConfig, StoreConfig, SubAgentsConfig, ToolsConfig,
};
pub use config_store::{ConfigStore, FileConfigStore, MemoryConfigStore};
pub use error::{AgentError, ToolError};
pub use event::{
    AgentEvent, BudgetType, VerboseEventConfig, format_verbose_event,
    format_verbose_event_with_config,
};
pub use gateway::{Availability, AvailabilityCheck, ToolGateway, ToolGatewayBuilder};
pub use hooks::{
    HookCapability, HookDecision, HookEngine, HookEngineError, HookExecutionMode,
    HookExecutionReport, HookFailurePolicy, HookId, HookInvocation, HookLlmRequest,
    HookLlmResponse, HookOutcome, HookPatch, HookPatchEnvelope, HookPoint, HookReasonCode,
    HookRevision, HookToolCall, HookToolResult, default_failure_policy,
};
pub use mcp_config::{McpConfig, McpConfigError, McpScope, McpServerConfig, McpServerWithScope};
pub use ops::{
    ConcurrencyLimits, ContextStrategy, ForkBranch, ForkBudgetPolicy, OpEvent, OperationId,
    OperationPolicy, OperationResult, OperationSpec, ResultShape, SpawnSpec, SubAgentState,
    ToolAccessPolicy, WorkKind,
};
pub use prompt::{AGENTS_MD_MAX_BYTES, DEFAULT_SYSTEM_PROMPT, SystemPromptConfig};
pub use provider::Provider;
pub use retry::RetryPolicy;
pub use schema::{
    CompiledSchema, MeerkatSchema, SchemaCompat, SchemaError, SchemaFormat, SchemaWarning,
};
pub use service::{
    CreateSessionRequest, SessionError, SessionInfo, SessionQuery, SessionService, SessionSummary,
    SessionUsage, SessionView, StartTurnRequest,
};
pub use session::{SESSION_VERSION, Session, SessionMeta, SessionMetadata, SessionTooling};
pub use state::LoopState;
pub use sub_agent::{SubAgentCommsInfo, SubAgentCompletion, SubAgentInfo, SubAgentManager};
pub use turn_boundary::{TurnBoundaryHook, TurnBoundaryMessage};
pub use types::{
    ArtifactRef, AssistantBlock, AssistantMessage, BlockAssistantMessage, Message, OutputSchema,
    ProviderMeta, RunResult, SecurityMode, SessionId, StopReason, SystemMessage, ToolCall,
    ToolCallIter, ToolCallView, ToolDef, ToolResult, Usage, UserMessage,
};
