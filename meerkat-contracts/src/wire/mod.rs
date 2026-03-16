//! Canonical wire response types.

mod event;
mod mcp_live;
mod models;
mod params;
mod result;
mod session;
pub mod skills;
mod usage;

pub use event::WireEvent;
pub use mcp_live::{
    McpAddParams, McpLiveOpResponse, McpLiveOpStatus, McpLiveOperation, McpReloadParams,
    McpRemoveParams,
};
pub use models::{
    CatalogModelEntry, ModelsCatalogResponse, ProviderCatalog, WireModelProfile, WireModelTier,
};
pub use params::{CommsParams, CoreCreateParams, HookParams, SkillsParams, StructuredOutputParams};
pub use result::WireRunResult;
pub use session::{
    WireAssistantBlock, WireContentBlock, WireContentInput, WireProviderMeta, WireSessionHistory,
    WireSessionInfo, WireSessionMessage, WireSessionSummary, WireStopReason, WireToolCall,
    WireToolResult, WireToolResultContent,
};
pub use skills::{SkillEntry, SkillInspectResponse, SkillListResponse};
pub use usage::WireUsage;
