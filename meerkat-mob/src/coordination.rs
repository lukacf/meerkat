//! Product-neutral mob coordination records.

use crate::ids::{AgentIdentity, MobId, RunId};
use chrono::{DateTime, Utc};
use meerkat_core::types::SessionId;
use meerkat_core::{PrincipalRef, SurfaceMetadata, SurfaceMetadataError};
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};
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

    fn validate(&self) -> Result<(), MobCoordinationError> {
        if self.is_empty() {
            return Err(MobCoordinationError::MissingOwner);
        }
        if let Some(agent_identity) = &self.agent_identity
            && agent_identity.as_str().trim().is_empty()
        {
            return Err(MobCoordinationError::EmptyOwnerAgentIdentity);
        }
        Ok(())
    }
}

/// Typed optional owning references for a coordination record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CoordinationRecordRefs {
    /// Mob that owns the coordination board. Filled by [`MobCoordinationBoard`].
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

impl WorkIntentStatus {
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
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

impl WorkIntent {
    #[must_use]
    pub fn is_expired_at(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }

    #[must_use]
    pub fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        !self.status.is_terminal() && !self.is_expired_at(now)
    }
}

/// Draft for recording a new [`WorkIntent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWorkIntent {
    pub id: WorkIntentId,
    pub summary: String,
    pub details: Option<String>,
    pub status: WorkIntentStatus,
    pub owner: CoordinationOwner,
    pub owning_refs: CoordinationRecordRefs,
    pub resources: BTreeSet<CoordinationResourceRef>,
    pub metadata: SurfaceMetadata,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClaimStatus {
    Active,
    Released,
    Expired,
    Cancelled,
}

impl ResourceClaimStatus {
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Released | Self::Expired | Self::Cancelled)
    }
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

impl ResourceClaim {
    #[must_use]
    pub fn is_expired_at(&self, now: DateTime<Utc>) -> bool {
        self.status == ResourceClaimStatus::Expired
            || self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }

    #[must_use]
    pub fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        !self.status.is_terminal() && !self.is_expired_at(now)
    }

    #[must_use]
    pub fn overlaps_resources(&self, resources: &BTreeSet<CoordinationResourceRef>) -> bool {
        self.resources
            .iter()
            .any(|resource| resources.contains(resource))
    }
}

/// Draft for recording a new [`ResourceClaim`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewResourceClaim {
    pub id: ResourceClaimId,
    pub kind: ResourceClaimKind,
    pub status: ResourceClaimStatus,
    pub owner: CoordinationOwner,
    pub owning_refs: CoordinationRecordRefs,
    pub resources: BTreeSet<CoordinationResourceRef>,
    pub reason: Option<String>,
    pub metadata: SurfaceMetadata,
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

/// In-memory mob-owned coordination board.
#[derive(Debug, Clone)]
pub struct MobCoordinationBoard {
    mob_id: MobId,
    work_intents: BTreeMap<WorkIntentId, WorkIntent>,
    resource_claims: BTreeMap<ResourceClaimId, ResourceClaim>,
    events: Vec<MobCoordinationEvent>,
    next_event_sequence: u64,
}

impl MobCoordinationBoard {
    #[must_use]
    pub fn new(mob_id: MobId) -> Self {
        Self {
            mob_id,
            work_intents: BTreeMap::new(),
            resource_claims: BTreeMap::new(),
            events: Vec::new(),
            next_event_sequence: 1,
        }
    }

    #[must_use]
    pub fn mob_id(&self) -> &MobId {
        &self.mob_id
    }

