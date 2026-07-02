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
pub mod approval;
pub mod artifact;
pub mod auth;
pub mod blob;
pub mod budget;
pub mod checkpoint;
pub mod comms;
pub mod compact;
pub mod completion_feed;
pub mod config;
#[cfg(not(target_arch = "wasm32"))]
pub mod config_runtime;
#[cfg(not(target_arch = "wasm32"))]
pub mod config_store;
pub mod connection;
pub mod error;
pub mod event;
pub mod event_injector;
pub mod event_tap;
pub mod gateway;
pub mod generated;
pub mod handles;
pub mod hooks;
pub mod image_content;
pub mod image_generation;
pub mod interaction;
pub mod lifecycle;
pub mod live_adapter;
pub mod mcp_config;
pub mod memory;
pub mod model_defaults;
pub mod model_profile;
pub mod model_registry;
pub mod oauth_identity;
pub mod ops;
pub mod ops_lifecycle;
pub mod peer_correlation;
pub mod peer_meta;
pub use generated::approval_lifecycle;
pub use generated::session_document;
pub mod pending_continuation;
pub mod placement;
#[cfg(not(target_arch = "wasm32"))]
pub mod prompt;
pub mod provider;
pub mod provider_matrix;
pub mod realtime_transcript;
pub mod realtime_transcript_revision;
pub mod retry;
pub mod runtime_bootstrap;
pub mod runtime_epoch;
pub mod schema;
pub mod service;
pub mod session;
pub mod session_durable_config_authority;
pub mod session_recovery;
pub mod session_store;
pub mod skills;
pub mod skills_config;
pub mod state;
pub mod surface_metadata;
pub mod time_compat;
pub mod tool_catalog;
pub mod tool_execution_policy;
pub mod tool_scope;
pub mod turn_boundary;
pub mod turn_execution_authority;
pub mod types;
pub mod web_search;

// Re-export main types at crate root
pub use agent::{
    Agent, AgentBuildPolicyError, AgentBuilder, AgentExecutionSnapshot, AgentLlmClient,
    AgentLlmClientDecorator, AgentLlmFallbackSkippedTarget, AgentLlmFallbackSwitch, AgentRunner,
    AgentSessionStore, AgentToolDispatcher, BindOutcome, CancelAfterBoundaryCommand,
    CancelAfterBoundarySender, CommsCapabilityError, CommsRuntime, CurrentTurnContent,
    CurrentTurnImageRef, DefaultSystemPromptPolicy, DispatcherCapabilities, ExternalToolUpdate,
    FilteredToolDispatcher, LlmStreamResult, SnapshotProjectionError, SystemContextStateError,
    ToolDispatchContext, select_tool_catalog_mode, should_compose_tool_catalog_control_plane,
};
pub use approval::{
    ApprovalActionKind, ApprovalDecision, ApprovalDecisionRecord, ApprovalError, ApprovalId,
    ApprovalListFilter, ApprovalMemberRef, ApprovalMobRef, ApprovalOwnerRef, ApprovalPrincipalId,
    ApprovalProposedAction, ApprovalRecord, ApprovalRequest, ApprovalResourceId,
    ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk, ApprovalService, ApprovalStatus,
    ApprovalStore, ApprovalStoreError, InMemoryApprovalStore,
};
pub use artifact::{
    ArtifactContentHandle, ArtifactError, ArtifactHandle, ArtifactId, ArtifactListFilter,
    ArtifactOwner, ArtifactPayload, ArtifactRecord, ArtifactStore, ArtifactType,
};
pub use auth::{
    ActingOnBehalfOf, AuthGrant, GrantAction, GrantScope, PrincipalContractError, PrincipalId,
    PrincipalKind, PrincipalRef, VisibilityClass, can_observe_visibility,
    metadata_grants_no_visibility,
};
pub use blob::{BlobId, BlobPayload, BlobRef, BlobStore, BlobStoreError};
pub use budget::{
    Budget, BudgetDimension, BudgetExceeded, BudgetLimits, BudgetObservation, BudgetPool,
};
pub use checkpoint::SessionCheckpointer;
pub use comms::{
    CommsCommand, EventStream, InputSource, InputStreamMode, PeerDirectoryEntry,
    PeerDirectorySource, PeerName, PeerRoute, SUPERVISOR_BRIDGE_INTENT, SendAndStreamError,
    SendError, SendReceipt, SendTaintOverride, SenderContentTaint, StreamError, StreamScope,
};
pub use compact::{
    CompactionConfig, CompactionContext, CompactionCurator, CompactionCuratorError,
    CompactionResult, CompactionWindow, Compactor, CuratedCompactionSummary,
    SESSION_COMPACTION_CADENCE_KEY, SessionCompactionCadence,
};
pub use memory::{
    EmbeddingModel, HnswParams, MemoryEnumerationPage, MemoryEnumerationRequest, MemoryIndexBatch,
    MemoryIndexReceipt, MemoryIndexRequest, MemoryIndexScope, MemoryMetadata, MemoryOwner,
    MemoryRankingPolicy, MemoryRecord, MemoryResult, MemoryScopeDropReceipt, MemorySearchScope,
    MemorySource, MemoryStore, MemoryStoreError, MessageRange,
};
pub use model_profile::{ModelCatalog, ModelProfile};
pub use model_registry::{
    ModelCapability, ModelRegistry, ModelRegistryEntry, SelfHostedServerRef,
    UnsupportedModelCapabilityEvidence, UnsupportedModelCapabilityReason,
};
pub use peer_correlation::{
    InboundPeerRequestState, InteractionStreamState, OutboundPeerRequestState, PeerCorrelationId,
};
pub use peer_meta::PeerMeta;
pub use placement::{ExecutionPlacement, ExecutionPlacementIdentity, PlacementError};
pub use surface_metadata::{
    MEERKAT_METADATA_PREFIX, RESERVED_MOB_LABEL_KEYS, ReservedMetadataKey, RuntimeMetadata,
    SurfaceMetadata, SurfaceMetadataError, is_reserved_meerkat_label_key,
    is_reserved_meerkat_metadata_key, validate_public_app_context, validate_public_labels,
};

