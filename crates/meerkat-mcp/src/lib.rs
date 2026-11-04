//! meerkat-mcp - MCP client for Meerkat
//!
//! Connect to MCP servers and route tool calls.

// Without `workspace-tests` (the published package, whose test command
// leaves it off) the unit tests that need the workspace compile out, and
// the test helpers only they use go unused. The workspace's test builds
// turn the feature on and keep both lints.
#![cfg_attr(
    all(test, not(feature = "workspace-tests")),
    allow(dead_code, unused_imports)
)]

mod adapter;
pub mod apps;
mod attempt;
mod attempt_guard;
mod checked_route;
mod client_service;
mod connect_attempt;
mod connection;
mod context_provider;
mod error;
pub mod external_tool_surface_authority;
pub mod generated;
mod process_confinement;
mod protocol;
mod router;
mod stdio_diagnostics;
mod transport;

pub use adapter::McpRouterAdapter;
pub use adapter::{McpNotReady, McpReloadFailure, McpReloadReport};
pub use adapter::{McpRouterShutdownObservation, McpRouterShutdownObserver};
pub use attempt_guard::{
    McpAttemptCancelCause, McpAttemptCancellation, McpAttemptDisposition, McpAttemptGuard,
    McpAttemptGuardBound, McpAttemptGuardBounds, McpAttemptGuardBoundsError,
    McpAttemptGuardFactory, McpAttemptGuardRequest, McpAttemptGuards, McpAttemptKey,
    McpAttemptNotInstalled, McpAttemptOpenOutcome, McpAttemptOperation, McpAttemptProcessEnd,
    McpAttemptRefusal, McpConstructionId, McpConstructionOwner, McpGuardCompletion,
    McpGuardCompletionRecord, McpGuardFinishError, McpGuardStage,
};
pub use checked_route::{CheckedMcpRoute, CheckedMcpRouteError, CheckedMcpRouteSpec};
pub use client_service::McpClientServiceFactory;
pub use connect_attempt::{
    McpAttemptSettlement, McpConnectAttempt, McpLocalTermination, McpRemoteSession,
};
pub use connection::{
    McpAuthResolver, McpCloseTicket, McpConnectFailure, McpConnection, McpCustodyNotHeld,
    McpServerCloseOutcome, McpStdioCloseBounds, McpStdioCloseBoundsError, McpStdioClosePhase,
    McpStdioCloseReceipt, McpStdioLeaderExitObservation, McpStdioLeaderOutcome,
    McpStdioSignalOutcome, McpStdioWitnessOutcome,
};
pub use context_provider::{
    McpCallContext, McpCallContextError, McpCallContextProvider, McpCallFinalCheck,
    McpCallFinalCustody, McpCallTarget, McpConnectionId, McpResourceTarget,
};
pub use error::{McpError, ToolDiscoveryLimit};
pub use meerkat_auth_core::http_egress::{
    HttpEgressAddresses, HttpEgressError, HttpEgressOrigin, HttpEgressOriginError,
    HttpEgressPolicy, HttpEgressProfile, HttpEgressRefusal, HttpEgressScheme,
    HttpEgressUnavailable,
};
pub use meerkat_core::{ExternalToolDelta, ExternalToolDeltaPhase};
pub use process_confinement::McpStdioLaunchProfile;
pub use protocol::McpProtocol;
pub use router::{
    AuthorizedRetry, HeldSurfaceLineages, McpApplyDelta, McpApplyResult, McpBoundaryRejection,
    McpLifecycleAction, McpLifecyclePhase, McpReloadTarget, McpRouter, McpRouterShutdownReport,
    McpServerCloseRecord, McpServerLifecycleState, McpSurfaceOwnership, McpToolNameCollision,
    McpToolRoute, ReleasedCredential, SurfaceHold, SurfaceOwnershipSlot,
};

mod configured_entry;
pub use configured_entry::ConfiguredMcpEntry;

// Capability registration
inventory::submit! {
    meerkat_capabilities::CapabilityRegistration {
        id: meerkat_capabilities::CapabilityId::McpLive,
        description: "Live MCP server mutation (add/remove/reload on running sessions)",
        scope: meerkat_capabilities::CapabilityScope::Universal,
        requires_feature: Some("mcp"),
        prerequisites: &[],
        status_resolver: None,
    }
}

// Skill registration
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "mcp-server-setup",
        name: "MCP Server Setup",
        description: "How to configure MCP servers in .rkat/mcp.toml",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &[],
        body: include_str!("../skills/mcp-server-setup/SKILL.md"),
        extensions: &[],
    }
}

#[cfg(all(test, feature = "workspace-tests"))]
mod client_service_tests;
#[cfg(test)]
mod connect_attempt_tests;
#[cfg(test)]
mod egress_tests;
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod stdio_test_fixture;
