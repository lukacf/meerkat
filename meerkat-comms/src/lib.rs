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

/// Validate whether host mode can be enabled.
///
/// Returns `Ok(true)` if `requested` is true, `Ok(false)` if not.
/// This function existing in `meerkat-comms` means comms is compiled.
/// Surfaces that don't have comms feature-gated simply cannot call this.
///
/// This is the canonical validation point — all surfaces delegate here.
pub fn validate_host_mode(requested: bool) -> Result<bool, String> {
    Ok(requested)
}
