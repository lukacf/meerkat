//! Typed-ID newtypes shared by catalog DSL machines.
//!
//! These are stub-grade identifiers used by the catalog DSL (TLC-facing twin
//! of the runtime machines). They intentionally mirror the shape of the "real"
//! typed IDs elsewhere in the workspace (e.g. `meerkat_core::SessionId`,
//! `meerkat_mob::ids::AgentRuntimeId`) without taking a dependency on those
//! crates — the catalog DSL is a leaf crate.

/// Unique identifier for a configured MCP server.
///
/// Keyed on the server's configured name (matches `.rkat/mcp.toml` section
/// headers). The DSL uses this as the key of the `mcp_server_states` map on
/// `MeerkatMachine` state so the `[MCP_PENDING]` system-notice toggle is a
/// pure read off DSL-owned state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct McpServerId(pub String);

impl<T: Into<String>> From<T> for McpServerId {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

/// Opaque correlation identifier for the peer request / response lifecycle.
///
/// Catalog-DSL twin of [`meerkat_core::PeerCorrelationId`]. Carried on all
/// W1-A peer-interaction inputs and effects; used as the key of the DSL's
/// `pending_peer_requests` and `inbound_peer_requests` substate maps so the
/// subscriber / stream registries can project deterministically off DSL state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PeerCorrelationId(pub String);

impl<T: Into<String>> From<T> for PeerCorrelationId {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}
