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
pub mod blob;
pub mod budget;
pub mod checkpoint;
pub mod comms;
pub mod comms_drain_lifecycle_authority;
pub mod compact;
pub mod completion_feed;
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
pub mod generated;
pub mod hooks;
pub mod image_content;
pub mod interaction;
pub mod lifecycle;
pub mod mcp_config;
pub mod memory;
pub mod model_defaults;
pub mod model_registry;
pub mod ops;
pub mod ops_lifecycle;
pub mod peer_meta;
#[cfg(not(target_arch = "wasm32"))]
pub mod prompt;
pub mod provider;
pub mod retry;
pub mod runtime_bootstrap;
pub mod runtime_epoch;
pub mod schema;
pub mod service;
pub mod session;
pub mod session_recovery;
pub mod session_store;
pub mod skills;
pub mod skills_config;
pub mod state;
pub mod time_compat;
pub mod tool_catalog;
pub mod tool_scope;
pub mod turn_boundary;
pub mod turn_execution_authority;
pub mod types;

// Re-export main types at crate root
pub use agent::{
    Agent, AgentBuilder, AgentExecutionSnapshot, AgentLlmClient, AgentRunner, AgentSessionStore,
    AgentToolDispatcher, BindOutcome, CommsCapabilityError, CommsRuntime, DispatcherCapabilities,
    ExternalToolUpdate, FilteredToolDispatcher, LlmStreamResult, select_tool_catalog_mode,
    should_compose_tool_catalog_control_plane,
};
pub use blob::{BlobId, BlobPayload, BlobRef, BlobStore, BlobStoreError};
pub use budget::{Budget, BudgetLimits, BudgetPool};
pub use checkpoint::SessionCheckpointer;
pub use comms::{
    CommsCommand, EventStream, InputSource, InputStreamMode, PeerDirectoryEntry,
    PeerDirectorySource, PeerName, PeerReachability, PeerReachabilityReason, SendAndStreamError,
    SendError, SendReceipt, StreamError, StreamScope,
};
pub use comms_drain_lifecycle_authority::{
    CommsDrainLifecycleAuthority, CommsDrainLifecycleEffect, CommsDrainLifecycleError,
    CommsDrainLifecycleInput, CommsDrainLifecycleMutator, CommsDrainLifecycleTransition,
    CommsDrainMode, CommsDrainPhase, DrainExitReason,
};
pub use compact::{
    CompactionConfig, CompactionContext, CompactionResult, Compactor,
    SESSION_COMPACTION_CADENCE_KEY, SessionCompactionCadence,
};
pub use memory::{MemoryMetadata, MemoryResult, MemoryStore, MemoryStoreError};
pub use model_registry::{ModelRegistry, ModelRegistryEntry, SelfHostedServerRef};
pub use peer_meta::PeerMeta;