pub use completion_feed::{
    CompletionBatch, CompletionEnrichmentData, CompletionEnrichmentProvider, CompletionEntry,
    CompletionFeed, CompletionSeq,
};
pub use config::{
    AgentConfig, BudgetConfig, CallTimeoutOverride, CommandRuntimeConfig, CommsAuthMode,
    CommsRuntimeConfig, CommsRuntimeMode, Config, ConfigDelta, ConfigError, ConfigScope,
    HookAdapterConfig, HookEntryConfig, HookInProcessHandlerId, HookInProcessRuntimeConfig,
    HookRunOverrides, HookRuntimeKind, HooksConfig, HttpRuntimeConfig, LimitsConfig, ModelDefaults,
    PlainEventSource, ProviderToolsConfig, RetryConfig, SelfHostedApiStyle, SelfHostedConfig,
    SelfHostedModelConfig, SelfHostedServerConfig, SelfHostedTransport, ShellDefaults,
    StorageConfig, StoreConfig, SystemPromptOverride, ToolsConfig,
};
#[cfg(not(target_arch = "wasm32"))]
pub use config_runtime::{
    ConfigEnvelope, ConfigEnvelopePolicy, ConfigRuntime, ConfigRuntimeError, ConfigSnapshot,
};
#[cfg(not(target_arch = "wasm32"))]
pub use config_store::{
    ConfigResolvedPaths, ConfigStore, ConfigStoreMetadata, EffectiveConfigReader, FileConfigStore,
    MemoryConfigStore, RealmConfigSource, TaggedConfigStore, apply_config_patch_preview,
    merge_patch,
};
pub use error::{AgentError, ToolError};
pub use event::{
    AgentErrorClass, AgentErrorReport, AgentEvent, AssistantImageEvent, BudgetType,
    CompactionFailureReason, EventEnvelope, EventSourceIdentity, ExternalToolDelta,
    ExternalToolDeltaPhase, InteractionFailureReason, ScopedAgentEvent,
    SkillResolutionFailureReason, StreamScopeFrame, StreamTruncationReason, ToolCallArguments,
    ToolCallArgumentsError, ToolConfigChangeOperation, ToolConfigChangeStatus,
    ToolConfigChangedPayload, TurnErrorMetadata, VerboseEventConfig, agent_event_type,
    compare_event_envelopes, format_verbose_event, format_verbose_event_with_config,
};
pub use event_injector::{EventInjector, EventInjectorError};
pub use event_tap::{
    EventTap, EventTapState, new_event_tap, tap_emit, tap_send_terminal, tap_try_send,
};
pub use gateway::{DynamicToolComposite, ToolGateway, ToolGatewayBuilder};
pub use handles::{
    AuthLeasePhase, CommsDrainHandle, DrainExitReason, DrainMode, DslRejectionKind,
    DslTransitionError, ExternalToolSurfaceHandle, McpServerLifecycleHandle, PeerCommsHandle,
    PeerConversationProjection, PeerResponseProgressProjectionPhase,
    PeerResponseTerminalCorrelationId, PeerResponseTerminalDisplayIdentity,
    PeerResponseTerminalFact, PeerResponseTerminalFactError, PeerResponseTerminalProjectionStatus,
    PeerResponseTerminalRenderPayload, PeerResponseTerminalRouteIdentity,
    PeerResponseTerminalSource, PeerResponseTerminalTransportIdentity, SessionAdmissionHandle,
    SurfaceDiagnosticSnapshot, SurfaceSnapshot, TurnStateHandle, TurnStateSnapshot,
};
pub use hooks::{
    HookCapability, HookDecision, HookEngine, HookEngineError, HookExecutionMode,
    HookExecutionReport, HookFailureReason, HookId, HookInvocation, HookLlmRequest,
    HookLlmResponse, HookOutcome, HookPoint, HookReasonCode, HookToolCall, HookToolResult,
};
pub use image_content::{
    MissingBlobBehavior, collect_blob_ids_from_blocks, collect_blob_ids_from_messages,
    externalize_content_blocks, externalize_content_input, externalize_messages_from,
    hydrate_content_blocks, hydrate_content_input, hydrate_messages_for_execution,
};
pub use image_generation::*;
pub use interaction::{
    ClassifiedInboxInteraction, InboxInteraction, InteractionContent, InteractionId,
    PeerIngressAdmission, PeerIngressAdmissionDiagnostic, PeerIngressAuthDecision,
    PeerIngressAuthExemption, PeerIngressAuthorityPhase, PeerIngressClassification,
    PeerIngressConvention, PeerIngressDequeueAuthority, PeerIngressDequeueFacts,
    PeerIngressDiagnosticDisplay, PeerIngressEntrySnapshot, PeerIngressEnvelopeFacts,
    PeerIngressEnvelopeKind, PeerIngressFact, PeerIngressIdentity, PeerIngressKind,
    PeerIngressPlainEventFacts, PeerIngressQueueSnapshot, PeerIngressReceiveAuthority,
    PeerIngressReceiveFacts, PeerIngressReceiveOutcome, PeerIngressRuntimeSnapshot, PeerInputClass,
    ResponseStatus, SendResponseCallProjection, TerminalDisposition, TerminalityClass,
    format_external_event_projection, format_peer_ack_projection, format_peer_message_projection,
    format_peer_request_projection, format_peer_response_projection,
    render_peer_ingress_admitted_text,
};
pub use lifecycle::run_primitive::{
    ProviderParamsCarrier, ProviderParamsMergeError, ProviderParamsOverride, ProviderTag,
};
pub use lifecycle::{
    ConversationAppend, ConversationAppendRole, ConversationContextAppend, CoreApplyFailureCause,
    CoreApplyFailureCauseKind, CoreControlFailureCause, CoreControlFailureCauseKind, CoreExecutor,
    CoreExecutorBoundaryHandle, CoreExecutorError, CoreExecutorInterruptHandle, CoreRenderable,
    InputId, RunApplyBoundary, RunBoundaryReceipt, RunBoundaryReceiptDraft, RunEvent, RunId,
    RunPrimitive, StagedRunInput,
};
pub use mcp_config::{McpConfig, McpConfigError, McpScope, McpServerConfig, McpServerWithScope};
pub use model_defaults::ModelOperationalDefaultsResolver;
pub use oauth_identity::OAuthProviderIdentity;
pub use ops::{
    AsyncOpRef, ConcurrencyLimits, ContextStrategy, ForkBranch, ForkBudgetPolicy, OpEvent,
    OperationId, OperationPolicy, OperationResult, OperationSpec, ResultShape, SessionEffect,
    SpawnSpec, ToolAccessPolicy, ToolDispatchOutcome, ToolDispatchTerminalCause,
    ToolDispatchTerminalErrorKind, ToolDispatchTimeoutPolicy, WaitPolicy, WorkKind,
};
pub use ops_lifecycle::{
    OperationCompletionWatch, OperationCompletionWatchError, OperationKind,
    OperationLifecycleSnapshot, OperationPeerHandle, OperationProgressUpdate, OperationStatus,
    OperationTerminalOutcome, OpsLifecycleError, OpsLifecycleRegistry, WaitAllResult,
    WaitAllSatisfied,
};
#[cfg(not(target_arch = "wasm32"))]
pub use prompt::{AGENTS_MD_MAX_BYTES, DEFAULT_SYSTEM_PROMPT, SystemPromptConfig};
pub use provider::Provider;
pub use realtime_transcript::{
    RealtimeTranscriptApplyOutcome, RealtimeTranscriptEvent, RealtimeTranscriptMaterializedMessage,
    RealtimeTranscriptRole, SESSION_REALTIME_TRANSCRIPT_STATE_KEY,
};
pub use retry::{
    LlmRetryFailure, LlmRetryFailureKind, LlmRetryPlan, LlmRetrySchedule, RetryPolicy,
    select_retry_delay,
};
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
    PublicTurnToolOverlay, SessionBuildOptions, SessionControlError, SessionError,
    SessionForkAtRequest, SessionForkReplaceRequest, SessionForkResult, SessionHistoryPage,
    SessionHistoryQuery, SessionInfo, SessionQuery, SessionService, SessionServiceCommsExt,
    SessionServiceControlExt, SessionServiceHistoryExt, SessionServiceTranscriptEditExt,
    SessionSummary, SessionTranscriptRestoreRevisionRequest, SessionTranscriptRevisionList,
    SessionTranscriptRevisionListEntry, SessionTranscriptRevisionListQuery,
    SessionTranscriptRevisionPage, SessionTranscriptRevisionQuery, SessionTranscriptRewriteRequest,
    SessionTranscriptRewriteResult, SessionUsage, SessionView, StageToolResultsRequest,
    StageToolResultsResult, StartTurnRequest, TranscriptEditError, TranscriptEditRunningBehavior,
    TranscriptReplacement, TranscriptRewriteCommit, TranscriptRewriteReason,
    TranscriptRewriteSelection, TurnToolOverlay,
};
pub use session::{
    AuthorizedSessionToolVisibilityState, ConsumedDeferredTurnInputs, DeferredFirstTurnPhase,
    DeferredToolLoadAuthority, InheritedToolVisibilityAuthority, PendingDeferredPrompt,
    PendingSystemContextAppend, PendingToolResultsMessage, SESSION_BUILD_STATE_KEY,
    SESSION_DEFERRED_TURN_STATE_KEY, SESSION_LIFECYCLE_TERMINAL_KEY,
    SESSION_METADATA_SCHEMA_VERSION, SESSION_SYSTEM_CONTEXT_STATE_KEY,
    SESSION_TOOL_VISIBILITY_STATE_KEY, SESSION_TRANSCRIPT_HISTORY_STATE_KEY, SESSION_VERSION,
    SYSTEM_CONTEXT_SEPARATOR, SeenSystemContextKey, SeenSystemContextState, Session,
    SessionBuildState, SessionDeferredTurnState, SessionLifecycleTerminal, SessionLlmIdentity,
    SessionLlmIdentityOverride, SessionLlmIdentityOverrideError, SessionLlmRequestPolicy,
    SessionMeta, SessionMetadata, SessionSystemContextState, SessionToolVisibilityState,
    SessionTooling, SystemContextStageError, SystemContextStateHandle, ToolCategoryOverride,
    ToolVisibilityWitness, TranscriptHistoryState, TranscriptRevisionBody, TranscriptRewriteRecord,
    VIEW_IMAGE_TOOL_NAME, WitnessedToolFilter, capability_base_filter_for_image_tool_results,
    resolve_session_llm_identity_override, session_metadata_schema_version, session_version,
    transcript_messages_digest,
};
pub use session_recovery::{
    BUILD_ONLY_RECOVERY_OVERRIDE_ERROR, RecoveredSessionBuild, RecoveryBackendKind,
    SurfaceSessionRecoveryContext, SurfaceSessionRecoveryError, SurfaceSessionRecoveryOverrides,
    build_recovered_session, has_build_only_turn_overrides, has_materialization_overrides,
    session_allows_first_turn_build_overrides,
};
pub use session_store::{SessionFilter, SessionStore, SessionStoreError};
pub use state::LoopState;
pub use tool_catalog::{
    ToolCallability, ToolCatalogCapabilities, ToolCatalogDeferredEligibility, ToolCatalogEntry,
    ToolCatalogLoadRejectedReason, ToolCatalogLoadResolution, ToolCatalogMode, ToolPlaneClass,
    ToolUnavailableReason, deferred_session_entry_count, deferred_session_schema_volume,
    select_catalog_mode_from_snapshot,
};
pub use tool_execution_policy::{
    ExecutionPolicyGatedDispatcher, ToolExecutionPolicy, ToolExecutionPolicyError,
};
pub use tool_scope::{
    ComposedToolFilter, EXTERNAL_TOOL_FILTER_METADATA_KEY, ExternalToolSurfaceBaseState,
    ExternalToolSurfaceDeltaOperation, ExternalToolSurfaceDeltaPhase,
    ExternalToolSurfaceEntrySnapshot, ExternalToolSurfaceFailureCause,
    ExternalToolSurfaceGlobalPhase, ExternalToolSurfacePendingOp, ExternalToolSurfaceSnapshot,
    ExternalToolSurfaceStagedOp, GeneratedToolVisibilityOwner, LocalToolVisibilityOwner,
    ToolFilter, ToolScope, ToolScopeApplyError, ToolScopeHandle, ToolScopeRevision,
    ToolScopeSnapshot, ToolScopeStageError, ToolVisibilityOwner,
};
pub use turn_boundary::{TurnBoundaryHook, TurnBoundaryMessage};
pub use turn_execution_authority::{
    ContentShape, TurnExecutionEffect, TurnExecutionInput, TurnExecutionTransition, TurnPhase,
    TurnPrimitiveKind, TurnTerminalCauseKind, TurnTerminalOutcome,
};
pub use types::{
    ArtifactRef, AssistantBlock, BlockAssistantMessage, CommsNoticeKind, ContentBlock,
    ContentInput, ExtractionError, HandlingMode, ImageData, MemoryIndexExclusion,
    MemoryIndexableContent, Message, OutputSchema, ProviderMeta, RunInput, RunResult,
    SUPPORTED_VIDEO_MEDIA_TYPES, SecurityMode, ServerToolKind, SessionId, StopReason,
    SystemMessage, SystemNoticeBlock, SystemNoticeDirection, SystemNoticeKind, SystemNoticeMessage,
    SystemNoticePeer, SystemPromptMutationKind, ToolCall, ToolCallIter, ToolCallView, ToolDef,
    ToolIdentity, ToolName, ToolNameSet, ToolProvenance, ToolResult, ToolSourceId, ToolSourceKind,
    TranscriptMessageIdentity, TranscriptSource, TranscriptUserRole, Usage, UserMessage, VideoData,
    assistant_blocks_have_visible_or_actionable_output, has_images, has_non_text_content,
    has_video, is_supported_video_media_type, validate_inline_video_blocks,
};
pub use web_search::*;

