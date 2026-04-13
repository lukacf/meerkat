//! Diagnostic snapshot facade for the current Meerkat runtime surface.
//!
//! The cutover removes the old shadow/taxonomy/validator regime from this
//! facade. The real runtime authority lives in `meerkat-runtime`; this module
//! only preserves the joined snapshot types that are still useful for
//! diagnostics and future follow-up work.

use meerkat_core::{
    AgentExecutionSnapshot, ExternalToolSurfaceSnapshot, PeerIngressRuntimeSnapshot,
    ToolScopeSnapshot,
};
use meerkat_runtime::MeerkatMachineSpineSnapshot;

/// Joined diagnostic view over the current Meerkat runtime spine plus optional
/// turn, tool, and peer projections.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct MeerkatMachineSnapshot {
    pub spine: MeerkatMachineSpineSnapshot,
    pub turn: Option<AgentExecutionSnapshot>,
    pub tools: Option<ToolScopeSnapshot>,
    pub tool_surface: Option<ExternalToolSurfaceSnapshot>,
    pub peer: Option<PeerIngressRuntimeSnapshot>,
}
