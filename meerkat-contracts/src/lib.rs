//! Meerkat contracts — canonical wire types, capability model, and error contracts.
//!
//! This crate is the single source of truth for all wire-facing types.
//! Surface crates (RPC, REST, MCP, CLI) consume these types directly.

pub mod capability;
pub mod error;
pub mod event_catalog;
pub mod protocol;
pub mod rest_catalog;
pub mod rpc_catalog;
pub mod session_locator;
pub mod version;
pub mod wire;

#[cfg(feature = "schema")]
pub mod emit;

// Re-exports for convenience
pub use capability::{
    CapabilitiesResponse, CapabilityEntry, CapabilityId, CapabilityRegistration, CapabilityScope,
    CapabilityStatus, build_capabilities,
};
pub use error::{CapabilityHint, ErrorCategory, ErrorCode, WireError};
pub use event_catalog::KNOWN_AGENT_EVENT_TYPES;
pub use protocol::Protocol;
pub use rest_catalog::{
    RestOperationDescriptor, RestPathDescriptor, rest_documented_paths, rest_path_catalog,
};
pub use rpc_catalog::{
    RpcMethodCatalogOptions, RpcMethodDescriptor, RpcNotificationDescriptor, rpc_method_catalog,
    rpc_method_names, rpc_notification_catalog, rpc_notification_names,
};
pub use session_locator::{SessionLocator, SessionLocatorError, format_session_ref};
pub use version::ContractVersion;
pub use wire::{
    CatalogModelEntry, CommsParams, CoreCreateParams, HookParams, InputListParams, InputListResult,
    InputStateParams, InputStateResult, McpAddParams, McpLiveOpResponse, McpLiveOpStatus,
    McpLiveOperation, McpReloadParams, McpRemoveParams, MobPeerTarget, MobSendParams,
    MobSendResult, MobUnwireParams, MobUnwireResult, MobWireParams, MobWireResult,
    ModelsCatalogResponse, ProviderCatalog, RuntimeAcceptOutcomeType, RuntimeAcceptParams,
    RuntimeAcceptResult, RuntimeResetParams, RuntimeResetResult, RuntimeRetireParams,
    RuntimeRetireResult, RuntimeStateParams, RuntimeStateResult, SkillEntry, SkillInspectResponse,
    SkillListResponse, SkillsParams, StructuredOutputParams, WireAssistantBlock, WireContentBlock,
    WireContentInput, WireEvent, WireHandlingMode, WireInputLifecycleState, WireInputState,
    WireInputStateHistoryEntry, WireModelProfile, WireModelTier, WireProviderMeta, WireRenderClass,
    WireRenderMetadata, WireRenderSalience, WireRunResult, WireRuntimeState, WireSessionHistory,
    WireSessionInfo, WireSessionMessage, WireSessionSummary, WireStopReason, WireToolCall,
    WireToolResult, WireToolResultContent, WireTrustedPeerSpec, WireUsage,
};
