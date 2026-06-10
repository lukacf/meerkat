//! Canonical wire response types.

mod approval;
mod artifact;
mod auth;
mod comms;
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
mod result;
mod rpc_surface;
pub mod runtime;
mod schedule;
mod session;
pub mod skills;
mod stream_read;
pub mod supervisor_bridge;
mod usage;

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
    CommsPeerResponseResult, CommsPeersParams, CommsPeersResult, CommsSendParams, CommsSendResult,
    HandlingMode as WireCommsHandlingMode, InputSource as WireCommsInputSource,
    InputStreamMode as WireCommsInputStreamMode, PeerAddress, PeerCapabilitySet,
    PeerDirectoryEntry, PeerDirectoryListing, PeerDirectorySource, PeerId,
    PeerName as WireCommsPeerName, PeerSendability, PeerTransport,
    ResponseStatus as WireCommsResponseStatus,
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
    ArchiveSessionParams, BlobGetParams, DeferredCreateResult, InjectSystemContextParams,
    InjectSystemContextResult, InterruptParams, ListSessionsParams, ListSessionsResult,
    ReadSessionHistoryParams, ReadSessionParams, ScheduleToolCallParams, ScheduleToolsResult,
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
    WireModelRoutingApprovalRequest, WireScopedModelOverride, WireSessionModelRoutingStatus,
    WireSwitchTurnControlResult, WireSwitchTurnIntent, WireSwitchTurnPhase,
};
pub use live::{
    LiveChannelParams, LiveCloseResult, LiveCloseStatus, LiveCommitInputParams,
    LiveCommitInputResult, LiveCommitInputStatus, LiveInputChunkWire, LiveInterruptResult,
    LiveInterruptStatus, LiveOpenParams, LiveOpenResult, LiveOpenTransport, LiveRefreshResult,
    LiveRefreshStatus, LiveSendInputParams, LiveSendInputResult, LiveSendInputStatus,
    LiveStatusResult, LiveTruncateParams, LiveTruncateResult, LiveTruncateStatus,
    LiveWebrtcAnswerParams, LiveWebrtcAnswerResult, WireLiveAdapterErrorCode,
    WireLiveAdapterObservation, WireLiveAdapterStatus, WireLiveChannelCapabilities,
    WireLiveConfigRejectionReason, WireLiveContinuityMode, WireLiveDegradationReason,
    WireLiveResponseModality, WireLiveTransportBootstrap, WireProvider,
};
pub use mcp_live::{
    McpAddParams, McpLiveOpResponse, McpLiveOpStatus, McpLiveOperation, McpReloadParams,
    McpRemoveParams,
};
pub use mob::{
    MobAppendSystemContextParams, MobAppendSystemContextResult, MobBackendConfigInput,
    MobCancelAllWorkParams, MobCancelAllWorkResult, MobCancelWorkParams, MobCancelWorkResult,
    MobCollectionPolicyInput, MobConditionExprInput, MobCreateParams, MobCreateResult,
    MobDefinitionInput, MobDependencyModeInput, MobDestroyResult, MobDispatchModeInput,
    MobEnsureMemberOutcomeWire, MobEnsureMemberParams, MobEnsureMemberResult,
    MobEventRouterConfigInput, MobEventsParams, MobEventsResult, MobExternalBackendConfigInput,
    MobFlowCancelParams, MobFlowCancelResult, MobFlowNodeInput, MobFlowRunParams, MobFlowRunResult,
    MobFlowSpecInput, MobFlowStatusParams, MobFlowStatusResult, MobFlowStepInput, MobFlowsResult,
    MobForceCancelResult, MobForkHelperParams, MobFrameSpecInput, MobFrameStepInput,
    MobHelperResult, MobIdParams, MobIngressInteractionParams, MobIngressInteractionResult,
    MobLifecycleParams, MobLifecycleResult, MobLimitsSpecInput, MobListMembersMatchingParams,
    MobListMembersMatchingResult, MobListResult, MobMemberFilterWire, MobMemberListEntryWire,
    MobMemberParams, MobMemberSendParams, MobMemberSendResult, MobMemberSpecWire,
    MobMemberStatusResult, MobMembersResult, MobOrchestratorInput, MobPeerTarget,
    MobPolicyModeInput, MobProfileBindingInput, MobProfileCreateParams, MobProfileDeleteParams,
    MobProfileDeleteResult, MobProfileInput, MobProfileListResult, MobProfileLookupResult,
    MobProfileNameParams, MobProfileUpdateParams, MobReconcileFailureWire, MobReconcileOptionsWire,
    MobReconcileParams, MobReconcileReportWire, MobReconcileResult, MobRepeatUntilInput,
    MobRespawnParams, MobRespawnReceipt, MobRespawnResult, MobRetireResult, MobRoleWiringRuleInput,
    MobRotateSupervisorResult, MobSkillSourceInput, MobSnapshotResult, MobSpawnHelperParams,
    MobSpawnManyFailedResult, MobSpawnManyFailureCause, MobSpawnManyParams, MobSpawnManyResult,
    MobSpawnManyResultEntry, MobSpawnManyResultPayload, MobSpawnManyResultStatus,
    MobSpawnManySpawnedResult, MobSpawnParams, MobSpawnPolicyInput, MobSpawnReceiptWire,
    MobSpawnResult, MobSpawnSpecParams, MobStatusResult, MobStepOutputFormatInput,
    MobStreamCloseParams, MobStreamCloseResult, MobStreamOpenParams, MobStreamOpenResult,
    MobSubmitWorkParams, MobSubmitWorkResult, MobSupervisorSpecInput, MobToolConfigInput,
    MobTopologyRuleInput, MobTopologySpecInput, MobTurnStartParams, MobUnwireParams,
    MobUnwireResult, MobWaitMembersResult, MobWaitParams, MobWireMembersBatchEdge,
    MobWireMembersBatchParams, MobWireMembersBatchResult, MobWireParams, MobWireResult,
    MobWiringRulesInput, SupervisorRotationReportWire, WireAgentRuntimeId,
    WireAppendSystemContextStatus, WireBudgetSplitPolicy, WireForkContext, WireHandlingMode,
    WireMemberLaunchMode, WireMemberRef, WireMemberRefError, WireMemberState, WireMobBackendKind,
    WireMobError, WireMobLifecycleAction, WireMobLifecycleStatus, WireMobMemberStatus,
    WireMobProfile, WireMobReconcileStage, WireMobRespawnOutcome, WireMobRun, WireMobRunStatus,
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
    RealtimeAudioChunk, RealtimeAudioFormat, RealtimeCapabilities, RealtimeInputChunk,
    RealtimeInputKind, RealtimeOutputKind, RealtimeTextChunk, RealtimeTurningMode,
    RealtimeVideoChunk,
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
    ListSchedulesParams, ScheduleIdParams, ScheduleListResult, ScheduleOccurrencesParams,
    ScheduleOccurrencesResult, UpdateScheduleParams,
};
pub use session::{
    ForkSessionAtParams, ForkSessionReplaceParams, ReadSessionTranscriptRevisionParams,
    RestoreSessionTranscriptRevisionParams, RevisionId, RevisionSelector,
    RewriteSessionTranscriptParams, SessionStreamCloseParams, SessionStreamCloseResult,
    SessionStreamOpenParams, SessionStreamOpenResult, TranscriptRewriteMessage, WireAssistantBlock,
    WireContentBlock, WireContentInput, WireProviderMeta, WireSessionHistory, WireSessionInfo,
    WireSessionMessage, WireSessionSummary, WireSessionTranscriptRevision, WireStopReason,
    WireToolResult, WireToolResultContent, WireTranscriptSource,
};
pub use skills::{SkillEntry, SkillInspectResponse, SkillListResponse, SkillSourceProvenance};
pub use stream_read::StreamReadStatus;
pub use supervisor_bridge::{
    BridgeAck, BridgeBindPayload, BridgeBindResponse, BridgeCapabilities, BridgeCommand,
    BridgeCommandDecodeError, BridgeDeliveryOutcome, BridgeDeliveryPayload,
    BridgeDeliveryRejectionCause, BridgeDeliveryResponse, BridgeDestroyResponse,
    BridgeHardCancelPayload, BridgeMemberRuntimeState, BridgeMobPeerOverlayHandoff,
    BridgeObservationResponse, BridgePeerConnectivity, BridgePeerSpec, BridgePeerWiringPayload,
    BridgeProtocolVersion, BridgeReply, BridgeRetireResponse, BridgeSupervisorPayload,
    SUPERVISOR_BRIDGE_CURRENT_PROTOCOL_VERSION, SUPERVISOR_BRIDGE_DEFAULT_PROTOCOL_VERSION,
    SUPERVISOR_BRIDGE_INTENT, SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
    SUPERVISOR_BRIDGE_SUPPORTED_PROTOCOL_VERSIONS, UnsupportedBridgeProtocolVersion,
    decode_bridge_command, supervisor_bridge_current_protocol_version,
    supervisor_bridge_default_protocol_version, supervisor_bridge_protocol_version_supported,
    supervisor_bridge_supported_protocol_versions,
};
pub use usage::WireUsage;