pub use completion_feed::{
    CompletionBatch, CompletionEnrichmentData, CompletionEnrichmentProvider, CompletionEntry,
    CompletionFeed, CompletionSeq,
};
pub use config::{
    AgentConfig, BudgetConfig, CallTimeoutOverride, CommsAuthMode, CommsRuntimeConfig,
    CommsRuntimeMode, Config, ConfigDelta, ConfigError, ConfigScope, HookEntryConfig,
    HookRunOverrides, HookRuntimeConfig, HooksConfig, LimitsConfig, ModelDefaults,
    PlainEventSource, ProviderConfig, ProviderSettings, ProviderToolsConfig, RetryConfig,
    SelfHostedApiStyle, SelfHostedConfig, SelfHostedModelConfig, SelfHostedServerConfig,
    SelfHostedTransport, ShellDefaults, StorageConfig, StoreConfig, ToolsConfig,
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
    AgentEvent, BudgetType, EventEnvelope, ExternalToolDelta, ExternalToolDeltaPhase,
    ScopedAgentEvent, StreamScopeFrame, ToolConfigChangeOperation, ToolConfigChangedPayload,
    VerboseEventConfig, agent_event_type, compare_event_envelopes, format_verbose_event,
    format_verbose_event_with_config,
};
pub use event_injector::{EventInjector, EventInjectorError};
pub use event_tap::{
    EventTap, EventTapState, new_event_tap, tap_emit, tap_send_terminal, tap_try_send,
};
pub use gateway::{
    Availability, AvailabilityCheck, DynamicToolComposite, ToolGateway, ToolGatewayBuilder,
};
pub use hooks::{
    HookCapability, HookDecision, HookEngine, HookEngineError, HookExecutionMode,
    HookExecutionReport, HookFailurePolicy, HookId, HookInvocation, HookLlmRequest,
    HookLlmResponse, HookOutcome, HookPatch, HookPatchEnvelope, HookPoint, HookReasonCode,
    HookRevision, HookToolCall, HookToolResult, apply_tool_result_patch, default_failure_policy,
};
pub use image_content::{
    MissingBlobBehavior, collect_blob_ids_from_blocks, collect_blob_ids_from_messages,
    externalize_content_blocks, externalize_content_input, externalize_messages_from,
    hydrate_content_blocks, hydrate_content_input, hydrate_messages_for_execution,
};
pub use interaction::{
    ClassifiedInboxInteraction, InboxInteraction, InteractionContent, InteractionId,
    PeerIngressAuthorityPhase, PeerIngressEntrySnapshot, PeerIngressKind, PeerIngressQueueSnapshot,
    PeerIngressRuntimeSnapshot, PeerInputClass, ResponseStatus,
};
pub use lifecycle::{
    ConversationAppend, ConversationAppendRole, ConversationContextAppend, CoreExecutor,
    CoreExecutorError, CoreRenderable, InputId, RunApplyBoundary, RunBoundaryReceipt,
    RunControlCommand, RunEvent, RunId, RunPrimitive, StagedRunInput,
};
pub use mcp_config::{McpConfig, McpConfigError, McpScope, McpServerConfig, McpServerWithScope};
pub use model_defaults::ModelOperationalDefaultsResolver;
pub use ops::{
    AsyncOpRef, ConcurrencyLimits, ContextStrategy, ForkBranch, ForkBudgetPolicy, OpEvent,
    OperationId, OperationPolicy, OperationResult, OperationSpec, ResultShape, SessionEffect,
    SpawnSpec, ToolAccessPolicy, ToolDispatchOutcome, WaitPolicy, WorkKind,
};
pub use ops_lifecycle::{
    OperationCompletionWatch, OperationKind, OperationLifecycleSnapshot, OperationPeerHandle,
    OperationProgressUpdate, OperationStatus, OperationTerminalOutcome, OpsLifecycleError,
    OpsLifecycleRegistry, WaitAllResult, WaitAllSatisfied,
};
#[cfg(not(target_arch = "wasm32"))]
pub use prompt::{AGENTS_MD_MAX_BYTES, DEFAULT_SYSTEM_PROMPT, SystemPromptConfig};
pub use provider::Provider;
pub use retry::RetryPolicy;
pub use runtime_bootstrap::{
    ContextConfig, RealmConfig, RealmLocator, RealmSelection, RuntimeBootstrap,
    RuntimeBootstrapError, default_state_root, derive_workspace_realm_id, generate_realm_id,
};
pub use runtime_epoch::{
    EpochCursorSnapshot, EpochCursorState, RuntimeBuildMode, RuntimeEpochId, SessionRuntimeBindings,
};
pub use schema::{
    CompiledSchema, MeerkatSchema, SchemaCompat, SchemaError, SchemaFormat, SchemaWarning,
};
pub use service::{
    AppendSystemContextRequest, AppendSystemContextResult, AppendSystemContextStatus,
    CreateSessionRequest, DeferredPromptPolicy, MobToolsBuildArgs, MobToolsFactory,
    SessionBuildOptions, SessionControlError, SessionError, SessionHistoryPage,
    SessionHistoryQuery, SessionInfo, SessionQuery, SessionService, SessionServiceCommsExt,
    SessionServiceControlExt, SessionServiceHistoryExt, SessionSummary, SessionUsage, SessionView,
    StageToolResultsRequest, StageToolResultsResult, StartTurnRequest, TurnToolOverlay,
};
pub use session::{
    DeferredFirstTurnPhase, PendingDeferredPrompt, PendingSystemContextAppend,
    PendingToolResultsMessage, SESSION_BUILD_STATE_KEY, SESSION_DEFERRED_TURN_STATE_KEY,
    SESSION_SYSTEM_CONTEXT_STATE_KEY, SESSION_TOOL_VISIBILITY_STATE_KEY, SESSION_VERSION,
    SYSTEM_CONTEXT_SEPARATOR, SeenSystemContextKey, SeenSystemContextState, Session,
    SessionBuildState, SessionDeferredTurnState, SessionLlmIdentity, SessionMeta, SessionMetadata,
    SessionSystemContextState, SessionToolVisibilityState, SessionTooling, SystemContextStageError,
    ToolCategoryOverride, ToolVisibilityWitness,
};
pub use session_recovery::{
    BUILD_ONLY_RECOVERY_OVERRIDE_ERROR, RecoveredSessionBuild, SurfaceSessionRecoveryContext,
    SurfaceSessionRecoveryError, SurfaceSessionRecoveryOverrides, build_recovered_session,
    has_build_only_turn_overrides, has_materialization_overrides,
    session_allows_first_turn_build_overrides,
};
pub use session_store::{SessionFilter, SessionStore, SessionStoreError};
pub use state::LoopState;
pub use tool_catalog::{
    ToolCatalogCapabilities, ToolCatalogDeferredEligibility, ToolCatalogEntry,
    ToolCatalogLoadRejectedReason, ToolCatalogLoadResolution, ToolCatalogMode, ToolPlaneClass,
    deferred_session_entry_count, deferred_session_schema_volume,
    select_catalog_mode_from_snapshot,
};
pub use tool_scope::{
    ComposedToolFilter, EXTERNAL_TOOL_FILTER_METADATA_KEY, ExternalToolSurfaceBaseState,
    ExternalToolSurfaceDeltaOperation, ExternalToolSurfaceDeltaPhase,
    ExternalToolSurfaceEntrySnapshot, ExternalToolSurfaceGlobalPhase, ExternalToolSurfacePendingOp,
    ExternalToolSurfaceSnapshot, ExternalToolSurfaceStagedOp, ToolFilter, ToolScope,
    ToolScopeHandle, ToolScopeRevision, ToolScopeSnapshot, ToolScopeStageError,
};
pub use turn_boundary::{TurnBoundaryHook, TurnBoundaryMessage};
pub use turn_execution_authority::{
    ContentShape, TurnExecutionAuthority, TurnExecutionEffect, TurnExecutionInput,
    TurnExecutionMutator, TurnExecutionTransition, TurnPhase, TurnPrimitiveKind,
    TurnTerminalOutcome,
};
pub use types::{
    ArtifactRef, AssistantBlock, AssistantMessage, BlockAssistantMessage, ContentBlock,
    ContentInput, HandlingMode, ImageData, Message, OutputSchema, ProviderMeta, RunResult,
    SUPPORTED_VIDEO_MEDIA_TYPES, SecurityMode, SessionId, StopReason, SystemMessage,
    SystemNoticeKind, SystemNoticeMessage, ToolCall, ToolCallIter, ToolCallView, ToolDef,
    ToolProvenance, ToolResult, ToolSourceKind, Usage, UserMessage, VideoData, has_images,
    has_non_text_content, has_video, is_supported_video_media_type, validate_inline_video_blocks,
};
