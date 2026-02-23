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
    CompiledSchema,
    ConcurrencyLimits,
    // Config
    Config,
    ConfigDelta,
    ContextStrategy,
    ForkBranch,
    ForkBudgetPolicy,
    HookCapability,
    HookDecision,
    HookEngine,
    HookEngineError,
    HookEntryConfig,
    HookExecutionMode,
    HookExecutionReport,
    HookFailurePolicy,
    HookId,
    HookInvocation,
    HookOutcome,
    HookPatch,
    HookPatchEnvelope,
    HookPoint,
    HookReasonCode,
    HookRevision,
    HookRunOverrides,
    HookRuntimeConfig,
    HooksConfig,
    // Interaction types
    InteractionContent,
    InteractionId,
    LlmStreamResult,
    // State
    LoopState,
    MeerkatSchema,
    // Types
    Message,
    OpEvent,
    OperationId,
    OperationPolicy,
    OperationResult,
    OperationSpec,
    OutputSchema,
    // Provider
    Provider,
    ProviderConfig,
    ResponseStatus,
    ResultShape,
    RetryConfig,
    // Retry
    RetryPolicy,
    RunResult,
    SESSION_VERSION,
    SchemaCompat,
    SchemaError,
    SchemaFormat,
    SchemaWarning,
    // Session
    Session,
    SessionId,
    SessionMeta,
    SessionMetadata,
    SessionTooling,
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
#[cfg(feature = "comms")]
pub use meerkat_comms::agent::{CommsContent, CommsMessage, CommsStatus};
#[cfg(feature = "comms")]
pub use meerkat_comms::{CommsRuntime, CommsRuntimeError, CoreCommsConfig};
#[cfg(feature = "comms")]
pub use meerkat_core::{
    CommsCommand, EventStream, InputSource, InputStreamMode, PeerDirectoryEntry,
    PeerDirectorySource, PeerName, SendAndStreamError, SendError, SendReceipt, StreamError,
    StreamScope,
};

// Re-export client types
pub use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmResponse};
pub use meerkat_tools::ToolError;

// AgentFactory and build_agent types
mod factory;
pub use factory::{
    AgentBuildConfig, AgentFactory, BuildAgentError, DynAgent,
    decode_llm_client_override_from_service, encode_llm_client_override_for_service, provider_key,
};

// Factory-backed SessionService wiring
mod service_factory;
#[cfg(feature = "session-store")]
pub use service_factory::build_persistent_service;
pub use service_factory::{FactoryAgent, FactoryAgentBuilder, build_ephemeral_service};

// Session service
pub use meerkat_core::{
    CreateSessionRequest, SessionError, SessionInfo, SessionQuery, SessionService, SessionSummary,
    SessionUsage, SessionView, StartTurnRequest,
};
#[cfg(feature = "session-compaction")]
pub use meerkat_session::DefaultCompactor;
#[cfg(feature = "session-store")]
pub use meerkat_session::PersistentSessionService;
pub use meerkat_session::{EphemeralSessionService, SessionAgent, SessionAgentBuilder};

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

#[cfg(feature = "session-store")]
pub use meerkat_store::RedbSessionStore;

// Re-export tools
pub use meerkat_tools::{DispatchError, ToolDispatcher, ToolRegistry, ToolValidationError};

// Re-export builtin tools infrastructure
#[cfg(feature = "comms")]
pub use meerkat_tools::CommsToolSurface;
pub use meerkat_tools::{
    BuiltinTool, BuiltinToolConfig, BuiltinToolEntry, BuiltinToolError, CompositeDispatcher,
    CompositeDispatcherError, EnforcedToolPolicy, FileTaskStore, MemoryTaskStore,
    ResolvedToolPolicy, TaskStore, ToolMode, ToolPolicyLayer, ensure_rkat_dir, find_project_root,
};

// Re-export MCP client
#[cfg(feature = "mcp")]
pub use meerkat_mcp::{McpConnection, McpError, McpRouter, McpServerConfig};

// Skill types re-exports
pub use meerkat_core::skills::{
    SkillCollection, SkillDescriptor, SkillDocument, SkillFilter, SkillId, SkillIntrospectionEntry,
    SkillRuntime, SkillScope,
};

// Contracts re-exports
pub use meerkat_contracts::{
    self as contracts, CapabilitiesResponse, CapabilityHint, CapabilityId, CapabilityRegistration,
    CapabilityScope, CapabilityStatus, CommsParams, ContractVersion, CoreCreateParams,
    ErrorCategory, ErrorCode, HookParams, Protocol, SkillEntry, SkillInspectResponse,
    SkillListResponse, SkillsParams, StructuredOutputParams, WireError, WireEvent, WireRunResult,
    WireSessionInfo, WireSessionSummary, WireUsage, build_capabilities,
};

// Surface infrastructure
pub mod surface;

// Prompt assembly
mod prompt_assembly;
pub use prompt_assembly::assemble_system_prompt;

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