// === Provider auth v2 (landed ahead of wiring — see
// /Users/luka/.claude/plans/yes-make-a-plan-shimmying-bengio.md) ===
pub use auth::{
    AnthropicAuthMetadata, AnthropicRouteHints, AuthConstraints, AuthError, AuthErrorKind,
    AuthErrorSummary, AuthLease, AuthMetadata, AuthMetadataDefaults, AuthRefreshReason,
    AuthRouteHints, AuthStatus, AuthStatusPhase, GoogleAuthMetadata, GoogleRouteHints,
    HttpAuthorizationRequest, HttpAuthorizer, OpenAiAuthMetadata, OpenAiRouteHints,
    ProviderAuthMetadata, PublishedAuthStatus, RefreshFailureObservation, ResolvedAuthEnvelope,
    ResolvedAuthKind, TokenLifecycleClearError, clear_tokens_and_publish_lifecycle_released,
    lease_snapshot_expires_at_datetime, mark_tokens_lifecycle_published_for_transition,
    oauth_status_projection_snapshot_from_newer_marker, persisted_auth_mode_is_directly_creatable,
    persisted_auth_mode_uses_oauth_login_lifecycle, persisted_token_expires_at_epoch_secs,
    project_published_auth_status, publish_token_lifecycle_acquired,
    publish_token_lifecycle_released, restore_token_lifecycle_snapshot,
    tokens_lifecycle_publication, tokens_lifecycle_publication_with_explicit_expiry,
    tokens_lifecycle_published, tokens_lifecycle_published_generation,
};
#[cfg(not(target_arch = "wasm32"))]
pub use auth::{
    AuthLoginLifecycleGuard, AuthStatusRehydrateError, acquire_auth_login_lifecycle_guard,
    rehydrate_marked_tokens_for_status,
};
pub use connection::{
    AuthBindingRef, AuthProfile, AuthProfileConfig, BackendProfile, BackendProfileConfig,
    BindingId, BindingOrigin, BindingPolicy, ConnectionTargetError, CredentialSourceSpec,
    ExternalResolverId, IdentityError, MemberCommsName, MemberCommsNameError, MobMemberBinding,
    PeerRole, ProfileId, ProviderBinding, ProviderBindingConfig, ProviderBindingError,
    RealmConfigSection, RealmConnectionSet, RealmId, ResolvedConnectionTarget, mob_realm_id,
    resolve_auth_binding_candidates_for_provider, resolve_auth_binding_or_default_for_provider,
    resolve_realm_binding_target_for_provider,
};
