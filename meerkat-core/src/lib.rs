//! meerkat-core - Foundational agent contracts and logic for Meerkat.
//!
//! This crate contains all the core types, traits, and logic for Meerkat agents.
//! Most runtime implementations live in satellite crates. Native builds also
//! own the canonical config-store, config-runtime, path-layout, and maintenance
//! fence primitives, so this crate is not a blanket no-I/O layer.

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
pub mod comms;
pub mod compact;
pub mod completion_feed;
pub mod config;
#[cfg(not(target_arch = "wasm32"))]
pub mod config_runtime;
#[cfg(not(target_arch = "wasm32"))]
pub mod config_store;
pub mod connection;
pub mod context_budget;
mod digest_observability;
pub mod error;
pub mod event;
pub mod event_injector;
pub mod event_tap;
pub mod exact_operation;
pub mod gateway;
pub mod generated;
pub mod handles;
pub mod hooks;
pub mod image_content;
pub mod image_generation;
pub mod interaction;
pub mod lifecycle;
pub mod live_adapter;
pub mod live_execution;
pub mod mcp_config;
pub mod memory;
pub mod model_defaults;
pub mod model_profile;
pub mod model_registry;
pub mod oauth_identity;
pub mod ops;
pub mod ops_lifecycle;
pub mod panic_payload;
pub mod peer_correlation;
pub mod peer_meta;
pub mod persistence_contract;
pub mod secret_entropy;
pub use generated::approval_lifecycle;
pub use generated::session_document;
pub mod pending_continuation;
pub mod placement;
pub mod prompt;
pub mod provider;
pub mod provider_evidence;
pub mod provider_matrix;
pub mod realtime_transcript;
pub mod realtime_transcript_revision;
pub mod realtime_transcript_sidecar;
pub mod retry;
pub mod runtime_bootstrap;
pub mod runtime_epoch;
pub mod schema;
pub mod self_hosted_binding;
pub mod service;
pub mod session;
pub mod session_component_sidecar;
pub mod session_durable_config_authority;
pub mod session_recovery;
pub mod session_store;
pub mod skills;
pub mod skills_config;
pub mod state;
pub mod storage_diagnostics;
pub mod storage_durability;
pub mod storage_layout;
pub mod streaming_tool;
pub mod surface_metadata;
pub mod time_compat;
pub mod tool_catalog;
pub mod tool_consequence_policy;
pub mod tool_execution;
pub mod tool_execution_policy;
pub mod tool_scope;
pub mod turn_boundary;
pub mod turn_execution_authority;
pub mod turn_terminal;
pub mod types;
pub mod web_search;

