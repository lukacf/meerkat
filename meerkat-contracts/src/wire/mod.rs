//! Canonical wire response types.

mod event;
mod mcp_live;
mod mob;
mod models;
mod params;
mod result;
mod runtime;
mod schedule;
mod session;
pub mod skills;
mod usage;

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
    MobRepeatUntilInput, MobRoleWiringRuleInput, MobSkillSourceInput, MobSpawnPolicyInput,
    MobStepOutputFormatInput, MobSupervisorSpecInput, MobToolConfigInput, MobTopologyRuleInput,
    MobTopologySpecInput, MobUnwireParams, MobUnwireResult, MobWireParams, MobWireResult,
    MobWiringRulesInput, WireHandlingMode, WireMobBackendKind, WireMobRuntimeMode, WireRenderClass,
    WireRenderMetadata, WireRenderSalience, WireTrustedPeerSpec,
};
pub use models::{
    CatalogModelEntry, ModelsCatalogResponse, ProviderCatalog, WireModelProfile, WireModelTier,
};
pub use params::{CommsParams, CoreCreateParams, HookParams, SkillsParams, StructuredOutputParams};
pub use result::WireRunResult;
pub use runtime::{
    InputListParams, InputListResult, InputStateParams, InputStateResult, RuntimeAcceptOutcomeType,
    RuntimeAcceptParams, RuntimeAcceptResult, RuntimeResetParams, RuntimeResetResult,
    RuntimeRetireParams, RuntimeRetireResult, RuntimeStateParams, RuntimeStateResult,
    WireInputLifecycleState, WireInputState, WireInputStateHistoryEntry, WireRuntimeState,
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
pub use usage::WireUsage;