    pub fn record_work_intent(
        &mut self,
        mut draft: NewWorkIntent,
        now: DateTime<Utc>,
    ) -> Result<&WorkIntent, MobCoordinationError> {
        if self.work_intents.contains_key(&draft.id) {
            return Err(MobCoordinationError::DuplicateWorkIntent { id: draft.id });
        }
        validate_summary(&draft.summary)?;
        draft.owner.validate()?;
        validate_resources(&draft.resources)?;
        draft.metadata.validate_public()?;
        self.normalize_refs(&mut draft.owning_refs)?;

        let id = draft.id.clone();
        self.work_intents.insert(
            id.clone(),
            WorkIntent {
                id: id.clone(),
                revision: 1,
                summary: draft.summary,
                details: draft.details,
                status: draft.status,
                owner: draft.owner,
                owning_refs: draft.owning_refs,
                resources: draft.resources,
                metadata: draft.metadata,
                created_at: now,
                updated_at: now,
                expires_at: draft.expires_at,
            },
        );
        self.push_event(
            now,
            MobCoordinationEventKind::WorkIntentRecorded {
                intent_id: id.clone(),
            },
        );
        Ok(self
            .work_intents
            .get(&id)
            .expect("inserted work intent must be present"))
    }

    pub fn update_work_intent_status(
        &mut self,
        id: &WorkIntentId,
        expected_revision: u64,
        status: WorkIntentStatus,
        now: DateTime<Utc>,
    ) -> Result<&WorkIntent, MobCoordinationError> {
        let intent = self
            .work_intents
            .get_mut(id)
            .ok_or_else(|| MobCoordinationError::UnknownWorkIntent { id: id.clone() })?;
        ensure_fresh_revision(intent.revision, expected_revision)?;
        if !status.is_terminal() && intent.is_expired_at(now) {
            return Err(MobCoordinationError::ExpiredRecord);
        }
        intent.status = status;
        intent.revision += 1;
        intent.updated_at = now;
        self.push_event(
            now,
            MobCoordinationEventKind::WorkIntentStatusChanged {
                intent_id: id.clone(),
                status,
            },
        );
        Ok(self
            .work_intents
            .get(id)
            .expect("updated work intent must be present"))
    }

    pub fn record_resource_claim(
        &mut self,
        mut draft: NewResourceClaim,
        now: DateTime<Utc>,
    ) -> Result<&ResourceClaim, MobCoordinationError> {
        if self.resource_claims.contains_key(&draft.id) {
            return Err(MobCoordinationError::DuplicateResourceClaim { id: draft.id });
        }
        draft.owner.validate()?;
        validate_resources(&draft.resources)?;
        draft.metadata.validate_public()?;
        self.normalize_refs(&mut draft.owning_refs)?;

        let id = draft.id.clone();
        let kind = draft.kind;
        self.resource_claims.insert(
            id.clone(),
            ResourceClaim {
                id: id.clone(),
                revision: 1,
                kind,
                status: draft.status,
                owner: draft.owner,
                owning_refs: draft.owning_refs,
                resources: draft.resources,
                reason: draft.reason,
                metadata: draft.metadata,
                created_at: now,
                updated_at: now,
                expires_at: draft.expires_at,
            },
        );
        self.push_event(
            now,
            MobCoordinationEventKind::ResourceClaimRecorded {
                claim_id: id.clone(),
                kind,
            },
        );
        Ok(self
            .resource_claims
            .get(&id)
            .expect("inserted resource claim must be present"))
    }

    pub fn update_resource_claim_status(
        &mut self,
        id: &ResourceClaimId,
        expected_revision: u64,
        status: ResourceClaimStatus,
        now: DateTime<Utc>,
    ) -> Result<&ResourceClaim, MobCoordinationError> {
        let claim = self
            .resource_claims
            .get_mut(id)
            .ok_or_else(|| MobCoordinationError::UnknownResourceClaim { id: id.clone() })?;
        ensure_fresh_revision(claim.revision, expected_revision)?;
        if !status.is_terminal() && claim.is_expired_at(now) {
            return Err(MobCoordinationError::ExpiredRecord);
        }
        claim.status = status;
        claim.revision += 1;
        claim.updated_at = now;
        self.push_event(
            now,
            MobCoordinationEventKind::ResourceClaimStatusChanged {
                claim_id: id.clone(),
                status,
            },
        );
        Ok(self
            .resource_claims
            .get(id)
            .expect("updated resource claim must be present"))
    }

