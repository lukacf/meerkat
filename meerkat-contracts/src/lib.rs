//! Meerkat contracts — canonical wire types, capability model, and error contracts.
//!
//! This crate is the single source of truth for all wire-facing types.
//! Surface crates (RPC, REST, MCP, CLI) consume these types directly.

pub mod capability;
pub mod error;
pub mod protocol;
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
pub use protocol::Protocol;
pub use session_locator::{SessionLocator, SessionLocatorError, format_session_ref};
pub use version::ContractVersion;
pub use wire::{
    CatalogModelEntry, CommsParams, CoreCreateParams, HookParams, McpAddParams, McpLiveOpResponse,
    McpLiveOpStatus, McpLiveOperation, McpReloadParams, McpRemoveParams, ModelsCatalogResponse,
    ProviderCatalog, SkillEntry, SkillInspectResponse, SkillListResponse, SkillsParams,
    StructuredOutputParams, WireAssistantBlock, WireContentBlock, WireContentInput, WireEvent,
    WireModelProfile, WireModelTier, WireProviderMeta, WireRunResult, WireSessionHistory,
    WireSessionInfo, WireSessionMessage, WireSessionSummary, WireStopReason, WireToolCall,
    WireToolResult, WireToolResultContent, WireUsage,
};
