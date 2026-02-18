//! Meerkat resolver trait and identity types.
//!
//! A [`MeerkatResolver`] provides dynamic discovery of meerkat instances
//! for roles with `per_meerkat` cardinality. The [`MeerkatIdentity`] type
//! carries the identity and labels for a single meerkat instance.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Identity of a single meerkat instance.
///
/// Returned by [`MeerkatResolver::list_meerkats`] and used during reconcile
/// to determine which sessions to spawn or retire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeerkatIdentity {
    /// Unique meerkat instance ID.
    pub meerkat_id: String,
    /// Role this meerkat belongs to.
    pub role: String,
    /// Discovery labels (e.g., region, team).
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Additional attributes (opaque to the mob runtime).
    #[serde(default = "default_attributes")]
    pub attributes: serde_json::Value,
}

fn default_attributes() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// Context provided to resolvers during list operations.
#[derive(Debug, Clone)]
pub struct ResolverContext {
    /// Mob ID being resolved for.
    pub mob_id: String,
    /// Role being resolved.
    pub role: String,
    /// Resolver name (from the spec).
    pub resolver_name: String,
}

/// Trait for resolving meerkat identities dynamically.
///
/// Implementations provide discovery for roles with `per_meerkat` cardinality.
/// The reconcile engine calls [`list_meerkats`](MeerkatResolver::list_meerkats)
/// to compare desired vs. actual state.
#[async_trait]
pub trait MeerkatResolver: Send + Sync {
    /// List all meerkat identities for the given context.
    async fn list_meerkats(
        &self,
        ctx: &ResolverContext,
    ) -> Result<Vec<MeerkatIdentity>, crate::error::MobError>;
}