    #[must_use]
    pub fn work_intent(&self, id: &WorkIntentId) -> Option<&WorkIntent> {
        self.work_intents.get(id)
    }

    #[must_use]
    pub fn resource_claim(&self, id: &ResourceClaimId) -> Option<&ResourceClaim> {
        self.resource_claims.get(id)
    }

    #[must_use]
    pub fn active_work_intents(&self, now: DateTime<Utc>) -> Vec<&WorkIntent> {
        self.work_intents
            .values()
            .filter(|intent| intent.is_active_at(now))
            .collect()
    }

    #[must_use]
    pub fn active_resource_claims(&self, now: DateTime<Utc>) -> Vec<&ResourceClaim> {
        self.resource_claims
            .values()
            .filter(|claim| claim.is_active_at(now))
            .collect()
    }

    #[must_use]
    pub fn active_work_intents_for_resources(
        &self,
        resources: &BTreeSet<CoordinationResourceRef>,
        now: DateTime<Utc>,
    ) -> Vec<&WorkIntent> {
        self.work_intents
            .values()
            .filter(|intent| intent.is_active_at(now))
            .filter(|intent| resources_overlap(&intent.resources, resources))
            .collect()
    }

    #[must_use]
    pub fn overlapping_resource_claims(
        &self,
        resources: &BTreeSet<CoordinationResourceRef>,
        now: DateTime<Utc>,
    ) -> Vec<&ResourceClaim> {
        self.resource_claims
            .values()
            .filter(|claim| claim.is_active_at(now))
            .filter(|claim| claim.overlaps_resources(resources))
            .collect()
    }

    pub fn observe_claim_overlaps(
        &mut self,
        claim_id: &ResourceClaimId,
        now: DateTime<Utc>,
    ) -> Result<Vec<&ResourceClaim>, MobCoordinationError> {
        let resources = self
            .resource_claims
            .get(claim_id)
            .ok_or_else(|| MobCoordinationError::UnknownResourceClaim {
                id: claim_id.clone(),
            })?
            .resources
            .clone();
        let overlaps: Vec<ResourceClaimId> = self
            .overlapping_resource_claims(&resources, now)
            .into_iter()
            .filter(|claim| &claim.id != claim_id)
            .map(|claim| claim.id.clone())
            .collect();

        self.push_event(
            now,
            MobCoordinationEventKind::ResourceClaimOverlapObserved {
                claim_id: claim_id.clone(),
                overlaps: overlaps.clone(),
            },
        );

        Ok(overlaps
            .iter()
            .filter_map(|id| self.resource_claims.get(id))
            .collect())
    }

    #[must_use]
    pub fn events(&self) -> &[MobCoordinationEvent] {
        &self.events
    }

    fn normalize_refs(
        &self,
        refs: &mut CoordinationRecordRefs,
    ) -> Result<(), MobCoordinationError> {
        match &refs.mob_id {
            Some(mob_id) if mob_id != &self.mob_id => Err(MobCoordinationError::MismatchedMobRef {
                expected: self.mob_id.clone(),
                actual: mob_id.clone(),
            }),
            Some(_) => Ok(()),
            None => {
                refs.mob_id = Some(self.mob_id.clone());
                Ok(())
            }
        }
    }

    fn push_event(&mut self, timestamp: DateTime<Utc>, kind: MobCoordinationEventKind) {
        let sequence = self.next_event_sequence;
        self.next_event_sequence += 1;
        self.events.push(MobCoordinationEvent {
            sequence,
            timestamp,
            mob_id: self.mob_id.clone(),
            kind,
        });
    }
}

/// Snapshot of the board suitable for projection consumers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MobCoordinationSnapshot {
    pub work_intents: Vec<WorkIntent>,
    pub resource_claims: Vec<ResourceClaim>,
}

impl From<&MobCoordinationBoard> for MobCoordinationSnapshot {
    fn from(board: &MobCoordinationBoard) -> Self {
        Self {
            work_intents: board.work_intents.values().cloned().collect(),
            resource_claims: board.resource_claims.values().cloned().collect(),
        }
    }
}

