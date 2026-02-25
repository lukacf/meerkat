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

// On wasm32, provide tokio alias backed by tokio_with_wasm.
#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

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

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub use meerkat_store::RedbSessionStore;

// Re-export tools
#[cfg(not(target_arch = "wasm32"))]
pub use meerkat_tools::{DispatchError, ToolDispatcher, ToolRegistry, ToolValidationError};

// Re-export builtin tools infrastructure
#[cfg(feature = "comms")]
pub use meerkat_tools::CommsToolSurface;
pub use meerkat_tools::{
    BuiltinTool, BuiltinToolConfig, BuiltinToolEntry, BuiltinToolError, CompositeDispatcher,
    CompositeDispatcherError, EnforcedToolPolicy, MemoryTaskStore,
    ResolvedToolPolicy, TaskStore, ToolMode, ToolPolicyLayer,
};
#[cfg(not(target_arch = "wasm32"))]
pub use meerkat_tools::{FileTaskStore, ensure_rkat_dir, find_project_root};

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

// Prompt assembly (filesystem-dependent: reads AGENTS.md, system_prompt_file)
#[cfg(not(target_arch = "wasm32"))]
mod prompt_assembly;
#[cfg(not(target_arch = "wasm32"))]
pub use prompt_assembly::assemble_system_prompt;

// SDK helpers (filesystem-dependent: .rkat dir, hook config files)
#[cfg(not(target_arch = "wasm32"))]
mod sdk;
#[cfg(not(target_arch = "wasm32"))]
pub use sdk::*;
#[cfg(not(target_arch = "wasm32"))]
mod sdk_config;
#[cfg(not(target_arch = "wasm32"))]
pub use sdk_config::SdkConfigStore;

// Comms tool composition (available on all platforms including wasm32)
#[cfg(feature = "comms")]
pub fn compose_tools_with_comms(
    base_tools: std::sync::Arc<dyn meerkat_core::AgentToolDispatcher>,
    tool_usage_instructions: String,
    runtime: &meerkat_comms::CommsRuntime,
) -> Result<
    (std::sync::Arc<dyn meerkat_core::AgentToolDispatcher>, String),
    meerkat_tools::ToolError,
> {
    use meerkat_tools::CommsToolSurface;
    use std::sync::Arc;
    let router = runtime.router_arc();
    let trusted_peers = runtime.trusted_peers_shared();
    let self_pubkey = router.keypair_arc().public_key();
    let comms_surface = CommsToolSurface::new(router, trusted_peers.clone());
    let availability = CommsToolSurface::peer_availability(trusted_peers, self_pubkey);
    let gateway = meerkat_core::ToolGatewayBuilder::new()
        .add_dispatcher(base_tools)
        .add_dispatcher_with_availability(Arc::new(comms_surface), availability)
        .build()?;
    let mut instructions = tool_usage_instructions;
    if !instructions.is_empty() {
        instructions.push_str("\n\n");
    }
    instructions.push_str(CommsToolSurface::usage_instructions());
    Ok((Arc::new(gateway), instructions))
}

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
