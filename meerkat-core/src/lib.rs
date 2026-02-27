//! meerkat-core - Core agent logic for Meerkat (no I/O deps)
//!
//! This crate contains all the core types, traits, and logic for Meerkat agents.
//! It is intentionally free of I/O dependencies to enable easy testing and embedding.

// On wasm32, use tokio_with_wasm as a drop-in replacement for tokio.
// All internal code uses `tokio::` paths — the alias makes them resolve
// to tokio_with_wasm on wasm32 and real tokio on native.
// On wasm32, provide a `tokio` module that re-exports tokio_with_wasm's
// API. All internal `tokio::*` paths resolve through this on wasm32,
// and through the real tokio crate on native.
#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

pub mod agent;
pub mod budget;
pub mod checkpoint;
pub mod comms;
pub mod compact;
pub mod config;
#[cfg(not(target_arch = "wasm32"))]
pub mod config_runtime;
#[cfg(not(target_arch = "wasm32"))]
pub mod config_store;
pub mod error;
pub mod event;
pub mod event_injector;
pub mod event_tap;
pub mod gateway;
pub mod hooks;
pub mod interaction;
pub mod mcp_config;
pub mod memory;
pub mod ops;
pub mod peer_meta;
#[cfg(not(target_arch = "wasm32"))]
pub mod prompt;
pub mod provider;
pub mod retry;
pub mod runtime_bootstrap;
pub mod schema;
pub mod service;
pub mod session;
pub mod skills;
pub mod skills_config;
pub mod state;
pub mod sub_agent;
pub mod time_compat;
pub mod tool_scope;
pub mod turn_boundary;
pub mod types;

// Re-export main types at crate root
pub use agent::{
    Agent, AgentBuilder, AgentLlmClient, AgentRunner, AgentSessionStore, AgentToolDispatcher,
    CommsRuntime, FilteredToolDispatcher, LlmStreamResult,
};
pub use budget::{Budget, BudgetLimits, BudgetPool};
pub use checkpoint::SessionCheckpointer;
pub use comms::{
    CommsCommand, EventStream, InputSource, InputStreamMode, PeerDirectoryEntry,
    PeerDirectorySource, PeerName, SendAndStreamError, SendError, SendReceipt, StreamError,
    StreamScope,
};
pub use compact::{CompactionConfig, CompactionContext, CompactionResult, Compactor};
pub use memory::{MemoryMetadata, MemoryResult, MemoryStore, MemoryStoreError};
pub use peer_meta::PeerMeta;

pub use config::{
    AgentConfig, BudgetConfig, CommsAuthMode, CommsRuntimeConfig, CommsRuntimeMode, Config,
    ConfigDelta, ConfigScope, HookEntryConfig, HookRunOverrides, HookRuntimeConfig, HooksConfig,
    LimitsConfig, ModelDefaults, PlainEventSource, ProviderConfig, ProviderSettings,
    ResolvedSubAgentConfig, RetryConfig, ShellDefaults, StorageConfig, StoreConfig,
    SubAgentsConfig, ToolsConfig,
};
#[cfg(not(target_arch = "wasm32"))]
pub use config_runtime::{
    ConfigEnvelope, ConfigEnvelopePolicy, ConfigRuntime, ConfigRuntimeError, ConfigSnapshot,
};
#[cfg(not(target_arch = "wasm32"))]
pub use config_store::{
    ConfigResolvedPaths, ConfigStore, ConfigStoreMetadata, FileConfigStore, MemoryConfigStore,
    TaggedConfigStore,
};
pub use error::{AgentError, ToolError};
pub use event::{
    AgentEvent, BudgetType, EventEnvelope, ScopedAgentEvent, StreamScopeFrame,
    ToolConfigChangeOperation, ToolConfigChangedPayload, VerboseEventConfig, agent_event_type,
    compare_event_envelopes, format_verbose_event, format_verbose_event_with_config,
};
pub use event_injector::{
    EventInjector, EventInjectorError, InteractionSubscription, SubscribableInjector,
};
pub use event_tap::{
    EventTap, EventTapState, new_event_tap, tap_emit, tap_send_terminal, tap_try_send,
};
pub use gateway::{Availability, AvailabilityCheck, ToolGateway, ToolGatewayBuilder};
pub use hooks::{
    HookCapability, HookDecision, HookEngine, HookEngineError, HookExecutionMode,
    HookExecutionReport, HookFailurePolicy, HookId, HookInvocation, HookLlmRequest,
    HookLlmResponse, HookOutcome, HookPatch, HookPatchEnvelope, HookPoint, HookReasonCode,
    HookRevision, HookToolCall, HookToolResult, default_failure_policy,
};
pub use interaction::{InboxInteraction, InteractionContent, InteractionId, ResponseStatus};
pub use mcp_config::{McpConfig, McpConfigError, McpScope, McpServerConfig, McpServerWithScope};
pub use ops::{
    ConcurrencyLimits, ContextStrategy, ForkBranch, ForkBudgetPolicy, OpEvent, OperationId,
    OperationPolicy, OperationResult, OperationSpec, ResultShape, SpawnSpec, SubAgentState,
    ToolAccessPolicy, WorkKind,
};
#[cfg(not(target_arch = "wasm32"))]
pub use prompt::{AGENTS_MD_MAX_BYTES, DEFAULT_SYSTEM_PROMPT, SystemPromptConfig};
pub use provider::Provider;
pub use retry::RetryPolicy;
pub use runtime_bootstrap::{
    ContextConfig, RealmConfig, RealmLocator, RealmSelection, RuntimeBootstrap,
    RuntimeBootstrapError, default_state_root, derive_workspace_realm_id, generate_realm_id,
};
pub use schema::{
    CompiledSchema, MeerkatSchema, SchemaCompat, SchemaError, SchemaFormat, SchemaWarning,
};
pub use service::{
    CreateSessionRequest, SessionBuildOptions, SessionError, SessionInfo, SessionQuery,
    SessionService, SessionSummary, SessionUsage, SessionView, StartTurnRequest, TurnToolOverlay,
};
pub use session::{SESSION_VERSION, Session, SessionMeta, SessionMetadata, SessionTooling};
pub use state::LoopState;
pub use sub_agent::{SubAgentCommsInfo, SubAgentCompletion, SubAgentInfo, SubAgentManager};
pub use tool_scope::{
    ComposedToolFilter, EXTERNAL_TOOL_FILTER_METADATA_KEY, ToolFilter, ToolScope, ToolScopeHandle,
    ToolScopeRevision, ToolScopeStageError,
};
pub use turn_boundary::{TurnBoundaryHook, TurnBoundaryMessage};
pub use types::{
    ArtifactRef, AssistantBlock, AssistantMessage, BlockAssistantMessage, Message, OutputSchema,
    ProviderMeta, RunResult, SecurityMode, SessionId, StopReason, SystemMessage, ToolCall,
    ToolCallIter, ToolCallView, ToolDef, ToolResult, Usage, UserMessage,
};
