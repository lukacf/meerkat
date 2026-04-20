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

/// Stable identity of a comms runtime instance.
///
/// Catalog-DSL identity for an `Arc<dyn CommsRuntime>` pointer so the DSL can
/// distinguish distinct runtime instances and reject silent transport
/// downgrades (W2-G / issue #264). The runtime derives the string from the
/// `Arc`'s pointer address; the catalog DSL treats it as an opaque newtype.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CommsRuntimeId(pub String);

impl<T: Into<String>> From<T> for CommsRuntimeId {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

/// Mob instance identifier.
///
/// Catalog-DSL twin of [`meerkat_mob::ids::MobId`] carried on
/// `AttachMobIngress` to record which mob owns a peer-ingress transport
/// capability. The runtime mirrors the real typed ID; catalog DSL keeps it
/// stub-grade since this crate is a leaf.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MobId(pub String);

impl<T: Into<String>> From<T> for MobId {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}
