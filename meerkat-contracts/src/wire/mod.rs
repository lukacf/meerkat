//! Canonical wire response types.

mod approval;
mod artifact;
mod auth;
mod comms;
mod config;
mod connection;
mod error;
mod event;
mod help;
mod host;
mod image_generation;
mod live;
mod mcp_live;
mod mob;
mod models;
mod params;
mod realtime;
mod rest;
mod result;
mod rpc_surface;
pub mod runtime;
mod schedule;
mod session;
pub mod skills;
mod stream_read;
pub mod supervisor_bridge;
mod usage;
mod workgraph;

pub use approval::{
    ApprovalActionKind, ApprovalDecideParams, ApprovalDecision, ApprovalDecisionRecord,
    ApprovalGetParams, ApprovalId, ApprovalListFilter, ApprovalListParams, ApprovalListResult,
    ApprovalMemberRef, ApprovalMobRef, ApprovalOwnerRef, ApprovalPrincipalId,
    ApprovalProposedAction, ApprovalRecord, ApprovalRequest, ApprovalRequestParams,
    ApprovalResourceId, ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk, ApprovalStatus,
};
pub use comms::{
    CommsChecksumTokenParams, CommsChecksumTokenResult, CommsChecksumTokenResultIntent,
    CommsCommandError, CommsCommandProjectionError, CommsCommandRequest, CommsPeerEntry,
    CommsPeerLifecycleParams, CommsPeerRequestIntent, CommsPeerRequestParams,
    CommsPeerResponseResult, CommsPeerUnreachableReason, CommsPeersParams, CommsPeersResult,
    CommsSendErrorData, CommsSendParams, CommsSendResult, HandlingMode as WireCommsHandlingMode,
    InputSource as WireCommsInputSource, InputStreamMode as WireCommsInputStreamMode, PeerAddress,
    PeerCapabilitySet, PeerDirectoryEntry, PeerDirectoryListing, PeerDirectorySource, PeerId,
    PeerName as WireCommsPeerName, PeerSendability, PeerTransport,
    ResponseStatus as WireCommsResponseStatus, SendTaintOverride, SenderContentTaint,
};
#[cfg(not(target_arch = "wasm32"))]
pub use config::ConfigWriteResult;
pub use config::{
    ConfigPatchParams, ConfigSetParams, WireLiveChannelCloseFailure, WireLiveChannelRefreshFailure,
    WireLiveCloseFailure, WireLiveConfigPropagationReport, WireLiveHotSwapSkip,
    WireLiveHotSwapSkipReason, WireLiveRefreshFailure, WireLiveSwapFailure,
};
pub use connection::{
    BindingIdParams, CreateProfileParams, DeviceCompleteParams, DeviceStartParams,
    LoginCompleteParams, LoginStartParams, ProvisionApiKeyParams, RealmIdParams,
    WireAuthBindingRef, WireAuthError, WireAuthProfile, WireAuthProfileCleared,
    WireAuthProfileCreated, WireAuthProfileDetail, WireAuthProfilesList, WireAuthStatus,
    WireAuthStatusDetail, WireBackendProfile, WireBindingIdentity, WireDeviceCompleteResult,
    WireDeviceStart, WireLoginReady, WireLoginStart, WireProviderBinding,
    WireProvisionApiKeyResult, WireRealmConnectionSet, WireRealmList, WireRealmSummary,
};
pub use rpc_surface::{
    ArchiveSessionParams, BlobGetParams, CallbackToolDefinition, DeferredCreateResult,
    InjectSystemContextParams, InjectSystemContextResult, InterruptParams, ListSessionsParams,
    ListSessionsResult, ReadSessionHistoryParams, ReadSessionParams, ScheduleToolCallParams,
    ScheduleToolsResult, ServerCapabilities, ServerInfo, SessionInputStateParams,
    SessionInputStateResult, SessionInputStateSelector, ToolsRegisterParams, ToolsRegisterResult,
};

