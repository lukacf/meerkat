//! Product-neutral mob coordination projection types.
//!
//! The authoritative coordination board state — per-entity work-intent and
//! resource-claim records, their optimistic-concurrency revisions, affected
//! resource sets, raw expiry timestamps, and the monotonic coordination event
//! cursor — is owned by `MobMachine` (see
//! `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs`, the folded
//! coordination transitions). The types in this module are *projections* over
//! the typed effects MobMachine emits (`WorkIntentRecorded`,
//! `ResourceClaimRecorded`, `WorkIntentStatusChanged`,
//! `ResourceClaimStatusChanged`, `ResourceClaimOverlapObserved`); they carry no
//! admission/CAS/expiry/overlap authority of their own.

use crate::ids::{AgentIdentity, MobId, RunId};
use chrono::{DateTime, Utc};
use meerkat_core::types::SessionId;
use meerkat_core::{PrincipalRef, SurfaceMetadata, SurfaceMetadataError};
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::collections::BTreeSet;
use std::fmt;

macro_rules! coordination_string_newtype {
    ($(#[$meta:meta])* $name:ident, $empty_error:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, MobCoordinationError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(MobCoordinationError::$empty_error);
                }
                if value.chars().any(char::is_control) {
                    return Err(MobCoordinationError::InvalidControlCharacter {
                        value_type: stringify!($name),
                    });
                }
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

coordination_string_newtype!(
    /// Stable identifier for a product-neutral mob work intent.
    WorkIntentId,
    EmptyWorkIntentId
);

coordination_string_newtype!(
    /// Stable identifier for a mob resource claim.
    ResourceClaimId,
    EmptyResourceClaimId
);

coordination_string_newtype!(
    /// Product-neutral reference to a resource affected by mob coordination.
    CoordinationResourceRef,
    EmptyResourceRef
);

/// Owner identity for a coordination record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CoordinationOwner {
    /// Auth principal that owns the coordination record, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<PrincipalRef>,
    /// Stable mob member identity that owns the coordination record, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_identity: Option<AgentIdentity>,
}

impl CoordinationOwner {
    #[must_use]
    pub fn principal(principal: PrincipalRef) -> Self {
        Self {
            principal: Some(principal),
            agent_identity: None,
        }
    }

    #[must_use]
    pub fn agent(agent_identity: AgentIdentity) -> Self {
        Self {
            principal: None,
            agent_identity: Some(agent_identity),
        }
    }

    #[must_use]
    pub fn principal_and_agent(principal: PrincipalRef, agent_identity: AgentIdentity) -> Self {
        Self {
            principal: Some(principal),
            agent_identity: Some(agent_identity),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.principal.is_none() && self.agent_identity.is_none()
    }
}

/// Typed optional owning references for a coordination record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CoordinationRecordRefs {
    /// Mob that owns the coordination board.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mob_id: Option<MobId>,
    /// Mob run associated with this record, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Runtime session associated with this record, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
}

/// Work intent lifecycle for mob coordination.
///
/// Mirrors `MobCoordinationWorkIntentStatus` in MobMachine. The terminal
/// classification is a projection of the machine's own classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkIntentStatus {
    /// Work is known but not yet actively being performed.
    Planned,
    /// Work is being performed or is ready to be performed.
    Active,
    /// Work is still live but blocked by some other condition.
    Blocked,
    /// Work finished truthfully.
    Completed,
    /// Work was abandoned.
    Cancelled,
}

/// A product-neutral declaration of mob work over affected resources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WorkIntent {
    pub id: WorkIntentId,
    pub revision: u64,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    pub status: WorkIntentStatus,
    pub owner: CoordinationOwner,
    pub owning_refs: CoordinationRecordRefs,
    pub resources: BTreeSet<CoordinationResourceRef>,
    #[serde(default, skip_serializing_if = "SurfaceMetadata::is_empty")]
    pub metadata: SurfaceMetadata,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

/// Advisory strength for a mob resource claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClaimKind {
    /// Informational claim; callers should consider overlap but no lock is implied.
    Advisory,
    /// Soft hold; callers should coordinate before overlapping.
    SoftReservation,
    /// Exclusive intent; overlap remains observable but is not globally enforced here.
    Exclusive,
}

/// Resource claim lifecycle.
///
/// Mirrors `MobCoordinationResourceClaimStatus` in MobMachine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClaimStatus {
    Active,
    Released,
    Expired,
    Cancelled,
}

/// A mob-owned claim over one or more affected resources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResourceClaim {
    pub id: ResourceClaimId,
    pub revision: u64,
    pub kind: ResourceClaimKind,
    pub status: ResourceClaimStatus,
    pub owner: CoordinationOwner,
    pub owning_refs: CoordinationRecordRefs,
    pub resources: BTreeSet<CoordinationResourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "SurfaceMetadata::is_empty")]
    pub metadata: SurfaceMetadata,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

/// Mob-owned coordination event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MobCoordinationEvent {
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub mob_id: MobId,
    pub kind: MobCoordinationEventKind,
}

/// Typed coordination event payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MobCoordinationEventKind {
    WorkIntentRecorded {
        intent_id: WorkIntentId,
    },
    WorkIntentStatusChanged {
        intent_id: WorkIntentId,
        status: WorkIntentStatus,
    },
    ResourceClaimRecorded {
        claim_id: ResourceClaimId,
        kind: ResourceClaimKind,
    },
    ResourceClaimStatusChanged {
        claim_id: ResourceClaimId,
        status: ResourceClaimStatus,
    },
    ResourceClaimOverlapObserved {
        claim_id: ResourceClaimId,
        overlaps: Vec<ResourceClaimId>,
    },
}

/// Snapshot of the coordination board suitable for projection consumers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MobCoordinationSnapshot {
    pub work_intents: Vec<WorkIntent>,
    pub resource_claims: Vec<ResourceClaim>,
}

/// Validation and projection errors for mob coordination records.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MobCoordinationError {
    #[error("work intent id must not be empty")]
    EmptyWorkIntentId,
    #[error("resource claim id must not be empty")]
    EmptyResourceClaimId,
    #[error("coordination resource ref must not be empty")]
    EmptyResourceRef,
    #[error("{value_type} must not contain control characters")]
    InvalidControlCharacter { value_type: &'static str },
    #[error("coordination record owner must include a principal or agent identity")]
    MissingOwner,
    #[error("coordination owner agent identity must not be empty")]
    EmptyOwnerAgentIdentity,
    #[error("coordination record must affect at least one resource")]
    EmptyResources,
    #[error("work intent summary must not be empty")]
    EmptyWorkIntentSummary,
    #[error("invalid coordination metadata: {0}")]
    InvalidMetadata(#[from] SurfaceMetadataError),
    #[error("work intent '{id}' already exists")]
    DuplicateWorkIntent { id: WorkIntentId },
    #[error("resource claim '{id}' already exists")]
    DuplicateResourceClaim { id: ResourceClaimId },
    #[error("unknown work intent '{id}'")]
    UnknownWorkIntent { id: WorkIntentId },
    #[error("unknown resource claim '{id}'")]
    UnknownResourceClaim { id: ResourceClaimId },
    #[error("coordination mob ref mismatch: expected '{expected}', actual '{actual}'")]
    MismatchedMobRef { expected: MobId, actual: MobId },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordination_newtypes_reject_empty_and_control_chars() {
        assert!(WorkIntentId::new(" ").is_err());
        assert!(ResourceClaimId::new("\n").is_err());
        assert!(CoordinationResourceRef::new("").is_err());
        assert!(WorkIntentId::new("intent-1").is_ok());
    }
}
