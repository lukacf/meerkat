//! Canonical wire response types.

mod approval;
mod artifact;
mod auth;
mod comms;
mod connection;
mod event;
mod host;
mod image_generation;
mod mcp_live;
mod mob;
mod models;
mod params;
mod realtime;
mod result;
pub mod runtime;
mod schedule;
mod session;
pub mod skills;
pub mod supervisor_bridge;
mod usage;

pub use approval::{
    ApprovalActionKind, ApprovalDecideParams, ApprovalDecision, ApprovalDecisionRecord,
    ApprovalGetParams, ApprovalId, ApprovalListFilter, ApprovalListParams, ApprovalListResult,
    ApprovalOwnerRef, ApprovalPrincipalId, ApprovalProposedAction, ApprovalRecord, ApprovalRequest,
    ApprovalRequestParams, ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk, ApprovalStatus,
};
pub use comms::{
    CommsCommandError, CommsCommandRequest, CommsPeerEntry, CommsPeersParams, CommsPeersResult,
    CommsSendParams, CommsSendResult, HandlingMode as WireCommsHandlingMode,
    InputSource as WireCommsInputSource, InputStreamMode as WireCommsInputStreamMode, PeerAddress,
    PeerCapabilitySet, PeerDirectoryEntry, PeerDirectoryListing, PeerDirectorySource, PeerId,
    PeerName as WireCommsPeerName, PeerReachability, PeerReachabilityReason, PeerSendability,
    PeerTransport, ResponseStatus as WireCommsResponseStatus,
};
pub use connection::{
    BindingIdParams, CreateProfileParams, DeviceCompleteParams, DeviceStartParams,
    LoginCompleteParams, LoginStartParams, ProvisionApiKeyParams, RealmIdParams, WireAuthError,
    WireAuthProfile, WireAuthProfileCleared, WireAuthProfileCreated, WireAuthProfileDetail,
    WireAuthProfilesList, WireAuthStatus, WireAuthStatusDetail, WireBackendProfile,
    WireBindingIdentity, WireConnectionRef, WireDeviceCompleteResult, WireDeviceStart,
    WireLoginReady, WireLoginStart, WireProviderBinding, WireProvisionApiKeyResult,
    WireRealmConnectionSet, WireRealmList, WireRealmSummary,
};

