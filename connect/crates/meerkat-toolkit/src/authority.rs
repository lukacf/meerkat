//! Toolkit setup proposal constraints and native work evidence.
//!
//! These constraints can narrow a setup proposal. They do not issue native
//! grants, authenticate an input, authorize an operation, or restore custody.

mod request_context;
pub(crate) use request_context::principal_codec;
pub use request_context::{
    SetupContextError, SetupProposalConstraints, SetupRequestEvidence, SetupUseContext,
};

#[cfg(not(target_arch = "wasm32"))]
mod request_context_host;
#[cfg(not(target_arch = "wasm32"))]
pub use request_context_host::check_ingress_correspondence;

#[cfg(not(target_arch = "wasm32"))]
mod channel_request_owner;
#[cfg(not(target_arch = "wasm32"))]
pub use channel_request_owner::{
    ChannelInboxObservation, ChannelRequestKey, ChannelSourceError, ChannelSourceMember,
    ChannelSourceProvenance, SqliteChannelSourceReader,
};

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use toolkit_package::Id;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    FindAndInstall,
    InstallArtifacts,
    ConnectAccount,
    GrantService,
    ExposeFrontend,
    InvokeService,
    Deliver,
    ReadMemory,
    Delegate,
}

/// An action and its resource stay paired through every delegation hop.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionPermission {
    pub action: Action,
    pub resource: Id,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LifetimeScope {
    Task { task: Id },
    Member { member: Id },
    Deployment { deployment: Id },
}

/// Lifetime of an approved role, independent of the setup proposal's validity.
/// An absent expiry means until revoked, in the named scope.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleLifetime {
    pub scope: LifetimeScope,
    pub expires_at_ms: Option<u64>,
}

impl RoleLifetime {
    pub fn permits(&self, requested: &Self) -> bool {
        self.scope == requested.scope
            && match (self.expires_at_ms, requested.expires_at_ms) {
                (None, _) => true,
                (Some(limit), Some(expiry)) => expiry <= limit,
                (Some(_), None) => false,
            }
    }

    pub fn active_at(&self, now_ms: u64) -> bool {
        self.expires_at_ms.is_none_or(|expiry| now_ms < expiry)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServicePermission {
    pub target: Id,
    pub connection: Id,
    pub operation: Id,
    pub resource: Id,
    pub lifetime: RoleLifetime,
}

impl ServicePermission {
    pub fn permits(&self, requested: &Self) -> bool {
        self.target == requested.target
            && self.connection == requested.connection
            && self.operation == requested.operation
            && self.resource == requested.resource
            && self.lifetime.permits(&requested.lifetime)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontendPermission {
    pub target: Id,
    pub endpoint: Id,
    pub participant: Id,
    pub interaction: Id,
    pub lifetime: RoleLifetime,
}

impl FrontendPermission {
    pub fn permits(&self, requested: &Self) -> bool {
        self.target == requested.target
            && self.endpoint == requested.endpoint
            && self.participant == requested.participant
            && self.interaction == requested.interaction
            && self.lifetime.permits(&requested.lifetime)
    }
}

/// Exact proposal allowlists. Empty sets permit no proposal; native grants remain separate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ceiling {
    pub actions: BTreeSet<ActionPermission>,
    pub services: BTreeSet<ServicePermission>,
    pub frontends: BTreeSet<FrontendPermission>,
    pub output_audiences: BTreeSet<Id>,
}

impl Ceiling {
    pub fn permits(&self, action: Action, resource: &Id) -> bool {
        self.actions.contains(&ActionPermission {
            action,
            resource: resource.clone(),
        })
    }

    pub fn contains(&self, other: &Self) -> bool {
        other.actions.is_subset(&self.actions)
            && other
                .services
                .iter()
                .all(|wanted| self.services.iter().any(|allowed| allowed.permits(wanted)))
            && other
                .frontends
                .iter()
                .all(|wanted| self.frontends.iter().any(|allowed| allowed.permits(wanted)))
            && other.output_audiences.is_subset(&self.output_audiences)
    }
}

#[cfg(test)]
pub(crate) mod test_support;
