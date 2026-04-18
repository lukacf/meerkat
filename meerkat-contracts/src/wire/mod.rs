//! Canonical wire response types.

mod connection;
mod event;
mod mcp_live;
mod mob;
mod models;
mod params;
mod realtime;
mod result;
mod runtime;
mod schedule;
mod session;
pub mod skills;
pub mod supervisor_bridge;
mod usage;

pub use connection::{
    WireAuthError, WireAuthProfile, WireAuthStatus, WireBackendProfile, WireConnectionRef,
    WireProviderBinding, WireRealmConnectionSet,
};

pub use event::WireEvent;
pub use mcp_live::{
    McpAddParams, McpLiveOpResponse, McpLiveOpStatus, McpLiveOperation, McpReloadParams,
    McpRemoveParams,
};
pub use mob::{
    MobBackendConfigInput, MobCollectionPolicyInput, MobConditionExprInput, MobCreateParams,
    MobCreateResult, MobDefinitionInput, MobDependencyModeInput, MobDispatchModeInput,
    MobEventRouterConfigInput, MobExternalBackendConfigInput, MobFlowNodeInput, MobFlowSpecInput,
    MobFlowStepInput, MobFrameSpecInput, MobFrameStepInput, MobLimitsSpecInput,
    MobMcpServerConfigInput, MobMemberSendParams, MobMemberSendResult, MobOrchestratorInput,
    MobPeerTarget, MobPolicyModeInput, MobProfileBindingInput, MobProfileInput,
    MobRealtimeAttachParams, MobRealtimeAttachResult, MobRealtimeDetachParams,
    MobRealtimeDetachResult, MobRepeatUntilInput, MobRoleWiringRuleInput, MobSkillSourceInput,
    MobSpawnPolicyInput, MobStepOutputFormatInput, MobSupervisorSpecInput, MobToolConfigInput,
    MobTopologyRuleInput, MobTopologySpecInput, MobUnwireParams, MobUnwireResult, MobWireParams,
    MobWireResult, MobWiringRulesInput, WireAgentRuntimeId, WireHandlingMode, WireMobBackendKind,
    WireMobRuntimeMode, WireRenderClass, WireRenderMetadata, WireRenderSalience,
    WireRuntimeBinding, WireTrustedPeerSpec,
};
pub use models::{
    CatalogModelEntry, ModelsCatalogResponse, ProviderCatalog, WireModelProfile, WireModelTier,
};
pub use params::{CommsParams, CoreCreateParams, HookParams, SkillsParams, StructuredOutputParams};
pub use realtime::{
    AudioFormatMismatchContext, RealtimeAudioChunk, RealtimeAudioFormat,
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
    InputListParams, InputListResult, InputStateParams, InputStateResult, RuntimeAcceptOutcomeType,
    RuntimeAcceptParams, RuntimeAcceptResult, RuntimeRealtimeAttachmentStatusParams,
    RuntimeRealtimeAttachmentStatusResult, RuntimeResetParams, RuntimeResetResult,
    RuntimeRetireParams, RuntimeRetireResult, RuntimeStateParams, RuntimeStateResult,
    WireInputLifecycleState, WireInputState, WireInputStateHistoryEntry,
    WireRealtimeAttachmentStatus, WireRuntimeState,
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
pub use skills::{SkillEntry, SkillInspectResponse, SkillListResponse};
pub use supervisor_bridge::{
    BridgeAck, BridgeBindPayload, BridgeBindResponse, BridgeCapabilities, BridgeCommand,
    BridgeDeliveryOutcome, BridgeDeliveryPayload, BridgeDeliveryResponse, BridgeDestroyResponse,
    BridgeMemberRuntimeState, BridgeObservationResponse, BridgePeerConnectivity, BridgePeerSpec,
    BridgePeerWiringPayload, BridgeReply, BridgeRetireResponse, BridgeSupervisorPayload,
    SUPERVISOR_BRIDGE_INTENT,
};
pub use usage::WireUsage;
