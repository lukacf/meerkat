//! Canonical wire response types.

mod event;
mod mcp_live;
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
pub use params::{CommsParams, CoreCreateParams, HookParams, SkillsParams, StructuredOutputParams};
pub use result::WireRunResult;
pub use session::{WireSessionInfo, WireSessionSummary};
pub use skills::{SkillEntry, SkillInspectResponse, SkillListResponse};
pub use usage::WireUsage;
