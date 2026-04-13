//! Diagnostic snapshot facade for the Meerkat runtime surface.
//!
//! The real runtime authority now lives in `meerkat-runtime`; this module keeps
//! only the joined snapshot types that remain useful for diagnostics and future
//! follow-up work.

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
