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
    CommsCommandError, CommsCommandRequest, HandlingMode as WireCommsHandlingMode,
    InputSource as WireCommsInputSource, InputStreamMode as WireCommsInputStreamMode,
    PeerName as WireCommsPeerName, ResponseStatus as WireCommsResponseStatus,
};
pub use connection::{
    WireAuthError, WireAuthProfile, WireAuthProfileCleared, WireAuthProfileCreated,
    WireAuthProfileDetail, WireAuthProfilesList, WireAuthStatus, WireAuthStatusDetail,
    WireBackendProfile, WireBindingIdentity, WireConnectionRef, WireDeviceStart, WireLoginReady,
    WireLoginStart, WireProviderBinding, WireRealmConnectionSet, WireRealmList, WireRealmSummary,
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
    MobBackendConfigInput, MobCancelAllWorkParams, MobCancelWorkParams, MobCollectionPolicyInput,
    MobConditionExprInput, MobCreateParams, MobCreateResult, MobDefinitionInput,
    MobDependencyModeInput, MobDispatchModeInput, MobEnsureMemberOutcomeWire,
    MobEnsureMemberParams, MobEnsureMemberResult, MobEventRouterConfigInput,
    MobExternalBackendConfigInput, MobFlowNodeInput, MobFlowSpecInput, MobFlowStepInput,
    MobFrameSpecInput, MobFrameStepInput, MobIngressInteractionParams, MobIngressInteractionResult,
    MobLifecycleParams, MobLimitsSpecInput, MobListMembersMatchingParams,
    MobListMembersMatchingResult, MobMcpServerConfigInput, MobMemberFilterWire,
    MobMemberListEntryWire, MobMemberSendParams, MobMemberSendResult, MobMemberSpecWire,
    MobOrchestratorInput, MobPeerTarget, MobPolicyModeInput, MobProfileBindingInput,
    MobProfileInput, MobReconcileFailureWire, MobReconcileOptionsWire, MobReconcileParams,
    MobReconcileReportWire, MobReconcileResult, MobRepeatUntilInput, MobRoleWiringRuleInput,
    MobSkillSourceInput, MobSpawnPolicyInput, MobSpawnReceiptWire, MobStepOutputFormatInput,
    MobSubmitWorkParams, MobSubmitWorkResult, MobSupervisorSpecInput, MobToolConfigInput,
    MobTopologyRuleInput, MobTopologySpecInput, MobUnwireParams, MobUnwireResult, MobWireParams,
    MobWireResult, MobWiringRulesInput, WireAgentRuntimeId, WireHandlingMode, WireMemberRef,
    WireMemberRefError, WireMemberState, WireMobBackendKind, WireMobLifecycleAction,
    WireMobMemberStatus, WireMobRuntimeMode, WireRenderClass, WireRenderMetadata,
    WireRenderSalience, WireRuntimeBinding, WireTrustedPeerSpec, WireWorkOrigin,
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
    InputListParams,
    InputListResult,
    InputStateParams,
    InputStateResult,
    PeerResponseTerminalStatusWire,
    RuntimeAcceptOutcomeType,
    RuntimeAcceptParams,
    RuntimeAcceptResult,
    RuntimeRealtimeAttachmentStatusParams,
    RuntimeRealtimeAttachmentStatusResult,
    RuntimeResetParams,
    RuntimeResetResult,
    RuntimeRetireParams,
    RuntimeRetireResult,
    RuntimeStateParams,
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
    WireAssistantBlock, WireContentBlock, WireContentInput, WireProviderMeta, WireSessionHistory,
    WireSessionInfo, WireSessionMessage, WireSessionSummary, WireStopReason, WireToolCall,
    WireToolResult, WireToolResultContent,
};
pub use skills::{SkillEntry, SkillInspectResponse, SkillListResponse, SkillSourceProvenance};
pub use supervisor_bridge::{
    BridgeAck, BridgeBindPayload, BridgeBindResponse, BridgeCapabilities, BridgeCommand,
    BridgeDeliveryOutcome, BridgeDeliveryPayload, BridgeDeliveryRejectionCause,
    BridgeDeliveryResponse, BridgeDestroyResponse, BridgeMemberRuntimeState,
    BridgeObservationResponse, BridgePeerConnectivity, BridgePeerSpec, BridgePeerWiringPayload,
    BridgeReply, BridgeRetireResponse, BridgeSupervisorPayload, SUPERVISOR_BRIDGE_INTENT,
};
pub use usage::WireUsage;
