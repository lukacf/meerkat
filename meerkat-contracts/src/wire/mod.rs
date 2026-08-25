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
pub mod jobs;
mod live;
mod mcp_live;
mod mob;
mod models;
mod params;
mod portable_spec;
mod realtime;
mod rest;
mod result;
mod rpc_surface;
pub mod runtime;
mod schedule;
mod session;
pub mod skills;
mod spec_digest;
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
    ExportAtifParams, InjectSystemContextParams, InjectSystemContextResult, InterruptParams,
    ListSessionsParams, ListSessionsResult, ReadSessionHistoryParams, ReadSessionParams,
    ScheduleToolCallParams, ScheduleToolsResult, ServerCapabilities, ServerInfo,
    SessionInputStateParams, SessionInputStateResult, SessionInputStateSelector,
    ToolsRegisterParams, ToolsRegisterResult,
};

pub use artifact::{
    ArtifactDownloadParams, ArtifactDownloadResult, ArtifactIdParams, ArtifactListParams,
    ArtifactListResult,
};
pub use auth::{
    ActingOnBehalfOf, AuthGrant, GrantAction, GrantScope, PrincipalId, PrincipalKind, PrincipalRef,
    VisibilityClass,
};
pub use error::{
    WireConversionError, WireHostUnavailableDetail, WireMobErrorDetail, WireStaleCursorDetail,
    WireStaleFenceDetail,
};
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
pub use jobs::*;
pub use live::{
    LIVE_CLIENT_CONTEXT_V1_CAPABILITY, LIVE_EXECUTION_IDENTITY_V1_CAPABILITY,
    LIVE_FUNCTION_BRIDGE_V1_CAPABILITY, LiveAssistantOutputAvailableParams, LiveChannelParams,
    LiveCloseResult, LiveCloseStatus, LiveCommitInputParams, LiveCommitInputResult,
    LiveCommitInputStatus, LiveInputChunkWire, LiveInterruptResult, LiveInterruptStatus,
    LiveOpenParams, LiveOpenResult, LiveOpenTransport, LivePlaybackCompleteParams,
    LivePlaybackCompleteResult, LivePlaybackCompleteStatus, LiveRefreshResult, LiveRefreshStatus,
    LiveSendInputErrorData, LiveSendInputParams, LiveSendInputResult, LiveSendInputStatus,
    LiveStatusResult, LiveTruncateParams, LiveTruncateResult, LiveTruncateStatus,
    LiveWebrtcAnswerParams, LiveWebrtcAnswerResult, WireLiveAdapterErrorCode,
    WireLiveAdapterObservation, WireLiveAdapterStatus, WireLiveAuthBindingRef,
    WireLiveChannelCapabilities, WireLiveConfigRejectionReason, WireLiveContinuityMode,
    WireLiveDegradationReason, WireLiveExecutionIdentityOverrideV1,
    WireLiveExecutionIdentityVersion, WireLiveIdentityOverride, WireLiveResponseModality,
    WireLiveTransportBootstrap, WireProvider, WireRealtimeTranscriptEvent,
};
pub use mcp_live::{
    McpAddParams, McpLiveOpResponse, McpLiveOpStatus, McpLiveOperation, McpReloadParams,
    McpRemoveParams,
};
pub use mob::{
    MOB_RUN_ACCOUNTING_UNPRICED_REASON, MobAdoptMemberIdentityDeclarationParams,
    MobAdoptMemberIdentityDeclarationResult, MobAppendSystemContextParams,
    MobAppendSystemContextResult, MobApplyMemberToolDeclarationParams,
    MobApplyMemberToolDeclarationResult, MobBackendConfigInput, MobBindHostParams,
    MobBindHostResult, MobBoundedHelperResult, MobBoundedHelperResultStatus,
    MobCancelAllWorkParams, MobCancelAllWorkResult, MobCancelWorkParams, MobCancelWorkResult,
    MobCollectionPolicyInput, MobConcludeObjectiveParams, MobConcludeObjectiveResult,
    MobConditionExprInput, MobCreateParams, MobCreateResult, MobDefinitionInput,
    MobDependencyModeInput, MobDestroyResult, MobDispatchModeInput, MobEnsureMemberOutcomeWire,
    MobEnsureMemberParams, MobEnsureMemberResult, MobEventRouterConfigInput, MobEventsParams,
    MobEventsResult, MobExternalBackendConfigInput, MobFlowCancelParams, MobFlowCancelResult,
    MobFlowNodeFailurePolicyInput, MobFlowNodeInput, MobFlowRunParams, MobFlowRunResult,
    MobFlowSpecInput, MobFlowStatusParams, MobFlowStatusResult, MobFlowStepInput, MobFlowsResult,
    MobForceCancelResult, MobForkHelperParams, MobFrameSpecInput, MobFrameStepInput,
    MobGrantScopesParams, MobGrantScopesResult, MobGrantsResult, MobHardCancelParams,
    MobHardCancelResult, MobHelperResult, MobHostStatus, MobHostsResult, MobIdParams,
    MobIngressInteractionParams, MobIngressInteractionResult, MobLifecycleParams,
    MobLifecycleResult, MobLimitsSpecInput, MobListMembersMatchingParams,
    MobListMembersMatchingResult, MobListResult, MobMemberFilterWire, MobMemberHistoryParams,
    MobMemberHistoryResult, MobMemberListEntryWire, MobMemberLiveChannelParams,
    MobMemberLiveControlParams, MobMemberLiveOpenParams, MobMemberLiveStatusParams,
    MobMemberParams, MobMemberSendParams, MobMemberSendResult, MobMemberSpecWire,
    MobMemberStatusResult, MobMemberToolDeclarationParams, MobMemberToolDeclarationResult,
    MobMembersResult, MobOrchestratorInput, MobPeerTarget, MobPolicyModeInput,
    MobProfileBindingInput, MobProfileCreateParams, MobProfileDeleteParams, MobProfileDeleteResult,
    MobProfileInput, MobProfileListResult, MobProfileLookupResult, MobProfileNameParams,
    MobProfileUpdateParams, MobReconcileFailureWire, MobReconcileOptionsWire, MobReconcileParams,
    MobReconcileReportWire, MobReconcileResult, MobRepeatUntilInput,
    MobResolveIdentityConvergenceBlockParams, MobResolveIdentityConvergenceBlockResult,
    MobRespawnParams, MobRespawnReceipt, MobRespawnResult, MobRetireResult, MobRevokeHostParams,
    MobRevokeHostResult, MobRevokeScopesParams, MobRevokeScopesResult, MobRoleWiringRuleInput,
    MobRotateSupervisorResult, MobRouteInstallsResult, MobRunParams, MobRunResult,
    MobRunResultParams, MobSkillSourceInput, MobSnapshotResult, MobSpawnHelperParams,
    MobSpawnManyFailedResult, MobSpawnManyFailureCause, MobSpawnManyParams, MobSpawnManyResult,
    MobSpawnManyResultEntry, MobSpawnManyResultPayload, MobSpawnManyResultStatus,
    MobSpawnManySpawnedResult, MobSpawnParams, MobSpawnPolicyInput, MobSpawnReceiptWire,
    MobSpawnResult, MobSpawnSpecParams, MobStatusResult, MobStepOutputFormatInput,
    MobStreamCloseParams, MobStreamCloseResult, MobStreamOpenParams, MobStreamOpenResult,
    MobSubmitWorkParams, MobSubmitWorkResult, MobSupervisorSpecInput, MobToolConfigInput,
    MobTopologyRuleInput, MobTopologySpecInput, MobTurnStartParams, MobUnwireParams,
    MobUnwireResult, MobWaitMembersResult, MobWaitParams, MobWireMembersBatchEdge,
    MobWireMembersBatchParams, MobWireMembersBatchResult, MobWireParams, MobWireResult,
    MobWiringRulesInput, SupervisorRotationIncompleteDataWire,
    SupervisorRotationIncompleteDetailsWire, SupervisorRotationIncompleteKind,
    SupervisorRotationReportWire, SupervisorRotationRetryAuthority, SupervisorRotationRetryScope,
    WireAgentRuntimeId, WireAppendSystemContextStatus, WireCallbackToolSetDeclaration,
    WireControlScope, WireDesiredExecution, WireDesiredIdentityEdge, WireDesiredLocalCallbackTool,
    WireDesiredSessionAuthorityPolicy, WireDesiredSessionTarget, WireForkContext, WireGrantRecord,
    WireHandlingMode, WireHistoryRow, WireHostBindPhase, WireHostCapabilityFlags, WireHostRef,
    WireIdentityAdoptionOutcome, WireIdentityAdoptionPrecondition,
    WireIdentityConvergenceCondition, WireIdentityConvergenceMode,
    WireIdentityConvergenceResolutionOutcome, WireIdentityConvergenceStatus,
    WireIdentityProfileMemberDeclaration, WireIdentityReconcileDecision, WireIdentityWiringCustody,
    WireMemberHealthClass, WireMemberHistoryPageBody, WireMemberLaunchMode,
    WireMemberLifecycleCapabilities, WireMemberProgressEvent, WireMemberProgressSnapshot,
    WireMemberRef, WireMemberRefError, WireMemberRunState, WireMemberToolAccessConstraint,
    WireMemberToolAccessDeclaration, WireMemberToolCommitOutcome, WireMemberToolDeclaration,
    WireMobBackendKind, WireMobError, WireMobLifecycleAction, WireMobLifecycleStatus,
    WireMobMemberStatus, WireMobProfile, WireMobReconcileStage, WireMobRespawnOutcome,
    WireMobResumeOverrideField, WireMobRun, WireMobRunAccounting, WireMobRunMemberAccounting,
    WireMobRunResultEnvelope, WireMobRunStatus, WireMobRunUsageAttribution, WireMobRuntimeMode,
    WireMobToolConfig, WireMobWireAction, WirePeerConnectivity, WirePeerConnectivitySnapshot,
    WireProjectionProvenance, WireReachability, WireRenderClass, WireRenderMetadata,
    WireRenderSalience, WireRouteInstallObligation, WireRuntimeBinding, WireScopeDeniedDetail,
    WireToolAccessPolicy, WireToolFilter, WireTrustedPeerIdentity, WireTrustedPeerSpec,
    WireUnreachablePeer, WireWorkExecutionLifecyclePhase, WireWorkGraphFlowExecutionBinding,
    WireWorkGraphFlowWorkRef, WireWorkOrigin,
};
pub use models::{
    CatalogModelEntry, ModelsCatalogResponse, ProviderCatalog, WireModelBetaHeader,
    WireModelProfile, WireModelReleaseStage, WireModelTier, WireResolvedModelCapabilities,
};
pub use params::{CommsParams, CoreCreateParams, HookParams, SkillsParams, StructuredOutputParams};
pub use portable_spec::{
    PortableDefinitionExtract, PortableMcpDecl, PortableMemberSpec, PortableProfile,
    PortableSkillSource, PortableSpawnOverlay, PortableSystemPrompt, PortableToolConfig,
    WireMobToolAuthorityContext, WireMobToolCallerProvenance, WireNonPortableResourceKind,
    WireResolvedToolAccessPolicy, WireSecretBearingFieldKind, WireSpawnContinuityIntent,
    WireToolAccessConstraint,
};
pub use realtime::{
    RealtimeAudioChunk, RealtimeAudioFormat, RealtimeCapabilities, RealtimeImageChunk,
    RealtimeInputChunk, RealtimeInputKind, RealtimeOutputKind, RealtimeTextChunk,
    RealtimeTurningMode, RealtimeVideoChunk,
};
pub use rest::{
    RestAdoptMemberIdentityDeclarationRequest, RestAppendSystemContextRequest,
    RestApplyMemberToolDeclarationRequest, RestAuthBindingTestRequest,
    RestAuthProfileCreateRequest, RestContinueSessionRequest, RestCreateSessionRequest,
    RestMobForkHelperRequest, RestMobHelperRequest, RestMobWaitRequest,
    RestMobWireMembersBatchRequest, RestPatchConfigRequest, RestPeerResponseTerminalRequest,
    RestResolveIdentityConvergenceBlockRequest, RestSessionDetailsResponse, RestSetConfigRequest,
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
    SessionExternalEventParams,
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
    SessionStreamOpenParams, SessionStreamOpenResult, TranscriptRewriteMessage,
    UpdateSystemPromptParams, WireAssistantBlock, WireContentBlock, WireContentInput,
    WireInterruptOutcome, WirePromptInput, WireProviderMeta, WireSessionHistory, WireSessionInfo,
    WireSessionMessage, WireSessionSummary, WireSessionTranscriptRevision,
    WireSessionTranscriptRevisionEntry, WireSessionTranscriptRevisionList, WireStopReason,
    WireSystemMessageIdentity, WireToolResult, WireToolResultContent, WireTranscriptSource,
};
pub use skills::{SkillEntry, SkillInspectResponse, SkillListResponse, SkillSourceProvenance};
pub use spec_digest::{SpecDigestError, portable_member_spec_digest};
pub use stream_read::StreamReadStatus;
pub use supervisor_bridge::{
    BridgeAck, BridgeBindPayload, BridgeBindResponse, BridgeCapabilities, BridgeCommand,
    BridgeCommandDecodeError, BridgeDeliveryOutcome, BridgeDeliveryPayload,
    BridgeDeliveryRejectionCause, BridgeDeliveryResponse, BridgeDestroyResponse,
    BridgeDirectMemberFence, BridgeDirectMemberFenceEvidence, BridgeDirectMemberIncarnation,
    BridgeDirectRuntimeSessionToken, BridgeEventCursor, BridgeHardCancelPayload,
    BridgeHostBindPayload, BridgeHostBindResponse, BridgeHostBootstrapProof,
    BridgeHostMemberRecord, BridgeHostRebindPayload, BridgeHostReboundResponse,
    BridgeHostRuntimeIncarnation, BridgeHostStatusPayload, BridgeHostStatusResponse,
    BridgeInterruptPayload, BridgeLiveChannelPayload, BridgeLiveControlOutcome,
    BridgeLiveControlPayload, BridgeLiveControlVerb, BridgeLiveControlledResponse,
    BridgeLiveOpenPayload, BridgeLiveOpenedResponse, BridgeLiveStatusPayload,
    BridgeMaterializePayload, BridgeMaterializedResponse, BridgeMemberEventsPage,
    BridgeMemberHistoryPage, BridgeMemberIncarnation, BridgeMemberOperatorPayload,
    BridgeMemberReleasedResponse, BridgeMemberRuntimeState, BridgeMobPeerOverlayHandoff,
    BridgeObservationResponse, BridgeOutcomeTracking, BridgePeerConnectivity, BridgePeerSpec,
    BridgePeerTrustPayload, BridgePeerWiringPayload, BridgePollEventsPayload,
    BridgeProtocolVersion, BridgeReadHistoryPayload, BridgeReleasePayload, BridgeReply,
    BridgeRetireOutcome, BridgeRetirePayload, BridgeRetireResponse, BridgeSupervisorDelivery,
    BridgeSupervisorPayload, BridgeSupervisorRotationObservation, BridgeSupervisorRotationObserve,
    BridgeSupervisorRotationOperationReceipt, BridgeSupervisorRotationPendingPhase,
    BridgeSupervisorRotationRejectionCause, BridgeSupervisorRotationRejectionReceipt,
    BridgeSupervisorRotationState, BridgeSupervisorRotationSubmit,
    BridgeSupervisorRotationTargetReceipt, BridgeTurnCorrelation, BridgeTurnDirective,
    BridgeTurnOutcomeRecord, ConnectionTargetErrorKind, MaterializeLaunchMode,
    MaterializeLaunchOutcome, MemberBuildRejection, MemberEventCursor, MemberOperatorOp,
    MemberOperatorOutcome, MemberOperatorReply, MemberOperatorSpawnSpec, MemberSessionDisposal,
    RuntimeReleaseCause, SUPERVISOR_BRIDGE_CURRENT_PROTOCOL_VERSION,
    SUPERVISOR_BRIDGE_DEFAULT_PROTOCOL_VERSION, SUPERVISOR_BRIDGE_INTENT,
    SUPERVISOR_BRIDGE_PROTOCOL_VERSION, SUPERVISOR_BRIDGE_SUPPORTED_PROTOCOL_VERSIONS,
    SupervisorRotationOperationId, UnsupportedBridgeProtocolVersion, WireEventRow,
    WireFlowTurnOutcome, WireHostBindingDescriptor, WireHostBindingDescriptorKind, WireOpaqueJson,
    decode_bridge_command, supervisor_bridge_current_protocol_version,
    supervisor_bridge_default_protocol_version, supervisor_bridge_protocol_version_supported,
    supervisor_bridge_supported_protocol_versions,
};
pub use usage::{WireTurnUsage, WireUsage};
pub use workgraph::{WorkEventsResult, WorkItemsResult};