pub use artifact::{
    ArtifactDownloadParams, ArtifactDownloadResult, ArtifactIdParams, ArtifactListParams,
    ArtifactListResult,
};
pub use auth::{
    ActingOnBehalfOf, AuthGrant, GrantAction, GrantScope, PrincipalId, PrincipalKind, PrincipalRef,
    VisibilityClass,
};
pub use error::WireConversionError;
pub use event::{
    EventReplayCursor, EventReplayCursorError, EventReplayEnvelope, EventReplayEventId,
    EventReplayScope, EventsLatestCursorParams, EventsLatestCursorResult, EventsListSinceParams,
    EventsListSinceResult, EventsSnapshotBody, EventsSnapshotParams, EventsSnapshotResult,
    WireEvent,
};
pub use help::{HelpExecutionMode, HelpRequest, HelpResponse};
pub use host::{
    RuntimeHostCapabilities, RuntimeHostEndpointProjection, RuntimeHostFeatureFlags,
    RuntimeHostHealth, RuntimeHostHealthStatus, RuntimeHostIdScope, RuntimeHostInfo,
    RuntimeHostRealmProjection,
};
pub use image_generation::{
    WireAssistantImageRef, WireGenerateImageExecutionPlan, WireGenerateImageRequest,
    WireImageGenerationToolResult, WireImageOperationPhase, WireModelRoutingApprovalPhase,
    WireScopedModelOverride, WireSessionModelRoutingStatus, WireSwitchTurnControlResult,
    WireSwitchTurnIntent, WireSwitchTurnPhase,
};
pub use live::{
    LiveChannelParams, LiveCloseResult, LiveCloseStatus, LiveCommitInputParams,
    LiveCommitInputResult, LiveCommitInputStatus, LiveInputChunkWire, LiveInterruptResult,
    LiveInterruptStatus, LiveOpenParams, LiveOpenResult, LiveOpenTransport, LiveRefreshResult,
    LiveRefreshStatus, LiveSendInputErrorData, LiveSendInputParams, LiveSendInputResult,
    LiveSendInputStatus, LiveStatusResult, LiveTruncateParams, LiveTruncateResult,
    LiveTruncateStatus, LiveWebrtcAnswerParams, LiveWebrtcAnswerResult, WireLiveAdapterErrorCode,
    WireLiveAdapterObservation, WireLiveAdapterStatus, WireLiveChannelCapabilities,
    WireLiveConfigRejectionReason, WireLiveContinuityMode, WireLiveDegradationReason,
    WireLiveResponseModality, WireLiveTransportBootstrap, WireProvider,
    WireRealtimeTranscriptEvent,
};
pub use mcp_live::{
    McpAddParams, McpLiveOpResponse, McpLiveOpStatus, McpLiveOperation, McpReloadParams,
    McpRemoveParams,
};
pub use mob::{
    MobAppendSystemContextParams, MobAppendSystemContextResult, MobBackendConfigInput,
    MobCancelAllWorkParams, MobCancelAllWorkResult, MobCancelWorkParams, MobCancelWorkResult,
    MobCollectionPolicyInput, MobConcludeObjectiveParams, MobConcludeObjectiveResult,
    MobConditionExprInput, MobCreateParams, MobCreateResult, MobDefinitionInput,
    MobDependencyModeInput, MobDestroyResult, MobDispatchModeInput, MobEnsureMemberOutcomeWire,
    MobEnsureMemberParams, MobEnsureMemberResult, MobEventRouterConfigInput, MobEventsParams,
    MobEventsResult, MobExternalBackendConfigInput, MobFlowCancelParams, MobFlowCancelResult,
    MobFlowNodeInput, MobFlowRunParams, MobFlowRunResult, MobFlowSpecInput, MobFlowStatusParams,
    MobFlowStatusResult, MobFlowStepInput, MobFlowsResult, MobForceCancelResult,
    MobForkHelperParams, MobFrameSpecInput, MobFrameStepInput, MobHelperResult, MobIdParams,
    MobIngressInteractionParams, MobIngressInteractionResult, MobLifecycleParams,
    MobLifecycleResult, MobLimitsSpecInput, MobListMembersMatchingParams,
    MobListMembersMatchingResult, MobListResult, MobMemberFilterWire, MobMemberListEntryWire,
    MobMemberParams, MobMemberSendParams, MobMemberSendResult, MobMemberSpecWire,
    MobMemberStatusResult, MobMembersResult, MobOrchestratorInput, MobPeerTarget,
    MobPolicyModeInput, MobProfileBindingInput, MobProfileCreateParams, MobProfileDeleteParams,
    MobProfileDeleteResult, MobProfileInput, MobProfileListResult, MobProfileLookupResult,
    MobProfileNameParams, MobProfileUpdateParams, MobReconcileFailureWire, MobReconcileOptionsWire,
    MobReconcileParams, MobReconcileReportWire, MobReconcileResult, MobRepeatUntilInput,
    MobRespawnParams, MobRespawnReceipt, MobRespawnResult, MobRetireResult, MobRoleWiringRuleInput,
    MobRotateSupervisorResult, MobRunParams, MobRunResult, MobRunResultParams, MobSkillSourceInput,
    MobSnapshotResult, MobSpawnHelperParams, MobSpawnManyFailedResult, MobSpawnManyFailureCause,
    MobSpawnManyParams, MobSpawnManyResult, MobSpawnManyResultEntry, MobSpawnManyResultPayload,
    MobSpawnManyResultStatus, MobSpawnManySpawnedResult, MobSpawnParams, MobSpawnPolicyInput,
    MobSpawnReceiptWire, MobSpawnResult, MobSpawnSpecParams, MobStatusResult,
    MobStepOutputFormatInput, MobStreamCloseParams, MobStreamCloseResult, MobStreamOpenParams,
    MobStreamOpenResult, MobSubmitWorkParams, MobSubmitWorkResult, MobSupervisorSpecInput,
    MobToolConfigInput, MobTopologyRuleInput, MobTopologySpecInput, MobTurnStartParams,
    MobUnwireParams, MobUnwireResult, MobWaitMembersResult, MobWaitParams, MobWireMembersBatchEdge,
    MobWireMembersBatchParams, MobWireMembersBatchResult, MobWireParams, MobWireResult,
    MobWiringRulesInput, SupervisorRotationIncompleteDataWire,
    SupervisorRotationIncompleteDetailsWire, SupervisorRotationIncompleteKind,
    SupervisorRotationReportWire, SupervisorRotationRetryAuthority, SupervisorRotationRetryScope,
    WireAgentRuntimeId, WireAppendSystemContextStatus, WireBudgetSplitPolicy, WireForkContext,
    WireHandlingMode, WireMemberHealthClass, WireMemberLaunchMode, WireMemberProgressEvent,
    WireMemberProgressSnapshot, WireMemberRef, WireMemberRefError, WireMemberRunState,
    WireMobBackendKind, WireMobError, WireMobLifecycleAction, WireMobLifecycleStatus,
    WireMobMemberStatus, WireMobProfile, WireMobReconcileStage, WireMobRespawnOutcome,
    WireMobResumeOverrideField, WireMobRun, WireMobRunResultEnvelope, WireMobRunStatus,
    WireMobRuntimeMode, WireMobToolConfig, WireMobWireAction, WirePeerConnectivity,
    WirePeerConnectivitySnapshot, WireRenderClass, WireRenderMetadata, WireRenderSalience,
    WireRuntimeBinding, WireToolAccessPolicy, WireToolFilter, WireTrustedPeerIdentity,
    WireTrustedPeerSpec, WireUnreachablePeer, WireWorkOrigin,
};
pub use models::{
    CatalogModelEntry, ModelsCatalogResponse, ProviderCatalog, WireModelBetaHeader,
    WireModelProfile, WireModelTier, WireResolvedModelCapabilities,
};
pub use params::{CommsParams, CoreCreateParams, HookParams, SkillsParams, StructuredOutputParams};
pub use realtime::{
    RealtimeAudioChunk, RealtimeAudioFormat, RealtimeCapabilities, RealtimeImageChunk,
    RealtimeInputChunk, RealtimeInputKind, RealtimeOutputKind, RealtimeTextChunk,
    RealtimeTurningMode, RealtimeVideoChunk,
};
pub use rest::{
    RestAppendSystemContextRequest, RestAuthBindingTestRequest, RestAuthProfileCreateRequest,
    RestContinueSessionRequest, RestCreateSessionRequest, RestMobForkHelperRequest,
    RestMobHelperRequest, RestMobWaitRequest, RestMobWireMembersBatchRequest,
    RestPatchConfigRequest, RestPeerResponseTerminalRequest, RestSessionDetailsResponse,
    RestSetConfigRequest,
};
pub use result::{
    WireCallbackPending, WireCallbackPendingStatus, WirePendingToolCall, WireRunResult,
};
pub use runtime::{
    PeerResponseTerminalStatusWire,
    RuntimeAcceptOutcomeType,
    RuntimeAcceptResult,
    RuntimeStateResult,
    SessionExternalEventEnvelope,
    SessionPeerResponseTerminalParams,
    // Re-export of the `StructuredProviderExtension` core relocation
    // from C-1 — external callers can still import via the wire path.
    StructuredProviderExtension,
    WireInputLifecycleState,
    WireInputState,
    WireInputStateHistoryEntry,
    WireRuntimeState,
};
pub use schedule::{
    ListSchedulesParams, Occurrence, Schedule, ScheduleIdParams, ScheduleListResult,
    ScheduleOccurrencesParams, ScheduleOccurrencesResult, UpdateScheduleParams,
};
pub use session::{
    ForkSessionAtParams, ForkSessionReplaceParams, InterruptResult,
    ListSessionTranscriptRevisionsParams, ReadSessionTranscriptRevisionParams,
    RestoreSessionTranscriptRevisionParams, RevisionId, RevisionSelector,
    RewriteSessionTranscriptParams, SessionStreamCloseParams, SessionStreamCloseResult,
    SessionStreamOpenParams, SessionStreamOpenResult, TranscriptRewriteMessage, WireAssistantBlock,
    WireContentBlock, WireContentInput, WireInterruptOutcome, WirePromptInput, WireProviderMeta,
    WireSessionHistory, WireSessionInfo, WireSessionMessage, WireSessionSummary,
    WireSessionTranscriptRevision, WireSessionTranscriptRevisionEntry,
    WireSessionTranscriptRevisionList, WireStopReason, WireToolResult, WireToolResultContent,
    WireTranscriptSource,
};
pub use skills::{SkillEntry, SkillInspectResponse, SkillListResponse, SkillSourceProvenance};
pub use stream_read::StreamReadStatus;
pub use supervisor_bridge::{
    BridgeAck, BridgeBindPayload, BridgeBindResponse, BridgeCapabilities, BridgeCommand,
    BridgeCommandDecodeError, BridgeDeliveryOutcome, BridgeDeliveryPayload,
    BridgeDeliveryRejectionCause, BridgeDeliveryResponse, BridgeDestroyResponse,
    BridgeHardCancelPayload, BridgeMemberRuntimeState, BridgeMobPeerOverlayHandoff,
    BridgeObservationResponse, BridgePeerConnectivity, BridgePeerSpec, BridgePeerWiringPayload,
    BridgeProtocolVersion, BridgeReply, BridgeRetireResponse, BridgeSupervisorDelivery,
    BridgeSupervisorPayload, BridgeSupervisorRotationObservation, BridgeSupervisorRotationObserve,
    BridgeSupervisorRotationOperationReceipt, BridgeSupervisorRotationPendingPhase,
    BridgeSupervisorRotationRejectionCause, BridgeSupervisorRotationRejectionReceipt,
    BridgeSupervisorRotationState, BridgeSupervisorRotationSubmit,
    BridgeSupervisorRotationTargetReceipt, SUPERVISOR_BRIDGE_CURRENT_PROTOCOL_VERSION,
    SUPERVISOR_BRIDGE_DEFAULT_PROTOCOL_VERSION, SUPERVISOR_BRIDGE_INTENT,
    SUPERVISOR_BRIDGE_PROTOCOL_VERSION, SUPERVISOR_BRIDGE_SUPPORTED_PROTOCOL_VERSIONS,
    SupervisorRotationOperationId, UnsupportedBridgeProtocolVersion, decode_bridge_command,
    supervisor_bridge_current_protocol_version, supervisor_bridge_default_protocol_version,
    supervisor_bridge_protocol_version_supported, supervisor_bridge_supported_protocol_versions,
};
pub use usage::WireUsage;
pub use workgraph::{WorkEventsResult, WorkItemsResult};
