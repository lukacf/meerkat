#![cfg_attr(test, allow(clippy::panic))]
// meerkat-comms
//! Inter-agent communication for Meerkat instances.

pub mod identity;
pub mod inbox;
pub mod inproc;
pub mod io_task;
pub mod router;
pub mod transport;
pub mod trust;
pub mod types;

pub mod agent;
pub mod mcp;
pub mod runtime;

pub use identity::{IdentityError, Keypair, PubKey, Signature};
pub use inbox::{Inbox, InboxError, InboxSender};
pub use inproc::{InprocRegistry, InprocSendError};
pub use io_task::{IoTaskError, handle_connection};
pub use router::{CommsConfig, DEFAULT_MAX_MESSAGE_BYTES, Router, SendError};
pub use transport::codec::{EnvelopeFrame, TransportCodec};
pub use transport::{PeerAddr, TransportError};
pub use trust::{TrustError, TrustedPeer, TrustedPeers};
pub use types::{Envelope, InboxItem, MessageKind, Status};

// Re-export high-level components
pub use runtime::comms_bootstrap::{
    CommsAdvertise, CommsBootstrap, CommsBootstrapError, CommsBootstrapMode, ParentCommsContext,
    PreparedComms,
};
pub use runtime::comms_config::{CoreCommsConfig, ResolvedCommsConfig};
pub use runtime::comms_runtime::{CommsRuntime, CommsRuntimeError};

pub use agent::CommsAgent;
pub use agent::dispatcher::{
    CommsToolDispatcher, DynCommsToolDispatcher, NoOpDispatcher, wrap_with_comms,
};
pub use mcp::tools::{ToolContext, handle_tools_call, tools_list};

// Capability registration
inventory::submit! {
    meerkat_contracts::CapabilityRegistration {
        id: meerkat_contracts::CapabilityId::Comms,
        description: "Inter-agent communication: send, request, response, list peers + host mode",
        scope: meerkat_contracts::CapabilityScope::Universal,
        requires_feature: Some("comms"),
        prerequisites: &[],
        status_resolver: None,
    }
}

// Skill registration
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "multi-agent-comms",
        name: "Multi-Agent Comms",
        description: "Setting up host mode, peer trust, send vs request/response patterns",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["comms"],
        body: include_str!("../skills/multi-agent-comms/SKILL.md"),
        extensions: &[],
    }
}

/// Confirm host mode availability when the comms crate is compiled in.
///
/// This function is intentionally a passthrough — its existence in the
/// dependency graph *is* the validation. When `meerkat-comms` is linked,
/// comms is available and host mode can be enabled. The feature-gate check
/// lives in `meerkat::surface::resolve_host_mode()`, which calls this
/// function under `#[cfg(feature = "comms")]` and returns an error under
/// `#[cfg(not(feature = "comms"))]`.
///
/// This two-layer design avoids duplicating the `cfg` check in every
/// surface crate.
pub fn validate_host_mode(requested: bool) -> Result<bool, String> {
    Ok(requested)
}