/// Validation and mutation errors for mob coordination records.
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
    #[error("stale revision: expected {expected}, actual {actual}")]
    StaleRevision { expected: u64, actual: u64 },
    #[error("coordination record is expired")]
    ExpiredRecord,
    #[error("coordination mob ref mismatch: expected '{expected}', actual '{actual}'")]
    MismatchedMobRef { expected: MobId, actual: MobId },
}

fn validate_summary(summary: &str) -> Result<(), MobCoordinationError> {
    if summary.trim().is_empty() {
        return Err(MobCoordinationError::EmptyWorkIntentSummary);
    }
    Ok(())
}

fn validate_resources(
    resources: &BTreeSet<CoordinationResourceRef>,
) -> Result<(), MobCoordinationError> {
    if resources.is_empty() {
        return Err(MobCoordinationError::EmptyResources);
    }
    Ok(())
}

fn ensure_fresh_revision(actual: u64, expected: u64) -> Result<(), MobCoordinationError> {
    if actual != expected {
        return Err(MobCoordinationError::StaleRevision { expected, actual });
    }
    Ok(())
}

fn resources_overlap(
    left: &BTreeSet<CoordinationResourceRef>,
    right: &BTreeSet<CoordinationResourceRef>,
) -> bool {
    left.iter().any(|resource| right.contains(resource))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use meerkat_core::{PrincipalKind, PrincipalRef, SurfaceMetadata};
    use serde_json::json;
    use std::collections::BTreeSet;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 26, 12, 0, 0).unwrap()
    }

    fn owner() -> CoordinationOwner {
        CoordinationOwner::agent(AgentIdentity::from("worker-1"))
    }

    fn principal_owner() -> CoordinationOwner {
        CoordinationOwner::principal(
            PrincipalRef::new(PrincipalKind::Human, "human:lane-k").expect("valid principal"),
        )
    }

    fn resource(value: &str) -> CoordinationResourceRef {
        CoordinationResourceRef::new(value).expect("valid resource")
    }

    fn resources(values: &[&str]) -> BTreeSet<CoordinationResourceRef> {
        values.iter().map(|value| resource(value)).collect()
    }

    fn intent(id: &str, resource_refs: &[&str]) -> NewWorkIntent {
        NewWorkIntent {
            id: WorkIntentId::new(id).expect("valid intent id"),
            summary: "coordinate shared edit".to_string(),
            details: None,
            status: WorkIntentStatus::Active,
            owner: owner(),
            owning_refs: CoordinationRecordRefs::default(),
            resources: resources(resource_refs),
            metadata: SurfaceMetadata::default(),
            expires_at: None,
        }
    }

    fn claim(id: &str, kind: ResourceClaimKind, resource_refs: &[&str]) -> NewResourceClaim {
        NewResourceClaim {
            id: ResourceClaimId::new(id).expect("valid claim id"),
            kind,
            status: ResourceClaimStatus::Active,
            owner: owner(),
            owning_refs: CoordinationRecordRefs::default(),
            resources: resources(resource_refs),
            reason: None,
            metadata: SurfaceMetadata::default(),
            expires_at: None,
        }
    }

    #[test]
    fn coordination_board_records_active_work_intents_with_typed_refs_and_metadata() {
        let mut board = MobCoordinationBoard::new(MobId::from("mob-k"));
        let mut metadata = SurfaceMetadata::default();
        metadata
            .labels
            .insert("client.feature".to_string(), "coordination".to_string());
        metadata.app_context = Some(json!({"client_ref": "lane-k"}));

        let mut draft = intent("intent-1", &["file:/src/lib.rs"]);
        draft.owner = principal_owner();
        draft.owning_refs.run_id = Some(RunId::new());
        draft.metadata = metadata.clone();

        let recorded = board
            .record_work_intent(draft, now())
            .expect("record work intent");

        assert_eq!(recorded.revision, 1);
        assert_eq!(recorded.owning_refs.mob_id, Some(MobId::from("mob-k")));
        assert_eq!(recorded.metadata, metadata);
        assert_eq!(board.active_work_intents(now()).len(), 1);
    }

    #[test]
    fn coordination_board_rejects_invalid_empty_ids_resources_and_owner() {
        assert!(WorkIntentId::new(" ").is_err());
        assert!(ResourceClaimId::new("\n").is_err());
        assert!(CoordinationResourceRef::new("").is_err());

        let mut board = MobCoordinationBoard::new(MobId::from("mob-k"));
        let mut empty_owner = intent("intent-1", &["file:/src/lib.rs"]);
        empty_owner.owner = CoordinationOwner::default();
        assert!(matches!(
            board.record_work_intent(empty_owner, now()),
            Err(MobCoordinationError::MissingOwner)
        ));

        let mut empty_resources = claim("claim-1", ResourceClaimKind::Exclusive, &[]);
        empty_resources.resources = BTreeSet::new();
        assert!(matches!(
            board.record_resource_claim(empty_resources, now()),
            Err(MobCoordinationError::EmptyResources)
        ));
    }

    #[test]
    fn coordination_board_rejects_reserved_metadata_spoofing() {
        let mut board = MobCoordinationBoard::new(MobId::from("mob-k"));
        let mut draft = claim(
            "claim-1",
            ResourceClaimKind::SoftReservation,
            &["file:/src/lib.rs"],
        );
        draft
            .metadata
            .labels
            .insert("mob_id".to_string(), "spoof".to_string());

        assert!(matches!(
            board.record_resource_claim(draft, now()),
            Err(MobCoordinationError::InvalidMetadata(_))
        ));
    }

    #[test]
    fn coordination_board_rejects_records_for_a_different_mob() {
        let mut board = MobCoordinationBoard::new(MobId::from("mob-k"));
        let mut draft = intent("intent-1", &["file:/src/lib.rs"]);
        draft.owning_refs.mob_id = Some(MobId::from("other-mob"));

        assert!(matches!(
            board.record_work_intent(draft, now()),
            Err(MobCoordinationError::MismatchedMobRef { expected, actual })
                if expected.as_str() == "mob-k" && actual.as_str() == "other-mob"
        ));
    }

    #[test]
    fn coordination_board_rejects_duplicate_records() {
        let mut board = MobCoordinationBoard::new(MobId::from("mob-k"));
        board
            .record_work_intent(intent("intent-1", &["file:/src/lib.rs"]), now())
            .expect("first intent");
        assert!(matches!(
            board.record_work_intent(intent("intent-1", &["file:/src/lib.rs"]), now()),
            Err(MobCoordinationError::DuplicateWorkIntent { .. })
        ));

        board
            .record_resource_claim(
                claim(
                    "claim-1",
                    ResourceClaimKind::Advisory,
                    &["file:/src/lib.rs"],
                ),
                now(),
            )
            .expect("first claim");
        assert!(matches!(
            board.record_resource_claim(
                claim(
                    "claim-1",
                    ResourceClaimKind::Advisory,
                    &["file:/src/lib.rs"],
                ),
                now(),
            ),
            Err(MobCoordinationError::DuplicateResourceClaim { .. })
        ));
    }

    #[test]
    fn coordination_board_uses_revisions_to_reject_stale_updates() {
        let mut board = MobCoordinationBoard::new(MobId::from("mob-k"));
        let id = WorkIntentId::new("intent-1").unwrap();
        board
            .record_work_intent(intent("intent-1", &["file:/src/lib.rs"]), now())
            .expect("record intent");
        board
            .update_work_intent_status(
                &id,
                1,
                WorkIntentStatus::Blocked,
                now() + Duration::seconds(1),
            )
            .expect("fresh update");

        assert!(matches!(
            board.update_work_intent_status(
                &id,
                1,
                WorkIntentStatus::Completed,
                now() + Duration::seconds(2),
            ),
            Err(MobCoordinationError::StaleRevision {
                expected: 1,
                actual: 2
            })
        ));
    }

    #[test]
    fn coordination_board_filters_expired_and_terminal_records_from_active_queries() {
        let mut board = MobCoordinationBoard::new(MobId::from("mob-k"));
        let mut expired_intent = intent("intent-1", &["file:/src/lib.rs"]);
        expired_intent.expires_at = Some(now() - Duration::seconds(1));
        board
            .record_work_intent(expired_intent, now() - Duration::minutes(1))
            .expect("record expired intent");

        let mut released_claim = claim(
            "claim-1",
            ResourceClaimKind::Exclusive,
            &["file:/src/lib.rs"],
        );
        released_claim.status = ResourceClaimStatus::Released;
        board
            .record_resource_claim(released_claim, now())
            .expect("record released claim");

        assert!(board.active_work_intents(now()).is_empty());
        assert!(board.active_resource_claims(now()).is_empty());
    }

    #[test]
    fn coordination_board_rejects_reactivating_expired_claims() {
        let mut board = MobCoordinationBoard::new(MobId::from("mob-k"));
        let id = ResourceClaimId::new("claim-1").unwrap();
        let mut draft = claim(
            "claim-1",
            ResourceClaimKind::SoftReservation,
            &["file:/src/lib.rs"],
        );
        draft.expires_at = Some(now() - Duration::seconds(1));
        board
            .record_resource_claim(draft, now() - Duration::minutes(1))
            .expect("record expired claim");

        assert!(matches!(
            board.update_resource_claim_status(&id, 1, ResourceClaimStatus::Active, now()),
            Err(MobCoordinationError::ExpiredRecord)
        ));
    }

    #[test]
    fn coordination_board_reports_overlapping_claims_without_enforcing_locks() {
        let mut board = MobCoordinationBoard::new(MobId::from("mob-k"));
        let exclusive = board
            .record_resource_claim(
                claim(
                    "claim-1",
                    ResourceClaimKind::Exclusive,
                    &["file:/src/lib.rs"],
                ),
                now(),
            )
            .expect("record exclusive claim")
            .id
            .clone();
        let soft = board
            .record_resource_claim(
                claim(
                    "claim-2",
                    ResourceClaimKind::SoftReservation,
                    &["file:/src/lib.rs", "file:/src/event.rs"],
                ),
                now(),
            )
            .expect("record overlapping claim")
            .id
            .clone();

        let overlaps = board.overlapping_resource_claims(&resources(&["file:/src/lib.rs"]), now());
        assert_eq!(overlaps.len(), 2);
        assert!(overlaps.iter().any(|claim| claim.id == exclusive));
        assert!(overlaps.iter().any(|claim| claim.id == soft));

        let observed = board
            .observe_claim_overlaps(&soft, now())
            .expect("observe overlaps");
        assert_eq!(
            observed.iter().map(|claim| &claim.id).collect::<Vec<_>>(),
            vec![&exclusive]
        );
        assert!(matches!(
            board.events().last().map(|event| &event.kind),
            Some(MobCoordinationEventKind::ResourceClaimOverlapObserved {
                claim_id,
                overlaps
            }) if claim_id == &soft && overlaps == &vec![exclusive]
        ));
    }

    #[test]
    fn coordination_board_matches_active_work_by_resource() {
        let mut board = MobCoordinationBoard::new(MobId::from("mob-k"));
        board
            .record_work_intent(intent("intent-1", &["file:/src/lib.rs"]), now())
            .expect("record first intent");
        board
            .record_work_intent(intent("intent-2", &["file:/src/run.rs"]), now())
            .expect("record second intent");

        let matches = board.active_work_intents_for_resources(
            &resources(&["file:/src/lib.rs", "file:/src/other.rs"]),
            now(),
        );

        assert_eq!(
            matches
                .iter()
                .map(|intent| intent.id.as_str())
                .collect::<Vec<_>>(),
            vec!["intent-1"]
        );
    }
}