// Re-export main types at crate root
pub use agent::{
    Agent, AgentBuildPolicyError, AgentBuilder, AgentControlStateError, AgentExecutionSnapshot,
    AgentLlmClient, AgentLlmClientDecorator, AgentLlmFallbackSkippedTarget, AgentLlmFallbackSwitch,
    AgentLlmRequestAttempt, AgentRunner, AgentSessionStore, AgentToolDispatcher, BindOutcome,
    CancelAfterBoundaryCommand, CancelAfterBoundarySender, CommsCapabilityError, CommsRuntime,
    CurrentTurnContent, CurrentTurnImageRef, DefaultSystemPromptPolicy, DispatcherCapabilities,
    ExternalToolUpdate, FilteredToolDispatcher, LiveBridgeNoncommittingRunPermit,
    LiveBridgePreparedOperation, LiveBridgeToolDispatchAdmission, LlmStreamResult,
    RequestAttemptAuthority, SnapshotProjectionError, StickyModelFallbackActivationProof,
    ToolDispatchContext, dispatch_tool_execution_plan_fenced, resolve_tool_execution_plan_fenced,
    select_tool_catalog_mode, should_compose_tool_catalog_control_plane,
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
    ActingOnBehalfOf, AuthBindingUseDecision, AuthBindingUseDenial, AuthBindingUseGateError,
    AuthBindingUseRequest, AuthBindingUseWitness, AuthGrant, GrantAction, GrantScope,
    PrincipalContractError, PrincipalId, PrincipalKind, PrincipalRef, VisibilityClass,
    authorize_explicit_auth_binding_use, authorize_then_materialize_auth_binding,
    can_observe_visibility, metadata_grants_no_visibility,
};
pub use blob::{
    BlobId, BlobPayload, BlobRef, BlobStore, BlobStoreError, ImageBlobIntegrityError,
    VerifiedImageBlob, content_blob_id, ensure_stored_image_blob, validate_image_blob_payload,
    verify_stored_image_blob,
};
pub use budget::{
    Budget, BudgetDimension, BudgetExceeded, BudgetLimits, BudgetObservation, BudgetPool,
};
pub use comms::{
    CommsCommand, EventStream, InputSource, InputStreamMode, PeerDirectoryEntry,
    PeerDirectorySource, PeerName, PeerRoute, SUPERVISOR_BRIDGE_INTENT, SendAndStreamError,
    SendError, SendReceipt, SendTaintOverride, SenderContentTaint, StreamError, StreamScope,
};
pub use compact::{
    COMPACTION_SUMMARY_PREFIX, CompactionConfig, CompactionContext, CompactionCurator,
    CompactionCuratorError, CompactionDiscard, CompactionResult, CompactionRetained,
    CompactionSummary, CompactionWindow, Compactor, CuratedCompactionSummary,
    ProviderRequestPressure, SESSION_COMPACTION_CADENCE_KEY, SessionCompactionCadence,
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
pub use context_budget::{
    ContextBudgetEstimateProvenance, ContextBudgetFact, ContextBudgetFactError, ContextBudgetState,
    context_budget_fact_for_messages, context_budget_fact_for_provider_request,
    context_budget_fact_for_session,
};
pub use digest_observability::{
    DIGEST_SITE_LABELS, digest_site_bytes, global_session_content_digest_bytes,
    global_session_encode_bytes, record_session_encode_bytes, rewrite_record_body_decodes,
    session_content_digest_bytes, session_content_digest_computations,
};
pub use error::{AgentError, ToolError};
pub use event::{
    AgentErrorClass, AgentErrorReport, AgentEvent, AssistantImageEvent, BudgetType,
    CompactionFailureReason, CompactionFitByteEvidence, CompactionFitTokenEvidence,
    CompactionPreservedHistoryFit, EventEnvelope, EventSourceIdentity, ExternalToolDelta,
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
pub use exact_operation::{
    CleanupReceipt, ExactOperationIdentity, OperationAcceptClass, OperationAdmissionReceipt,
    OperationAttributionError, OperationCompletion, OperationCompletionAttributionError,
    OperationExecutionScope, OperationTerminal, OperationTerminalIdentity, OperationTerminalScope,
    OperationWaitError, ProjectedTerminalText, ResultProjectionValidationError, TerminalReceipt,
    ValidatedResultProjectionSpec,
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
    peer_response_terminal_context_key,
};
pub use hooks::{
    HookCapability, HookDecision, HookEngine, HookEngineError, HookExecutionMode,
    HookExecutionReport, HookFailureReason, HookId, HookInvocation, HookLlmRequest,
    HookLlmResponse, HookOutcome, HookPoint, HookReasonCode, HookToolCall, HookToolResult,
};
pub use image_content::{
    MissingBlobBehavior, RealtimeOpenProjectionAdmission, RealtimeOpenProjectionAdmissionError,
    RealtimeOpenProjectionLease, RealtimeOpenProjectionLeaseSlot, collect_blob_ids_from_blocks,
    collect_blob_ids_from_messages, externalize_content_blocks, externalize_content_input,
    externalize_messages_from, hydrate_content_blocks, hydrate_content_input,
    hydrate_messages_for_execution,
};
pub use image_generation::*;
pub use interaction::{
    InboxInteraction, InteractionContent, InteractionId, ObjectiveId, PeerIngressAdmission,
    PeerIngressAdmissionDiagnostic, PeerIngressAuthDecision, PeerIngressAuthExemption,
    PeerIngressAuthorityPhase, PeerIngressClaimId, PeerIngressClaimSnapshot,
    PeerIngressClassification, PeerIngressConvention, PeerIngressDeliveryContract,
    PeerIngressDeliveryCorrelation, PeerIngressDequeueAuthority, PeerIngressDequeueFacts,
    PeerIngressDiagnosticDisplay, PeerIngressEntrySnapshot, PeerIngressEnvelopeFacts,
    PeerIngressEnvelopeKind, PeerIngressFact, PeerIngressHandoverState, PeerIngressIdentity,
    PeerIngressKind, PeerIngressPlainEventFacts, PeerIngressQueueClaim, PeerIngressQueueSnapshot,
    PeerIngressReceiveAuthority, PeerIngressReceiveFacts, PeerIngressReceiveOutcome,
    PeerIngressRuntimeSnapshot, PeerIngressTerminalOutcomeCounts, PeerIngressTerminalOutcomeKind,
    PeerInputClass, ResponseStatus, SendResponseCallProjection, TerminalDisposition,
    TerminalityClass, format_external_event_projection, format_peer_ack_projection,
    format_peer_message_projection, format_peer_request_projection,
    format_peer_response_projection, render_peer_ingress_admitted_text,
};
pub use lifecycle::run_primitive::{
    ProviderParamsCarrier, ProviderParamsMergeError, ProviderParamsOverride, ProviderTag,
};
pub use lifecycle::{
    CommittedSessionBoundaryAuthority, ConversationAppend, ConversationAppendRole,
    CoreApplyFailureCause, CoreApplyFailureCauseKind, CoreBoundaryStageError,
    CoreBoundaryStageOutput, CoreControlFailureCause, CoreControlFailureCauseKind, CoreExecutor,
    CoreExecutorBoundaryHandle, CoreExecutorError, CoreExecutorInterruptHandle,
    CoreExecutorPostStopCleanupHandle, CoreExecutorPublicationHandle, CoreExecutorTeardownReason,
    CoreExecutorTurnFinalizationBoundaryHandle, CoreExecutorTurnFinalizationGuard,
    CoreInteractionTerminalPublicationReceipt, CoreRenderable, InputId, RunApplyBoundary,
    RunBoundaryReceipt, RunBoundaryReceiptDraft, RunEvent, RunId, RunPrimitive, StagedRunInput,
};
pub use live_execution::{
    AmbiguousDeliveryNoRetryEvidence, CanonicalContextRevision, CanonicalTranscriptPrefixDigest,
    FinalLiveUserTranscriptCommitError, FinalLiveUserTranscriptCommitEvidence,
    FinalLiveUserTranscriptDisposition, LiveAppendDeliveryOutcome, LiveAppendDeliveryReceipt,
    LiveAssistantPlaybackEvidence, LiveAssistantPlaybackTruncationDisposition,
    LiveAssistantPlaybackTruncationError, LiveAssistantPlaybackTruncationEvidence,
    LiveBridgeCancellationReason, LiveBridgeEffectKind, LiveBridgeEffectOutcome,
    LiveBridgeOperationCorrelation, LiveBridgeOperationPhase, LiveBridgeOutputKind,
    LiveBridgeProviderCorrelation, LiveBridgeRequestDigest, LiveBridgeSubmissionObservation,
    LiveBridgeSubmissionState, LiveChannelId, LiveContextCursor, LiveExecutionCapabilities,
    LiveExecutionChannelPhase, LiveExecutionIdentityError, LiveExecutionMode,
    LiveHandoffInputProvenance, LiveHandoffReconciliation, LiveResultDisposition,
    LiveUserTurnCorrelation, MeerkatExecutionTerminal, NormalizedLiveUserInputDigest,
    OpaqueProviderCorrelation, ProvisionalLiveHandoff,
};
pub use mcp_config::{McpConfig, McpConfigError, McpScope, McpServerConfig, McpServerWithScope};
pub use memory::{
    CompactionCommitCoordinationError, CompactionCommitCoordinator, CompactionHandoffRefusal,
    CompactionProjectionId, CompactionProjectionIntent, CompactionProjectionPersistence,
    CompactionStageReceipt, CompactionStageReconcileReceipt, EmbeddingModel, HnswParams,
    MemoryEnumerationPage, MemoryEnumerationRequest, MemoryIndexBatch, MemoryIndexReceipt,
    MemoryIndexRequest, MemoryIndexScope, MemoryMetadata, MemoryOwner, MemoryRankingPolicy,
    MemoryRecord, MemoryResult, MemoryScopeDropReceipt, MemorySearchScope, MemorySource,
    MemoryStore, MemoryStoreError, MessageRange, SESSION_COMPACTION_PROJECTION_INTENTS_KEY,
};
pub use model_defaults::ModelOperationalDefaultsResolver;
pub use model_profile::catalog::ModelReleaseStage;
pub use model_profile::{ModelCatalog, ModelProfile};
pub use model_registry::{
    ModelCapability, ModelProfileWitness, ModelRegistry, ModelRegistryEntry, SelfHostedServerRef,
    UnsupportedModelCapabilityEvidence, UnsupportedModelCapabilityReason,
};
pub use oauth_identity::OAuthProviderIdentity;
pub use ops::{
    AsyncOpRef, ConcurrencyLimits, ContextStrategy, ForkBranch, ForkBudgetPolicy, OpEvent,
    OperationId, OperationPolicy, OperationResult, OperationSpec, ResultShape, SessionEffect,
    SpawnSpec, ToolAccessConstraint, ToolAccessPolicy, ToolDispatchOutcome,
    ToolDispatchTerminalCause, ToolDispatchTerminalErrorKind, ToolDispatchTimeoutPolicy,
    WaitPolicy, WorkKind,
};
pub use ops_lifecycle::{
    OperationCompletionWatch, OperationCompletionWatchError, OperationKind,
    OperationLifecycleSnapshot, OperationPeerHandle, OperationProgressUpdate,
    OperationRetentionRequest, OperationStatus, OperationTerminalOutcome, OpsLifecycleError,
    OpsLifecycleRegistry, WaitAllResult, WaitAllSatisfied,
};
pub use peer_correlation::{
    InboundPeerRequestState, InteractionStreamAbandonReason, InteractionStreamState,
    OutboundPeerRequestState, PeerCorrelationId,
};
pub use peer_meta::PeerMeta;
pub use persistence_contract::{
    HeadCanonicalProvisionalTailAuthority, ProvisionalTailAuthorityError, RunCheckpointAuthority,
    RunCheckpointReceipt, SessionCheckpointer, SessionControlCommitReceipt,
    WholeBlobProvisionalTailAuthority,
};
pub use placement::{ExecutionPlacement, ExecutionPlacementIdentity, PlacementError};
pub use prompt::{AGENTS_MD_MAX_BYTES, DEFAULT_SYSTEM_PROMPT, SystemPromptConfig};
pub use provider::Provider;
pub use provider_evidence::{
    AuthoredCacheBreakpoint, AuthoredCacheBreakpointRetention, CacheBreakpointBoundary,
    CacheBreakpointDiscardOrigin, CacheBreakpointDiscardReason, CacheBreakpointEvidenceError,
    DISPUTED_MARKER_PREFIX, DiscardedCacheBreakpoint, DiscardedCacheBreakpointIdentity,
    DisputedTurnUsageAccountingIdentity, LoweredRequestEncoding, LoweredRequestProvenance,
    PresentedTokenConvention, ProviderCacheBreakpointClaim, ProviderCacheBreakpointClaimRequest,
    ProviderCacheTtl, ProviderTokenAccounting, TURN_USAGE_ACCOUNTING_DIMENSION,
    TURN_USAGE_ACCOUNTING_IDENTITY_DIMENSION, TargetCacheLoweringCapability,
    TargetCacheLoweringIssuer, TokenAggregationProvenance, UNMEASURED_MARKER_PREFIX,
    UnmeasuredTurnUsageAccounting, ValidatedSourceCacheBreakpoint, canonical_cache_prefix_identity,
    provider_cache_breakpoint_claim,
};
pub use realtime_transcript::{
    LiveAssistantPlaybackTarget, PendingRealtimeUserContentBlob, RealtimeTranscriptApplyOutcome,
    RealtimeTranscriptEvent, RealtimeTranscriptMaterializedMessage, RealtimeTranscriptRole,
    RealtimeUserContentApplyOutcome, RealtimeUserContentIdentity, RealtimeUserContentTombstone,
    SESSION_REALTIME_TRANSCRIPT_STATE_KEY,
};
pub use realtime_transcript_sidecar::{
    REALTIME_TRANSCRIPT_SIDECAR_EVENT_SCHEMA_V1, RealtimeTranscriptSidecarError,
    RealtimeTranscriptSidecarRecord, RealtimeTranscriptSnapshotReasonV1,
};
pub use retry::{
    DEFAULT_STREAM_INACTIVITY_TIMEOUT, LlmRetryFailure, LlmRetryFailureKind, LlmRetryPlan,
    LlmRetrySchedule, RetryPolicy, select_retry_delay,
};
pub use runtime_bootstrap::{
    ContextConfig, DualRootResolution, REALM_MANIFEST_FILE_NAME, RealmConfig, RealmLocator,
    RealmRootChoice, RealmRootDefault, RealmSelection, RuntimeBootstrap, RuntimeBootstrapError,
    default_state_root, derive_workspace_realm_id, fnv1a64_hex, generate_realm_id,
    realm_exists_under, sanitize_realm_id,
};
pub use runtime_epoch::{
    EpochCursorSnapshot, EpochCursorState, RuntimeBuildMode, RuntimeEpochId, SessionRuntimeBindings,
};
pub use schema::{
    CompiledSchema, MeerkatSchema, SchemaCompat, SchemaError, SchemaFormat, SchemaWarning,
};
pub use service::{
    AppendSystemContextRequest, AppendSystemContextResult, AppendSystemContextStatus,
    CreateSessionRequest, DeferredPromptPolicy, DurableSessionForkTarget, ForkCacheInheritance,
    ForkCacheInheritanceInstall, ForkCacheInheritanceUnavailableReason, ForkPoint, ForkPointError,
    MobToolsBuildArgs, MobToolsFactory, PublicTurnToolOverlay, SessionBuildOptions,
    SessionControlError, SessionError, SessionForkAtRequest, SessionForkReplaceRequest,
    SessionForkResult, SessionHistoryPage, SessionHistoryQuery, SessionInfo, SessionQuery,
    SessionService, SessionServiceCommsExt, SessionServiceControlExt, SessionServiceHistoryExt,
    SessionServiceTranscriptEditExt, SessionSummary, SessionTranscriptRestoreRevisionRequest,
    SessionTranscriptRevisionList, SessionTranscriptRevisionListEntry,
    SessionTranscriptRevisionListQuery, SessionTranscriptRevisionPage,
    SessionTranscriptRevisionQuery, SessionTranscriptRewriteRequest,
    SessionTranscriptRewriteResult, SessionUsage, SessionView, StageToolResultsDisposition,
    StageToolResultsRequest, StageToolResultsResult, StartTurnRequest, TranscriptEditError,
    TranscriptEditRunningBehavior, TranscriptReplacement, TranscriptRewriteReason,
    TranscriptRewriteSelection, TranscriptRewriteSemantic, TurnToolOverlay,
    WorkGraphNamespaceGrant,
};
pub use session::{
    AuthorizedSessionToolVisibilityState, ConsumedDeferredTurnInputs, DeferredFirstTurnPhase,
    DeferredToolLoadAuthority, INSTRUCTION_ACTIVATION_RENDER_VERSION_V1,
    ImportedReleased0810Session, InheritedToolVisibilityAuthority,
    InstructionActivationAdmissionErrorCode, InstructionActivationDisposition,
    InstructionActivationError, InstructionActivationErrorCode, InstructionActivationExpectation,
    InstructionActivationKeyState, InstructionActivationMutation,
    InstructionActivationProjectionWitness, InstructionActivationReadPage,
    InstructionActivationReadQuery, InstructionActivationReceipt, InstructionActivationRecord,
    InstructionActivationRequest, InvalidSessionLineageId,
    MAX_INSTRUCTION_ACTIVATION_LINEAGE_BYTES, MAX_INSTRUCTION_BODY_BYTES, PendingDeferredPrompt,
    PendingToolResultsMessage, PersistedSessionMetadataView, PreparedTransientTurnContextBoundary,
    ProviderNativeToolPolicy, Released0810ImportError, Released0810ImportEvidence,
    Released0810ImportReceipt, SESSION_BUILD_STATE_KEY, SESSION_DEFERRED_TURN_STATE_KEY,
    SESSION_LIFECYCLE_TERMINAL_KEY, SESSION_METADATA_SCHEMA_VERSION,
    SESSION_TOOL_VISIBILITY_STATE_KEY, SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
    SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY, SESSION_VERSION, SerializedSessionArtifact,
    Session, SessionBuildState, SessionDeferredTurnState, SessionGeneration,
    SessionHeadMetadataCell, SessionHeadMetadataCellIdentity, SessionHeadMetadataDigest,
    SessionHeadMetadataIdentity, SessionHeadMetadataProjection, SessionHeadMetadataValueDigest,
    SessionLifecycleTerminal, SessionLineageId, SessionLlmIdentity, SessionLlmIdentityOverride,
    SessionLlmIdentityOverrideError, SessionLlmRequestPolicy, SessionMeta, SessionMetadata,
    SessionMetadataDocument, SessionToolVisibilityState, SessionTooling, SystemMessageAppendError,
    SystemPromptUpdateError, SystemPromptUpdateRequest, SystemPromptUpdateResult,
    SystemPromptUpdateStatus, ToolCategoryOverride, ToolCategoryOverrides, ToolVisibilityWitness,
    TranscriptEndpointWitness, TranscriptGraphPrefixAccumulator, TranscriptHistoryState,
    TranscriptParentAdvance, TranscriptRevisionBody, TranscriptRevisionEdge,
    TranscriptRewriteAuditReceiptBatch, TranscriptRewriteCommit, TranscriptRewriteParentTransition,
    TranscriptRewritePatch, TranscriptRewritePrefixAccumulator, TranscriptRewriteRecord,
    TransientTurnContextStateHandle, VIEW_IMAGE_TOOL_NAME, ValidatedTranscriptHistory,
    ValidatedTranscriptRewriteSuffix, WitnessedToolFilter,
    capability_base_filter_for_image_tool_results, extend_transcript_rewrite_prefix_accumulator,
    import_released_0810_session, released_0810_transcript_serialized_rows_digest,
    resolve_session_llm_identity_override, session_metadata_document_from_slice,
    session_metadata_schema_version, session_version,
    transcript_history_full_body_materializations, transcript_messages_digest,
    transcript_rewrite_prefix_digest, try_lifecycle_terminal_from_map,
    try_session_metadata_from_map, validate_current_persisted_transcript_history_slice,
};
pub use session_component_sidecar::{
    ComponentEventDigest, ComponentEventPrefixAuthority, ComponentEventPrefixDigest,
    PreparedComponentEventSuffix, SerializedComponentEvent, SessionComponentKind,
    SessionComponentSidecarError, StoredComponentEventRow, VerifiedComponentEventSequence,
};
pub use session_recovery::{
    BUILD_ONLY_RECOVERY_OVERRIDE_ERROR, RecoveredSessionBuild, RecoveryBackendKind,
    SurfaceSessionRecoveryContext, SurfaceSessionRecoveryError, SurfaceSessionRecoveryOverrides,
    build_recovered_session, has_build_only_turn_overrides, has_materialization_overrides,
    session_allows_first_turn_build_overrides,
};
pub use session_store::{
    HeadCanonicalAuthorityCrossing, HeadCanonicalStoreActivation, IncrementalSessionStore,
    PreparedHeadCanonicalMutationRoute, PreparedHeadCanonicalRewritePreflight, SessionFilter,
    SessionHead, SessionHeadCas, SessionMessageRowPrefixAccumulator, SessionStore,
    SessionStoreError, StrandLayout, StrandRewriteLayout, TranscriptStrandId,
    VerifiedHeadCanonicalAuthority, VerifiedSessionHeadMaterialization,
    head_canonical_plain_save_guard, session_head_cas_token, strand_layout_for_history,
};
pub use state::LoopState;
pub use storage_diagnostics::{
    DatabaseInventory, DiagnoseScope, FindingSeverity, StorageDiagnosis, StorageDiagnosticsError,
    StorageFinding, StorageInventoryEntry, StorageMigrator,
};
pub use storage_durability::{DurabilityClass, DurabilityDeclaration, DurabilityResolution};
pub use storage_layout::{
    ResolvedStorage, StorageLayout, StorageLayoutInputs, find_project_root, local_realms_candidate,
};
pub use streaming_tool::{
    ToolCancellationToken, ToolProgressFrame, ToolProgressFrameError, ToolProgressReportError,
    ToolProgressSink, ToolStreamingDispatchContext,
};
pub use surface_metadata::{
    MEERKAT_METADATA_PREFIX, RESERVED_MOB_LABEL_KEYS, ReservedMetadataKey, RuntimeMetadata,
    SurfaceMetadata, SurfaceMetadataError, is_reserved_meerkat_label_key,
    is_reserved_meerkat_metadata_key, validate_public_app_context, validate_public_labels,
};
pub use tool_catalog::{
    ToolCallability, ToolCatalogCapabilities, ToolCatalogDeferredEligibility, ToolCatalogEntry,
    ToolCatalogLoadRejectedReason, ToolCatalogLoadResolution, ToolCatalogMode, ToolPlaneClass,
    ToolUnavailableReason, deferred_session_entry_count, deferred_session_schema_volume,
    select_catalog_mode_from_snapshot,
};
pub use tool_consequence_policy::{
    ApplicationToolPolicyBinding, BoundToolConsequencePolicy,
    COMPILED_APPLICATION_TOOL_POLICY_SCHEMA_VERSION, CompiledApplicationToolPolicy,
    CompiledApplicationToolPolicyError, CompiledMemberToolAction, CompiledMemberToolGrant,
    CompiledMemberToolGrants, CompiledPolicySourceProvenance, CompiledToolConsequence,
    NoopToolConsequenceObserver, PolicyDigest, PolicyEvaluationProvenance,
    PolicyEvaluationSupervisor, PolicyEvaluationSupervisorConfig, PolicyId, PolicyIdentityError,
    PolicyProviderGeneration, PolicyProviderId, PolicyRevision, ToolConsequenceDenial,
    ToolConsequenceFailure, ToolConsequenceNarrowingPolicy, ToolConsequenceObservation,
    ToolConsequenceObservationOutcome, ToolConsequenceObserver, ToolConsequencePolicyRegistry,
    ToolConsequencePolicySnapshot, ToolConsequenceRequest, ToolConsequenceVerdict,
};
pub use tool_execution::{
    DeadlineChainError, DeadlineChainExtensionError, DetachedToolExecutionPolicy,
    EphemeralToolBindingFingerprint, IdempotencyScope, ResolvedExecutionKind,
    ResolvedToolExecutionPlan, RestartClass, RunnerIdentity, StreamingToolExecutionPolicy,
    ToolCredentialContextRef, ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner,
    ToolExecutionApplicability, ToolExecutionContract, ToolExecutionContractError,
    ToolExecutionDeclarationError, ToolExecutionMode, ToolExecutionOwnerWitness,
    ToolExecutionResolutionContext, ToolExecutionResolutionError, ToolOutputPolicy,
    ToolProgressPolicy, ephemeral_tool_catalog_binding_fingerprint,
};
pub use tool_execution_policy::{
    ExecutionPolicyGatedDispatcher, ToolDispatchAdmission, ToolExecutionPolicy,
    ToolExecutionPolicyError, ToolMutationClass,
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
// NOTE: `turn_terminal::TurnTerminalOutcome` (the classifier payload) is
// deliberately NOT re-exported at the root — `turn_execution_authority::
// TurnTerminalOutcome` already owns that root name; classifier consumers
// path-reference `turn_terminal::TurnTerminalOutcome`.
pub use turn_terminal::{ClassifiedTurnTerminal, TurnTerminalClassifier, TurnTerminalKind};
pub use types::{
    ArtifactRef, AssistantBlock, BlockAssistantMessage, CommsNoticeKind, ContentBlock,
    ContentInput, CumulativeUsage, ExtractionError, HandlingMode, ImageData,
    InstructionActivationId, InstructionActivationIdentity, InstructionContentDigest,
    InstructionKey, InstructionNamespace, InstructionRevisionId, InstructionRevisionRef,
    InvalidInstructionContentDigest, InvalidInstructionIdentifier, MemoryIndexExclusion,
    MemoryIndexableContent, Message, OutputSchema, ProviderMeta, RunInput, RunResult,
    SUPPORTED_VIDEO_MEDIA_TYPES, SecurityMode, ServerToolKind, SessionId, StopReason,
    SystemMessage, SystemMessageIdentity, SystemNoticeBlock, SystemNoticeDirection,
    SystemNoticeKind, SystemNoticeMessage, SystemNoticePeer, SystemPromptKey, SystemPromptVersion,
    SystemPromptVersionIdentity, ToolCall, ToolCallIter, ToolCallView, ToolDef, ToolIdentity,
    ToolName, ToolNameSet, ToolProvenance, ToolResult, ToolSourceId, ToolSourceKind,
    TranscriptMessageIdentity, TranscriptSource, TranscriptUserRole, TurnUsage,
    TurnUsageAccountingMissing, Usage, UserMessage, VideoData,
    assistant_blocks_have_visible_or_actionable_output, has_images, has_non_text_content,
    has_video, is_supported_video_media_type, materialize_latest_system_prompt_versions,
    superseded_system_prompt_offsets, validate_inline_video_blocks,
    validate_system_prompt_version_order,
};
pub use web_search::*;

// === Provider auth v2 (landed ahead of wiring — see
// /Users/luka/.claude/plans/yes-make-a-plan-shimmying-bengio.md) ===
pub use auth::{
    AnthropicAuthMetadata, AnthropicRouteHints, AuthConstraints, AuthError, AuthErrorKind,
    AuthErrorSummary, AuthLease, AuthMetadata, AuthMetadataDefaults, AuthRefreshReason,
    AuthRouteHints, AuthStatus, AuthStatusPhase, GoogleAuthMetadata, GoogleRouteHints,
    HttpAuthorizationContent, HttpAuthorizationReceipt, HttpAuthorizationRequest,
    HttpAuthorizationResponse, HttpAuthorizationResponseAction, HttpAuthorizer, OpenAiAuthMetadata,
    OpenAiRouteHints, ProviderAuthMetadata, PublishedAuthStatus, RefreshFailureObservation,
    ResolvedAuthEnvelope, ResolvedAuthKind, TokenLifecycleClearError,
    clear_tokens_and_publish_lifecycle_released,
    clear_tokens_and_publish_lifecycle_released_for_identity, lease_snapshot_expires_at_datetime,
    mark_tokens_lifecycle_published_for_transition,
    oauth_status_projection_snapshot_from_newer_marker, persisted_auth_mode_is_directly_creatable,
    persisted_auth_mode_uses_oauth_login_lifecycle, persisted_token_expires_at_epoch_secs,
    project_published_auth_status, publish_token_lifecycle_acquired,
    publish_token_lifecycle_acquired_for_identity, publish_token_lifecycle_released,
    publish_token_lifecycle_released_for_identity, restore_token_lifecycle_snapshot,
    tokens_lifecycle_publication, tokens_lifecycle_publication_with_explicit_expiry,
    tokens_lifecycle_published, tokens_lifecycle_published_generation,
};
#[cfg(not(target_arch = "wasm32"))]
pub use auth::{
    AuthLoginLifecycleGuard, AuthStatusRehydrateError, acquire_auth_login_lifecycle_guard,
    clear_tokens_and_publish_lifecycle_released_coordinated,
    clear_tokens_and_publish_lifecycle_released_coordinated_for_identity,
    rehydrate_durable_predecessor_for_mutation,
    rehydrate_durable_predecessor_for_mutation_for_identity, rehydrate_marked_tokens_for_status,
    rehydrate_marked_tokens_for_status_for_identity,
};
pub use connection::{
    AuthBindingRef, AuthCredentialIdentity, AuthProfile, AuthProfileConfig, BackendProfile,
    BackendProfileConfig, BindingId, BindingOrigin, BindingPolicy, ConnectionTargetError,
    CredentialAccountBindingCandidates, CredentialAccountId, CredentialAccountRef,
    CredentialSourceSpec, ExternalResolverId, IdentityError, MemberCommsName, MemberCommsNameError,
    MobMemberBinding, PeerRole, ProfileId, ProviderBinding, ProviderBindingConfig,
    ProviderBindingError, RealmConfigSection, RealmConnectionSet, RealmId,
    ResolvedConnectionTarget, mob_realm_id, resolve_auth_binding_candidates_for_provider,
    resolve_auth_binding_or_default_for_provider, resolve_credential_account_binding_for_provider,
    resolve_explicit_auth_binding_target, resolve_realm_binding_target_for_provider,
};
pub use self_hosted_binding::{
    SelfHostedBindingCandidate, SelfHostedBindingServer, SelfHostedConnectionError,
    resolve_self_hosted_binding_for_server, self_hosted_binding_server,
    validate_explicit_self_hosted_target,
};
