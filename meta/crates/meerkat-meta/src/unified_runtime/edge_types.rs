//! Edge discovery types and traits for distributed runtime topology.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

/// Error constructing a [`DesiredPeerEdge`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesiredPeerEdgeError {
    EmptyEndpoint,
    SelfEdge,
}

impl Display for DesiredPeerEdgeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyEndpoint => write!(f, "edge endpoint must not be empty"),
            Self::SelfEdge => write!(f, "self-edges are not allowed"),
        }
    }
}

impl std::error::Error for DesiredPeerEdgeError {}

/// A canonical undirected peer edge. Endpoints are sorted at construction
/// time and self-edges are rejected, so the invariant `a < b` always holds.
///
/// Fields are private - use [`DesiredPeerEdge::new`] or [`DesiredPeerEdge::endpoints`] to access.
/// Deserialization validates the invariant, rejecting non-canonical inputs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct DesiredPeerEdge {
    a: String,
    b: String,
}

impl<'de> Deserialize<'de> for DesiredPeerEdge {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            a: String,
            b: String,
        }
        let raw = Raw::deserialize(deserializer)?;
        DesiredPeerEdge::new(raw.a, raw.b).map_err(serde::de::Error::custom)
    }
}

impl DesiredPeerEdge {
    pub fn new(a: impl Into<String>, b: impl Into<String>) -> Result<Self, DesiredPeerEdgeError> {
        let mut a = a.into().trim().to_string();
        let mut b = b.into().trim().to_string();
        if a.is_empty() || b.is_empty() {
            return Err(DesiredPeerEdgeError::EmptyEndpoint);
        }
        if a == b {
            return Err(DesiredPeerEdgeError::SelfEdge);
        }
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        Ok(Self { a, b })
    }

    pub fn endpoints(&self) -> (&str, &str) {
        (&self.a, &self.b)
    }
}

/// Narrow view of a mob member passed to [`EdgeDiscovery`] implementations.
///
/// Projected from meerkat's `MobMemberListEntry` (or from a roster spec in
/// the legacy topology adapter). Carries only the fields edge-discovery
/// policies actually use: identity, role, current wiring, labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeMemberView {
    pub agent_identity: String,
    pub role: String,
    pub wired_to: BTreeSet<String>,
    pub labels: BTreeMap<String, String>,
}

/// Trait for computing desired peer edges from active mob members.
///
/// The app owns the policy (which agents should be wired). Meta owns
/// the lifecycle-safe reconciliation that makes reality match the policy.
pub trait EdgeDiscovery: Send + Sync {
    fn discover_edges(
        &self,
        active_members: Vec<EdgeMemberView>,
    ) -> Pin<Box<dyn Future<Output = Vec<DesiredPeerEdge>> + Send + '_>>;
}

/// A failed edge operation during reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeReconcileFailure {
    pub edge: DesiredPeerEdge,
    pub operation: String,
    pub error: String,
}

/// Opaque context produced by [`PreSpawnHook`] and consumed by [`Discovery::discover`].
///
/// Carries data from the pre-spawn phase (e.g. session resume maps, warmed caches)
/// into the discovery phase without requiring shared side-channel state.
pub type PreSpawnContext = serde_json::Value;

/// Trait for discovering agents to spawn into a mob at bootstrap time.
///
/// `discover` receives the [`PreSpawnContext`] produced by the pre-spawn hook
/// (or `Value::Null` if no hook ran). This enables the "query sessions once,
/// build a resume map, feed that into discovery" pattern without side-channel state.
pub trait Discovery: Send + Sync {
    fn discover(
        &self,
        context: PreSpawnContext,
    ) -> Pin<Box<dyn Future<Output = Vec<crate::types::AgentDiscoverySpec>> + Send + '_>>;
}

/// A callback that runs before discovery/spawn for session preloading, cache warming, etc.
///
/// Returns a [`PreSpawnContext`] on success, which is passed to [`Discovery::discover`].
/// This enables pre-spawn to produce data (resume maps, session queries, etc.) that
/// discovery consumes, replacing the need for shared mutable side-channel state.
pub type PreSpawnHook = Box<
    dyn FnOnce() -> Pin<
            Box<
                dyn Future<Output = Result<PreSpawnContext, Box<dyn std::error::Error + Send>>>
                    + Send,
            >,
        > + Send,
>;