pub use artifact::{
    ArtifactDownloadParams, ArtifactDownloadResult, ArtifactIdParams, ArtifactListParams,
    ArtifactListResult,
};
pub use auth::{
    ActingOnBehalfOf, AuthGrant, GrantAction, GrantScope, PrincipalId, PrincipalKind, PrincipalRef,
    VisibilityClass,
};
pub use event::{
    EventReplayCursor, EventReplayCursorError, EventReplayEnvelope, EventReplayEventId,
    EventReplayScope, EventsLatestCursorParams, EventsLatestCursorResult, EventsListSinceParams,
    EventsListSinceResult, EventsSnapshotBody, EventsSnapshotParams, EventsSnapshotResult,
    WireEvent,
};
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
    MobListMembersMatchingResult, MobListResult, MobMcpServerConfigInput, MobMemberFilterWire,
    MobMemberListEntryWire, MobMemberParams, MobMemberSendParams, MobMemberSendResult,
    MobMemberSpecWire, MobMemberStatusResult, MobMembersResult, MobOrchestratorInput,
    MobPeerTarget, MobPolicyModeInput, MobProfileBindingInput, MobProfileCreateParams,
    MobProfileDeleteParams, MobProfileDeleteResult, MobProfileInput, MobProfileListResult,
    MobProfileLookupResult, MobProfileNameParams, MobProfileUpdateParams, MobReconcileFailureWire,
    MobReconcileOptionsWire, MobReconcileParams, MobReconcileReportWire, MobReconcileResult,
    MobRepeatUntilInput, MobRespawnParams, MobRespawnReceipt, MobRespawnResult, MobRetireResult,
    MobRoleWiringRuleInput, MobRotateSupervisorResult, MobSkillSourceInput, MobSnapshotResult,
    MobSpawnHelperParams, MobSpawnManyFailedResult, MobSpawnManyFailureCause, MobSpawnManyParams,
    MobSpawnManyResult, MobSpawnManyResultEntry, MobSpawnManyResultPayload,
    MobSpawnManyResultStatus, MobSpawnManySpawnedResult, MobSpawnParams, MobSpawnPolicyInput,
    MobSpawnReceiptWire, MobSpawnResult, MobSpawnSpecParams, MobStatusResult,
    MobStepOutputFormatInput, MobStreamCloseParams, MobStreamCloseResult, MobStreamOpenParams,
    MobStreamOpenResult, MobSubmitWorkParams, MobSubmitWorkResult, MobSupervisorSpecInput,
    MobToolConfigInput, MobTopologyRuleInput, MobTopologySpecInput, MobTurnStartParams,
    MobUnwireParams, MobUnwireResult, MobWaitMembersResult, MobWaitParams, MobWireParams,
    MobWireResult, MobWiringRulesInput, WireAgentRuntimeId, WireBudgetSplitPolicy, WireForkContext,
    WireHandlingMode, WireMemberLaunchMode, WireMemberRef, WireMemberRefError, WireMemberState,
    WireMobBackendKind, WireMobLifecycleAction, WireMobMemberStatus, WireMobProfile,
    WireMobReconcileStage, WireMobRuntimeMode, WireMobToolConfig, WireRenderClass,
    WireRenderMetadata, WireRenderSalience, WireRuntimeBinding, WireToolAccessPolicy,
    WireToolFilter, WireTrustedPeerIdentity, WireTrustedPeerSpec, WireWorkOrigin,
};
pub use models::{
    CatalogModelEntry, ModelsCatalogResponse, ProviderCatalog, WireModelBetaHeader,
    WireModelProfile, WireModelTier,
};
pub use params::{CommsParams, CoreCreateParams, HookParams, SkillsParams, StructuredOutputParams};
pub use realtime::{
    AudioFormatMismatchContext, RealtimeActionResult, RealtimeAudioChunk, RealtimeAudioFormat,
    RealtimeBargeInTruncateFrame, RealtimeCapabilities, RealtimeCapabilitiesParams,
    RealtimeCapabilitiesResult, RealtimeChannelClosedFrame, RealtimeChannelConfig,
    RealtimeChannelErrorFrame, RealtimeChannelEventFrame, RealtimeChannelInputFrame,
    RealtimeChannelOpenFrame, RealtimeChannelOpenedFrame, RealtimeChannelRole,
    RealtimeChannelState, RealtimeChannelStatus, RealtimeChannelStatusFrame, RealtimeChannelTarget,
    RealtimeClientFrame, RealtimeErrorCode, RealtimeErrorDetails, RealtimeEvent,
    RealtimeInputChunk, RealtimeInputKind, RealtimeOpenInfo, RealtimeOpenRequest,
    RealtimeOutputChunk, RealtimeOutputKind, RealtimeProtocolVersion, RealtimeReconnectPolicy,
    RealtimeServerFrame, RealtimeStatusParams, RealtimeStatusResult, RealtimeTextChunk,
    RealtimeTextDelta, RealtimeTurningMode, RealtimeVideoChunk, ToolCallTimeoutContext,
};
pub use result::WireRunResult;
pub use runtime::{
    PeerResponseTerminalStatusWire,
    RuntimeAcceptOutcomeType,
    RuntimeAcceptResult,
    RuntimeRealtimeAttachmentStatusParams,
    RuntimeRealtimeAttachmentStatusResult,
    RuntimeStateResult,
    SessionExternalEventEnvelope,
    SessionPeerResponseTerminalParams,
    // Re-export of the `StructuredProviderExtension` core relocation
    // from C-1 — external callers can still import via the wire path.
    StructuredProviderExtension,
    WireInputLifecycleState,
    WireInputState,
    WireInputStateHistoryEntry,
    WireRealtimeAttachmentStatus,
    WireRuntimeState,
};
pub use schedule::{
    ListSchedulesParams, ScheduleIdParams, ScheduleListResult, ScheduleOccurrencesParams,
    ScheduleOccurrencesResult, UpdateScheduleParams,
};
pub use session::{
    SessionStreamCloseParams, SessionStreamCloseResult, SessionStreamOpenParams,
    SessionStreamOpenResult, WireAssistantBlock, WireContentBlock, WireContentInput,
    WireProviderMeta, WireSessionHistory, WireSessionInfo, WireSessionMessage, WireSessionSummary,
    WireStopReason, WireToolCall, WireToolResult, WireToolResultContent,
};
pub use skills::{SkillEntry, SkillInspectResponse, SkillListResponse, SkillSourceProvenance};
pub use supervisor_bridge::{
    BridgeAck, BridgeBindPayload, BridgeBindResponse, BridgeCapabilities, BridgeCommand,
    BridgeCommandDecodeError, BridgeDeliveryOutcome, BridgeDeliveryPayload,
    BridgeDeliveryRejectionCause, BridgeDeliveryResponse, BridgeDestroyResponse,
    BridgeHardCancelPayload, BridgeMemberRuntimeState, BridgeObservationResponse,
    BridgePeerConnectivity, BridgePeerSpec, BridgePeerWiringPayload, BridgeProtocolVersion,
    BridgeReply, BridgeRetireResponse, BridgeSupervisorPayload,
    SUPERVISOR_BRIDGE_CURRENT_PROTOCOL_VERSION, SUPERVISOR_BRIDGE_DEFAULT_PROTOCOL_VERSION,
    SUPERVISOR_BRIDGE_INTENT, SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
    SUPERVISOR_BRIDGE_SUPPORTED_PROTOCOL_VERSIONS, UnsupportedBridgeProtocolVersion,
    decode_bridge_command, supervisor_bridge_current_protocol_version,
    supervisor_bridge_default_protocol_version, supervisor_bridge_protocol_version_supported,
    supervisor_bridge_supported_protocol_versions,
};
pub use usage::WireUsage;
