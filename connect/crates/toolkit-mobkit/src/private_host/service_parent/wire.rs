//! Values on the existing protected parent connection. These are attribution
//! and correlation, never authentication or a reusable administrative grant.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use crate::private_originals::ServiceParentAuthorizationProposal;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServiceParentDecisionSource {
    /// Includes a standing rule whose actual owner explicitly binds its use to
    /// this login. Enrollment supplies lifetime only, not verified identity.
    CurrentLoginDecision { enrollment_id: u64 },
    /// The actual standing-rule owner is independent of a current human login.
    StandingRuleDecision,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServiceParentDecision {
    Authorized {
        source: ServiceParentDecisionSource,
        authorization_ref: String,
        provenance: Map<String, Value>,
    },
    AuthorizedWithToolPolicy {
        source: ServiceParentDecisionSource,
        authorization_ref: String,
        provenance: Map<String, Value>,
        tool_policy_sha256: String,
    },
    RequiresHuman,
    Denied,
    Unavailable,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceParentAdministrationRequest {
    pub exchange_id: u64,
    pub proposal_sha256: String,
    pub proposal: ServiceParentAuthorizationProposal,
}
