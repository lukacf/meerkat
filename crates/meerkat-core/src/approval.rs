//! Durable approval records and service contracts.
//!
//! Generated approval lifecycle authority owns approval status transitions.
//! Public surfaces may request, list, read, and decide approvals; the service
//! stores and projects the generated lifecycle decisions.
//!
//! The same generated owner also holds process-local review attempts for
//! retained native operations (see [`review`]). Review attempts are never
//! persisted or restored, so a stored approval record can never reconstruct
//! a review allow or spendable consent.

pub mod review;

use crate::generated::approval_lifecycle::{
    ApprovalLifecycleDecision, ApprovalLifecycleError, ApprovalLifecycleMachineAuthority,
    ApprovalLifecycleOutcome, ApprovalLifecycleRejectionReason, ApprovalLifecycleStatus,
    ReviewAttemptStatus, ReviewRetirementReason, ReviewVerdict,
};
use crate::lifecycle::identifiers::RunId;
use crate::{SessionId, SurfaceMetadata, ToolCallId};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use review::{ReservedReviewError, ReviewAttemptHandle, ReviewOwnerError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

/// Durable approval id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct ApprovalId(#[cfg_attr(feature = "schema", schemars(with = "String"))] pub Uuid);

impl ApprovalId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ApprovalId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ApprovalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for ApprovalId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(value)?))
    }
}

/// Principal identifier used for requester and decision actor projections.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct ApprovalPrincipalId(String);

impl ApprovalPrincipalId {
    pub fn new(value: impl Into<String>) -> Result<Self, ApprovalError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ApprovalError::InvalidPrincipal);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ApprovalPrincipalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Typed reference to a mob owning an approval.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct ApprovalMobRef(String);

impl ApprovalMobRef {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ApprovalMobRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Typed reference to a mob member owning an approval.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct ApprovalMemberRef(String);

impl ApprovalMemberRef {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ApprovalMemberRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Typed identifier for the resource affected by an approval.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct ApprovalResourceId(String);

impl ApprovalResourceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ApprovalResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Typed owner for an approval request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "owner_type", rename_all = "snake_case")]
pub enum ApprovalOwnerRef {
    Runtime,
    Session {
        session_id: SessionId,
    },
    Mob {
        mob_id: ApprovalMobRef,
    },
    Run {
        run_id: RunId,
    },
    ToolCall {
        tool_call_id: ToolCallId,
    },
    ExternalMember {
        mob_id: ApprovalMobRef,
        member_ref: ApprovalMemberRef,
    },
}

/// Typed resource kind affected by an approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalResourceKind {
    File,
    ShellCommand,
    ToolCall,
    Device,
    Runtime,
    Network,
    Other,
}

/// Resource affected by an approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalResourceRef {
    pub kind: ApprovalResourceKind,
    pub id: ApprovalResourceId,
}

/// Typed action kind for the proposed action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalActionKind {
    ShellCommand,
    FileWrite,
    FileDelete,
    NetworkCall,
    DeviceControl,
    ToolCall,
    Other,
}

/// Action proposed by the requester.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalProposedAction {
    pub kind: ApprovalActionKind,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
}

/// Risk classification for an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRisk {
    Low,
    Medium,
    High,
    Critical,
}

/// Allowed terminal decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

/// Approval record status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
    Cancelled,
}

/// Durable decision audit record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalDecisionRecord {
    pub decision: ApprovalDecision,
    pub actor: ApprovalPrincipalId,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub decided_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<serde_json::Value>,
}

/// Durable approval record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalRecord {
    pub approval_id: ApprovalId,
    pub status: ApprovalStatus,
    pub requester: ApprovalPrincipalId,
    pub owner: ApprovalOwnerRef,
    pub resource: ApprovalResourceRef,
    pub proposed_action: ApprovalProposedAction,
    pub risk: ApprovalRisk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<serde_json::Value>,
    pub allowed_decisions: BTreeSet<ApprovalDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
    pub expires_at: Option<DateTime<Utc>>,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub created_at: DateTime<Utc>,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub updated_at: DateTime<Utc>,
    pub metadata: SurfaceMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_provenance: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<ApprovalDecisionRecord>,
}

/// Input used by tools/runtime code to request an approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalRequest {
    pub requester: ApprovalPrincipalId,
    pub owner: ApprovalOwnerRef,
    pub resource: ApprovalResourceRef,
    pub proposed_action: ApprovalProposedAction,
    pub risk: ApprovalRisk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<serde_json::Value>,
    pub allowed_decisions: BTreeSet<ApprovalDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "SurfaceMetadata::is_empty")]
    pub metadata: SurfaceMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_provenance: Option<serde_json::Value>,
}

/// Filter for listing approvals.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalListFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ApprovalStatus>,
}

/// Errors from the approval service.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalError {
    #[error("approval not found: {approval_id}")]
    NotFound { approval_id: ApprovalId },
    #[error("approval has already been decided: {approval_id}")]
    AlreadyDecided { approval_id: ApprovalId },
    #[error("approval is expired: {approval_id}")]
    Expired { approval_id: ApprovalId },
    #[error("decision is not allowed for approval: {decision:?}")]
    InvalidDecision { decision: ApprovalDecision },
    #[error("approval request must allow at least one decision")]
    EmptyAllowedDecisions,
    #[error("approval principal id must not be empty")]
    InvalidPrincipal,
    #[error(transparent)]
    InvalidMetadata(#[from] crate::SurfaceMetadataError),
    #[error("approval store error: {0}")]
    Store(String),
}

/// Durable approval store mechanics.
///
/// Stores persist full records and do not decide status legality.
pub trait ApprovalStore: Send + Sync {
    fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError>;
    fn put(&self, record: &ApprovalRecord) -> Result<(), ApprovalStoreError>;
    fn is_persistent(&self) -> bool;
}

/// Approval store errors, erased at the service boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalStoreError {
    #[error("{0}")]
    Backend(String),
}

impl From<ApprovalStoreError> for ApprovalError {
    fn from(value: ApprovalStoreError) -> Self {
        Self::Store(value.to_string())
    }
}

/// In-memory approval store for tests and process-local runtimes.
#[derive(Debug, Default)]
pub struct InMemoryApprovalStore {
    records: RwLock<BTreeMap<ApprovalId, ApprovalRecord>>,
}

impl InMemoryApprovalStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ApprovalStore for InMemoryApprovalStore {
    fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
        Ok(self.records.read().values().cloned().collect())
    }

    fn put(&self, record: &ApprovalRecord) -> Result<(), ApprovalStoreError> {
        self.records
            .write()
            .insert(record.approval_id.clone(), record.clone());
        Ok(())
    }

    fn is_persistent(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone)]
struct ApprovalServiceState {
    records: BTreeMap<ApprovalId, ApprovalRecord>,
    authority: ApprovalLifecycleMachineAuthority,
}

impl ApprovalServiceState {
    fn empty() -> Self {
        Self {
            records: BTreeMap::new(),
            authority: ApprovalLifecycleMachineAuthority::new(),
        }
    }

    fn from_records(records: Vec<ApprovalRecord>) -> Result<Self, ApprovalError> {
        let mut state = Self::empty();
        for record in records {
            restore_record_into_authority(&mut state.authority, &record)?;
            state.records.insert(record.approval_id.clone(), record);
        }
        Ok(state)
    }
}

fn approval_lifecycle_status(status: ApprovalStatus) -> ApprovalLifecycleStatus {
    match status {
        ApprovalStatus::Pending => ApprovalLifecycleStatus::Pending,
        ApprovalStatus::Approved => ApprovalLifecycleStatus::Approved,
        ApprovalStatus::Denied => ApprovalLifecycleStatus::Denied,
        ApprovalStatus::Expired => ApprovalLifecycleStatus::Expired,
        ApprovalStatus::Cancelled => ApprovalLifecycleStatus::Cancelled,
    }
}

fn approval_status_from_lifecycle(status: ApprovalLifecycleStatus) -> ApprovalStatus {
    match status {
        ApprovalLifecycleStatus::Pending => ApprovalStatus::Pending,
        ApprovalLifecycleStatus::Approved => ApprovalStatus::Approved,
        ApprovalLifecycleStatus::Denied => ApprovalStatus::Denied,
        ApprovalLifecycleStatus::Expired => ApprovalStatus::Expired,
        ApprovalLifecycleStatus::Cancelled => ApprovalStatus::Cancelled,
    }
}

fn approval_lifecycle_decision(decision: ApprovalDecision) -> ApprovalLifecycleDecision {
    match decision {
        ApprovalDecision::Approve => ApprovalLifecycleDecision::Approve,
        ApprovalDecision::Deny => ApprovalLifecycleDecision::Deny,
    }
}

fn allowed_decision_flags(allowed_decisions: &BTreeSet<ApprovalDecision>) -> (bool, bool) {
    (
        allowed_decisions.contains(&ApprovalDecision::Approve),
        allowed_decisions.contains(&ApprovalDecision::Deny),
    )
}

fn lifecycle_rejection_error(
    approval_id: &ApprovalId,
    reason: ApprovalLifecycleRejectionReason,
    decision: Option<ApprovalDecision>,
) -> ApprovalError {
    match reason {
        ApprovalLifecycleRejectionReason::NotFound => ApprovalError::NotFound {
            approval_id: approval_id.clone(),
        },
        ApprovalLifecycleRejectionReason::AlreadyDecided => ApprovalError::AlreadyDecided {
            approval_id: approval_id.clone(),
        },
        ApprovalLifecycleRejectionReason::Expired => ApprovalError::Expired {
            approval_id: approval_id.clone(),
        },
        ApprovalLifecycleRejectionReason::InvalidDecision => {
            let Some(decision) = decision else {
                return ApprovalError::Store(format!(
                    "generated approval lifecycle rejected {approval_id} with InvalidDecision but no decision context"
                ));
            };
            ApprovalError::InvalidDecision { decision }
        }
        ApprovalLifecycleRejectionReason::EmptyAllowedDecisions => {
            ApprovalError::EmptyAllowedDecisions
        }
        ApprovalLifecycleRejectionReason::AlreadyExists
        | ApprovalLifecycleRejectionReason::InvalidRestoredRecord
        | ApprovalLifecycleRejectionReason::ReviewRetired
        | ApprovalLifecycleRejectionReason::ReviewNotSatisfied
        | ApprovalLifecycleRejectionReason::ReviewPending => ApprovalError::Store(format!(
            "generated approval lifecycle authority rejected {approval_id} with {reason:?}"
        )),
    }
}

fn lifecycle_status_from_outcome(
    approval_id: &ApprovalId,
    outcome: ApprovalLifecycleOutcome,
    decision: Option<ApprovalDecision>,
) -> Result<ApprovalStatus, ApprovalError> {
    match outcome {
        ApprovalLifecycleOutcome::Status(status) => Ok(approval_status_from_lifecycle(status)),
        ApprovalLifecycleOutcome::Rejected(reason) => {
            Err(lifecycle_rejection_error(approval_id, reason, decision))
        }
        ApprovalLifecycleOutcome::ReviewStatus(status) => Err(ApprovalError::Store(format!(
            "generated approval lifecycle emitted review status {status:?} for approval {approval_id}"
        ))),
    }
}

fn review_status_from_outcome(
    outcome: Result<ApprovalLifecycleOutcome, ApprovalLifecycleError>,
) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
    match outcome {
        Ok(ApprovalLifecycleOutcome::ReviewStatus(status)) => Ok(status),
        Ok(ApprovalLifecycleOutcome::Rejected(reason)) => Err(ReviewOwnerError::Rejected(reason)),
        Ok(ApprovalLifecycleOutcome::Status(_)) | Err(_) => Err(ReviewOwnerError::Unavailable),
    }
}

fn lifecycle_error(error: impl std::fmt::Display) -> ApprovalError {
    ApprovalError::Store(format!(
        "generated approval lifecycle authority failed: {error}"
    ))
}

fn restored_lifecycle_decision(record: &ApprovalRecord) -> Option<ApprovalLifecycleDecision> {
    record
        .decision
        .as_ref()
        .map(|decision| approval_lifecycle_decision(decision.decision))
}

fn restore_record_into_authority(
    authority: &mut ApprovalLifecycleMachineAuthority,
    record: &ApprovalRecord,
) -> Result<(), ApprovalError> {
    let (approve_allowed, deny_allowed) = allowed_decision_flags(&record.allowed_decisions);
    let outcome = authority
        .restore_approval(
            record.approval_id.to_string(),
            approval_lifecycle_status(record.status),
            approve_allowed,
            deny_allowed,
            record.expires_at.is_some(),
            restored_lifecycle_decision(record),
        )
        .map_err(lifecycle_error)?;
    let restored_status = lifecycle_status_from_outcome(
        &record.approval_id,
        outcome,
        record.decision.as_ref().map(|decision| decision.decision),
    )?;
    if restored_status != record.status {
        return Err(ApprovalError::Store(format!(
            "generated approval lifecycle restored {} as {:?}, stored {:?}",
            record.approval_id, restored_status, record.status
        )));
    }
    Ok(())
}

/// In-process approval service.
#[derive(Clone)]
pub struct ApprovalService {
    state: Arc<RwLock<ApprovalServiceState>>,
    store: Arc<dyn ApprovalStore>,
    unavailable_reason: Option<Arc<str>>,
    /// Test-only signal: a reserved review commit passed every pre-check and
    /// is about to take the commit reservation.
    /// Review attempt disposals handed over without waiting; settled under
    /// the owner's next review commit. Never held while waiting for `state`.
    queued_disposals: Arc<parking_lot::Mutex<Vec<QueuedReviewDisposal>>>,
    #[cfg(test)]
    reservation_probe: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Test-only count of review-path acquisitions of `state`.
    #[cfg(test)]
    review_lock_acquisitions: Arc<std::sync::atomic::AtomicUsize>,
}

impl ApprovalService {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(ApprovalServiceState::empty())),
            store: Arc::new(InMemoryApprovalStore::new()),
            unavailable_reason: None,
            queued_disposals: Arc::default(),
            #[cfg(test)]
            reservation_probe: None,
            #[cfg(test)]
            review_lock_acquisitions: Arc::default(),
        }
    }

    pub fn with_store(store: Arc<dyn ApprovalStore>) -> Result<Self, ApprovalError> {
        let state = ApprovalServiceState::from_records(store.load_all()?)?;
        Ok(Self {
            state: Arc::new(RwLock::new(state)),
            store,
            unavailable_reason: None,
            queued_disposals: Arc::default(),
            #[cfg(test)]
            reservation_probe: None,
            #[cfg(test)]
            review_lock_acquisitions: Arc::default(),
        })
    }

    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            state: Arc::new(RwLock::new(ApprovalServiceState::empty())),
            store: Arc::new(InMemoryApprovalStore::new()),
            unavailable_reason: Some(Arc::from(reason.into())),
            queued_disposals: Arc::default(),
            #[cfg(test)]
            reservation_probe: None,
            #[cfg(test)]
            review_lock_acquisitions: Arc::default(),
        }
    }

    #[must_use]
    pub fn is_persistent(&self) -> bool {
        self.store.is_persistent()
    }

    fn ensure_available(&self) -> Result<(), ApprovalError> {
        if let Some(reason) = &self.unavailable_reason {
            return Err(ApprovalError::Store(format!(
                "approval service unavailable: {reason}"
            )));
        }
        Ok(())
    }

    pub fn request(&self, request: ApprovalRequest) -> Result<ApprovalRecord, ApprovalError> {
        self.ensure_available()?;
        if request.requester.as_str().trim().is_empty() {
            return Err(ApprovalError::InvalidPrincipal);
        }
        request.metadata.validate_public()?;
        let now = Utc::now();
        let approval_id = ApprovalId::new();
        let mut state = self.state.write();
        let result = self.request_locked(&mut state, request, approval_id, now);
        // Settles queued review disposals as the lock is released.
        self.release_state(state).run();
        result
    }

    fn request_locked(
        &self,
        state: &mut ApprovalServiceState,
        request: ApprovalRequest,
        approval_id: ApprovalId,
        now: DateTime<Utc>,
    ) -> Result<ApprovalRecord, ApprovalError> {
        let (approve_allowed, deny_allowed) = allowed_decision_flags(&request.allowed_decisions);
        let mut authority = state.authority.clone();
        let outcome = authority
            .create_approval(
                approval_id.to_string(),
                approve_allowed,
                deny_allowed,
                request.expires_at.is_some(),
            )
            .map_err(lifecycle_error)?;
        let status = lifecycle_status_from_outcome(&approval_id, outcome, None)?;
        let record = ApprovalRecord {
            approval_id,
            status,
            requester: request.requester,
            owner: request.owner,
            resource: request.resource,
            proposed_action: request.proposed_action,
            risk: request.risk,
            request_body: request.request_body,
            allowed_decisions: request.allowed_decisions,
            expires_at: request.expires_at,
            created_at: now,
            updated_at: now,
            metadata: request.metadata,
            request_provenance: request.request_provenance,
            decision: None,
        };
        self.store.put(&record)?;
        state.authority = authority;
        state
            .records
            .insert(record.approval_id.clone(), record.clone());
        Ok(record)
    }

    pub fn get(&self, approval_id: &ApprovalId) -> Result<ApprovalRecord, ApprovalError> {
        self.ensure_available()?;
        self.refresh_expiry(approval_id)?;
        let record = self.state.read().records.get(approval_id).cloned();
        // A reader may have made a disposer's try-lock fail; settle after it.
        self.settle_queued_disposals_now().run();
        record.ok_or_else(|| ApprovalError::NotFound {
            approval_id: approval_id.clone(),
        })
    }

    pub fn list(&self, filter: ApprovalListFilter) -> Result<Vec<ApprovalRecord>, ApprovalError> {
        self.ensure_available()?;
        self.refresh_all_expiry()?;
        let records = self
            .state
            .read()
            .records
            .values()
            .filter(|record| filter.status.is_none_or(|status| record.status == status))
            .cloned()
            .collect();
        self.settle_queued_disposals_now().run();
        Ok(records)
    }

    pub fn decide(
        &self,
        approval_id: &ApprovalId,
        decision: ApprovalDecision,
        actor: ApprovalPrincipalId,
        reason: Option<String>,
        provenance: Option<serde_json::Value>,
    ) -> Result<ApprovalRecord, ApprovalError> {
        self.ensure_available()?;
        if actor.as_str().trim().is_empty() {
            return Err(ApprovalError::InvalidPrincipal);
        }
        let now = Utc::now();
        let mut state = self.state.write();
        let result = self.decide_locked(
            &mut state,
            approval_id,
            decision,
            actor,
            reason,
            provenance,
            now,
        );
        // Settles review disposals queued before or during this commit as
        // the lock is released, so none waits for a later owner call.
        self.release_state(state).run();
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn decide_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
        decision: ApprovalDecision,
        actor: ApprovalPrincipalId,
        reason: Option<String>,
        provenance: Option<serde_json::Value>,
        now: DateTime<Utc>,
    ) -> Result<ApprovalRecord, ApprovalError> {
        if !state.records.contains_key(approval_id) {
            let mut authority = state.authority.clone();
            let outcome = authority
                .decide_approval(
                    approval_id.to_string(),
                    approval_lifecycle_decision(decision),
                )
                .map_err(lifecycle_error)?;
            return match outcome {
                ApprovalLifecycleOutcome::Rejected(reason) => Err(lifecycle_rejection_error(
                    approval_id,
                    reason,
                    Some(decision),
                )),
                ApprovalLifecycleOutcome::Status(status) => Err(ApprovalError::Store(format!(
                    "generated approval lifecycle emitted {status:?} for missing approval {approval_id}"
                ))),
                ApprovalLifecycleOutcome::ReviewStatus(status) => {
                    Err(ApprovalError::Store(format!(
                        "generated approval lifecycle emitted review status {status:?} for missing approval {approval_id}"
                    )))
                }
            };
        }

        self.refresh_expiry_locked(state, approval_id, now)?;
        let record =
            state
                .records
                .get(approval_id)
                .cloned()
                .ok_or_else(|| ApprovalError::NotFound {
                    approval_id: approval_id.clone(),
                })?;

        let mut authority = state.authority.clone();
        let outcome = authority
            .decide_approval(
                approval_id.to_string(),
                approval_lifecycle_decision(decision),
            )
            .map_err(lifecycle_error)?;
        let status = lifecycle_status_from_outcome(approval_id, outcome, Some(decision))?;

        let mut decided_record = record;
        decided_record.status = status;
        decided_record.updated_at = now;
        decided_record.decision = Some(ApprovalDecisionRecord {
            decision,
            actor,
            decided_at: now,
            reason,
            provenance,
        });
        self.store.put(&decided_record)?;
        state.authority = authority;
        state
            .records
            .insert(approval_id.clone(), decided_record.clone());
        Ok(decided_record)
    }

    /// Test adapter: apply one raw review input in place under an unbounded
    /// lock. Production review inputs go through the non-blocking reservation or
    /// the non-blocking disposal. Review inputs never touch the store: review
    /// attempts are memory-only by contract.
    #[cfg(test)]
    fn apply_review(
        &self,
        apply: impl FnOnce(
            &mut ApprovalLifecycleMachineAuthority,
        ) -> Result<ApprovalLifecycleOutcome, ApprovalLifecycleError>,
    ) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
        self.ensure_available()
            .map_err(|_| ReviewOwnerError::Unavailable)?;
        let mut state = self.state.write();
        self.count_review_lock();
        let result = review_status_from_outcome(apply(&mut state.authority));
        self.release_state(state).run();
        result
    }

    /// Commit one review input that admits an effect (a verdict, or the spend
    /// at entry) under ONE commit reservation, the same state lock `decide`
    /// and expiry hold through `ApprovalStore::put`:
    ///
    /// 0. first settle already queued disposals and carry their reports
    ///    back to the caller (they run arbitrary observers, so the caller
    ///    delivers them only outside its locks, and for a spend after the
    ///    effect);
    /// 1. take the reservation WITHOUT waiting (callers run on async
    ///    threads; a timed wait would still block them): contention refuses
    ///    locally at once and queues this attempt's disposal;
    /// 2. under it, check the attempt's identity binding and run
    ///    `final_check` (deadline and currentness) immediately before the
    ///    conditional commit;
    /// 3. commit, and for a spend also dispose the attempt in the same
    ///    reservation, so nothing waits on this owner and no queued report
    ///    runs between the spend and the leaf's effect; a refusal retires and
    ///    disposes the attempt in the same reservation.
    ///
    /// On refusal the returned disposal is the owner's settled result, or
    /// `None` when it was queued (`on_queued` then reports it once settled).
    /// `final_check` runs under the reservation and must not re-enter this
    /// owner; policy owners never consult approval state.
    #[allow(clippy::too_many_arguments)]
    fn commit_review_reserved<E>(
        &self,
        handle: &ReviewAttemptHandle,
        bound: Option<(
            &crate::authorization::PreparedOperationCheck,
            &crate::authorization::WorkAuthorizationContext,
        )>,
        final_check: impl FnOnce() -> Result<(), E>,
        commit: impl FnOnce(
            &mut ApprovalLifecycleMachineAuthority,
        ) -> Result<ApprovalLifecycleOutcome, ApprovalLifecycleError>,
        dispose_on_commit: bool,
        retirement: impl FnOnce(&ReservedReviewError<E>) -> ReviewRetirementReason,
        on_queued: impl FnOnce() -> QueuedDisposalReport,
    ) -> Result<(ReviewAttemptStatus, SettledReports), RefusedReviewCommit<E>> {
        if self.ensure_available().is_err() {
            return Err(RefusedReviewCommit {
                error: ReservedReviewError::Owner(ReviewOwnerError::Unavailable),
                disposal: None,
                reports: SettledReports::default(),
            });
        }
        // Settle queued disposals before the reservation and final check;
        // their reports go back to the caller, which runs them only after it
        // released its own locks (and, for a spend, after the effect).
        let mut reports = self.settle_queued_disposals_now();
        #[cfg(test)]
        if let Some(probe) = &self.reservation_probe {
            probe();
        }
        let Some(mut state) = self.state.try_write() else {
            let error = ReservedReviewError::Contended;
            let reason = retirement(&error);
            let (disposal, more) = self.dispose_review(handle.id(), Some(reason), on_queued);
            reports.absorb(more);
            return Err(RefusedReviewCommit {
                error,
                disposal,
                reports,
            });
        };
        self.count_review_lock();
        let refused = if bound.is_some_and(|(check, work)| !handle.bound_to(check.binding(), work))
        {
            Some(ReservedReviewError::Owner(ReviewOwnerError::Mismatch))
        } else if let Err(failure) = final_check() {
            Some(ReservedReviewError::FinalCheck(failure))
        } else {
            None
        };
        let result = match refused {
            Some(error) => Err(error),
            None => match review_status_from_outcome(commit(&mut state.authority)) {
                Ok(status) => {
                    if dispose_on_commit {
                        // Same reservation: no second acquisition before the effect.
                        let _ = review_status_from_outcome(
                            state.authority.release_review(handle.id().to_owned()),
                        );
                    }
                    Ok(status)
                }
                Err(error) => Err(ReservedReviewError::Owner(error)),
            },
        };
        let result = result.map_err(|error| {
            let reason = retirement(&error);
            let disposal = dispose_locked(&mut state.authority, handle.id(), Some(reason));
            RefusedReviewCommit {
                error,
                disposal: Some(disposal),
                reports: SettledReports::default(),
            }
        });
        // Disposals queued during this commit are settled as the lock is
        // released. Every report goes back to the caller, which runs them
        // only after releasing its own locks: after the effect for a spend,
        // otherwise at once.
        reports.absorb(self.release_state(state));
        match result {
            Ok(status) => Ok((status, reports)),
            Err(mut refused) => {
                refused.reports = reports;
                Err(refused)
            }
        }
    }

    /// Settle and report queued disposals now if the owner is free, without
    /// waiting; reports run after the lock is released.
    #[must_use = "run the reports once outside every lock"]
    fn settle_queued_disposals_now(&self) -> SettledReports {
        if self.queued_disposals.lock().is_empty() {
            return SettledReports::default();
        }
        match self.state.try_write() {
            Some(state) => {
                self.count_review_lock();
                self.release_state(state)
            }
            None => SettledReports::default(),
        }
    }

    /// Retire (when `retire` is set) and dispose an attempt WITHOUT waiting
    /// for the owner: settled now when the state lock is free (the result is
    /// returned), otherwise queued for the owner's next commit and reported
    /// through `on_queued` once settled (`None` is returned). Reports of
    /// other disposals settled here are returned, never run: the caller may
    /// hold its own locks and runs them after releasing those.
    pub(crate) fn dispose_review(
        &self,
        id: &str,
        retire: Option<ReviewRetirementReason>,
        on_queued: impl FnOnce() -> QueuedDisposalReport,
    ) -> (
        Option<Result<ReviewAttemptStatus, ReviewOwnerError>>,
        SettledReports,
    ) {
        if self.ensure_available().is_err() {
            return (
                Some(Err(ReviewOwnerError::Unavailable)),
                SettledReports::default(),
            );
        }
        if let Some(mut state) = self.state.try_write() {
            self.count_review_lock();
            let result = dispose_locked(&mut state.authority, id, retire);
            return (Some(result), self.release_state(state));
        }
        self.queued_disposals.lock().push(QueuedReviewDisposal {
            id: id.to_owned(),
            retire,
            report: on_queued(),
        });
        // Every lock holder settles the queue as it releases (writers under
        // the queue mutex, readers right after), so an entry pushed while the
        // lock was held is never stranded. If the holder already released,
        // settle now, still without waiting.
        match self.state.try_write() {
            Some(state) => {
                self.count_review_lock();
                (None, self.release_state(state))
            }
            None => (None, SettledReports::default()),
        }
    }

    /// Release the state lock, settling every queued disposal first UNDER
    /// the queue mutex and unlocking while still holding it: a disposer that
    /// failed its try-lock pushes under that mutex, so its entry is either
    /// in this final drain or pushed after the unlock (when its own retry can
    /// take the lock). No entry is lost between the last drain and the
    /// unlock. The reports run later, outside both locks.
    fn release_state(
        &self,
        mut state: parking_lot::RwLockWriteGuard<'_, ApprovalServiceState>,
    ) -> SettledReports {
        let mut queue = self.queued_disposals.lock();
        let settled = std::mem::take(&mut *queue)
            .into_iter()
            .map(|disposal| {
                let result = dispose_locked(&mut state.authority, &disposal.id, disposal.retire);
                (disposal.report, result)
            })
            .collect();
        drop(state);
        drop(queue);
        SettledReports(settled)
    }

    #[inline]
    fn count_review_lock(&self) {
        #[cfg(test)]
        self.review_lock_acquisitions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    fn with_reservation_probe(mut self, probe: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.reservation_probe = Some(probe);
        self
    }

    /// Issue a fresh, never-reused attempt bound to the exact operation and
    /// its original admitted work, through the same non-blocking reservation:
    /// a held owner refuses locally (`Contended`) at once with no attempt
    /// created, so nothing needs cleanup and no async thread is blocked.
    pub(crate) fn try_begin_review(
        &self,
        binding: &crate::authorization::PreparedAuthorizationBinding,
        work: &crate::authorization::WorkAuthorizationContext,
    ) -> Result<ReviewAttemptHandle, ReservedReviewError<std::convert::Infallible>> {
        self.ensure_available()
            .map_err(|_| ReservedReviewError::Owner(ReviewOwnerError::Unavailable))?;
        let id: Arc<str> = Arc::from(crate::time_compat::new_uuid_v7().to_string());
        let Some(mut state) = self.state.try_write() else {
            return Err(ReservedReviewError::Contended);
        };
        self.count_review_lock();
        let status = review_status_from_outcome(state.authority.begin_review(id.to_string()));
        self.release_state(state).run();
        match status {
            Ok(ReviewAttemptStatus::Pending) => {
                Ok(ReviewAttemptHandle::new(id, binding.clone(), work.clone()))
            }
            Ok(_) => Err(ReservedReviewError::Owner(ReviewOwnerError::Unavailable)),
            Err(error) => Err(ReservedReviewError::Owner(error)),
        }
    }

    /// Owner-transition tests: begin on an uncontended owner.
    #[cfg(test)]
    pub(crate) fn begin_review(
        &self,
        binding: &crate::authorization::PreparedAuthorizationBinding,
        work: &crate::authorization::WorkAuthorizationContext,
    ) -> Result<ReviewAttemptHandle, ReviewOwnerError> {
        self.try_begin_review(binding, work)
            .map_err(|error| match error {
                ReservedReviewError::Owner(error) => error,
                _ => ReviewOwnerError::Unavailable,
            })
    }

    /// Accept a verdict under the single commit reservation, after
    /// `final_check`; a refusal retires and disposes the attempt there.
    pub(crate) fn record_review_verdict<E>(
        &self,
        handle: &ReviewAttemptHandle,
        verdict: ReviewVerdict,
        final_check: impl FnOnce() -> Result<(), E>,
        retirement: impl FnOnce(&ReservedReviewError<E>) -> ReviewRetirementReason,
        on_queued: impl FnOnce() -> QueuedDisposalReport,
    ) -> Result<(ReviewAttemptStatus, SettledReports), RefusedReviewCommit<E>> {
        self.commit_review_reserved(
            handle,
            None,
            final_check,
            |authority| authority.record_review_verdict(handle.id().to_owned(), verdict),
            false,
            retirement,
            on_queued,
        )
    }

    /// Record a reviewer failure under the non-blocking commit reservation;
    /// this is a refusal path, so it never waits on the owner.
    pub(crate) fn record_review_unavailable(
        &self,
        handle: &ReviewAttemptHandle,
        retirement: impl FnOnce(
            &ReservedReviewError<std::convert::Infallible>,
        ) -> ReviewRetirementReason,
        on_queued: impl FnOnce() -> QueuedDisposalReport,
    ) -> Result<(ReviewAttemptStatus, SettledReports), RefusedReviewCommit<std::convert::Infallible>>
    {
        self.commit_review_reserved(
            handle,
            None,
            || Ok(()),
            |authority| authority.record_review_unavailable(handle.id().to_owned()),
            false,
            retirement,
            on_queued,
        )
    }

    #[cfg(test)]
    pub(crate) fn retire_review(
        &self,
        handle: &ReviewAttemptHandle,
        reason: ReviewRetirementReason,
    ) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
        self.apply_review(|authority| authority.retire_review(handle.id().to_owned(), reason))
    }

    /// Spend the allow once, only for the exact operation binding and work
    /// association the attempt was issued for, and dispose the attempt, all
    /// under the single commit reservation after `final_check`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn consume_review_for_entry<E>(
        &self,
        handle: &ReviewAttemptHandle,
        check: &crate::authorization::PreparedOperationCheck,
        work: &crate::authorization::WorkAuthorizationContext,
        final_check: impl FnOnce() -> Result<(), E>,
        retirement: impl FnOnce(&ReservedReviewError<E>) -> ReviewRetirementReason,
        on_queued: impl FnOnce() -> QueuedDisposalReport,
    ) -> Result<(ReviewAttemptStatus, SettledReports), RefusedReviewCommit<E>> {
        self.commit_review_reserved(
            handle,
            Some((check, work)),
            final_check,
            |authority| authority.consume_review_for_entry(handle.id().to_owned()),
            true,
            retirement,
            on_queued,
        )
    }

    /// Dispose a settled attempt. A pending attempt must be retired first.
    #[cfg(test)]
    pub(crate) fn release_review(
        &self,
        handle: ReviewAttemptHandle,
    ) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
        self.apply_review(|authority| authority.release_review(handle.id().to_owned()))
    }

    fn refresh_expiry(&self, approval_id: &ApprovalId) -> Result<(), ApprovalError> {
        let now = Utc::now();
        let mut state = self.state.write();
        let result = self.refresh_expiry_locked(&mut state, approval_id, now);
        // An expiry put is an owner commit too: settle queued disposals.
        self.release_state(state).run();
        result
    }

    fn refresh_expiry_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
        now: DateTime<Utc>,
    ) -> Result<(), ApprovalError> {
        let Some(record) = state.records.get(approval_id).cloned() else {
            return Ok(());
        };
        let expired = record
            .expires_at
            .is_some_and(|expires_at| expires_at <= now);
        let mut authority = state.authority.clone();
        let outcome = authority
            .observe_approval_expiry(approval_id.to_string(), expired)
            .map_err(lifecycle_error)?;
        let status = lifecycle_status_from_outcome(approval_id, outcome, None)?;
        if status == record.status {
            state.authority = authority;
        } else {
            let mut expired_record = record;
            expired_record.status = status;
            expired_record.updated_at = now;
            self.store.put(&expired_record)?;
            state.authority = authority;
            state.records.insert(approval_id.clone(), expired_record);
        }
        Ok(())
    }

    fn refresh_all_expiry(&self) -> Result<(), ApprovalError> {
        let ids = self
            .state
            .read()
            .records
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        self.settle_queued_disposals_now().run();
        for id in ids {
            self.refresh_expiry(&id)?;
        }
        Ok(())
    }
}

/// Reports a queued review disposal's settled result once the owner settles
/// it (on its next commit). Runs after the state lock is released.
pub(crate) type QueuedDisposalReport =
    Box<dyn FnOnce(Result<ReviewAttemptStatus, ReviewOwnerError>) + Send>;

type SettledReviewDisposal = (
    QueuedDisposalReport,
    Result<ReviewAttemptStatus, ReviewOwnerError>,
);

/// A review attempt disposal handed to the owner without waiting for it.
struct QueuedReviewDisposal {
    id: String,
    retire: Option<ReviewRetirementReason>,
    report: QueuedDisposalReport,
}

/// A refused reserved review commit: the typed refusal and the attempt's
/// disposal, settled under the same reservation, or `None` when queued.
pub(crate) struct RefusedReviewCommit<E> {
    pub(crate) error: ReservedReviewError<E>,
    pub(crate) disposal: Option<Result<ReviewAttemptStatus, ReviewOwnerError>>,
    /// Reports of unrelated disposals settled along the way; the caller
    /// runs them only after releasing its own locks.
    pub(crate) reports: SettledReports,
}

/// Retire (when set) and release one attempt under a reservation already
/// held. Release is best-effort cleanup of memory-only state; the reported
/// result is the retirement's when there is one.
fn dispose_locked(
    authority: &mut ApprovalLifecycleMachineAuthority,
    id: &str,
    retire: Option<ReviewRetirementReason>,
) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
    let retired = retire
        .map(|reason| review_status_from_outcome(authority.retire_review(id.to_owned(), reason)));
    let released = review_status_from_outcome(authority.release_review(id.to_owned()));
    retired.unwrap_or(released)
}

/// Settled review disposal reports, run outside every lock. Dropping runs
/// any report not yet run, so none is lost.
#[must_use = "run the reports once outside every lock"]
#[derive(Default)]
pub(crate) struct SettledReports(Vec<SettledReviewDisposal>);

impl SettledReports {
    pub(crate) fn run(self) {
        drop(self);
    }

    /// Carry `other`'s reports with these, to run together later.
    pub(crate) fn absorb(&mut self, mut other: SettledReports) {
        self.0.append(&mut other.0);
    }
}

impl Drop for SettledReports {
    fn drop(&mut self) {
        for (report, result) in self.0.drain(..) {
            report(result);
        }
    }
}

impl Default for ApprovalService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::Duration;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct TestApprovalStore {
        records: RwLock<BTreeMap<ApprovalId, ApprovalRecord>>,
        put_calls: AtomicUsize,
        fail_on_put_call: Option<usize>,
    }

    impl TestApprovalStore {
        fn new(fail_on_put_call: Option<usize>) -> Self {
            Self {
                records: RwLock::new(BTreeMap::new()),
                put_calls: AtomicUsize::new(0),
                fail_on_put_call,
            }
        }

        fn record(&self, approval_id: &ApprovalId) -> Option<ApprovalRecord> {
            self.records.read().get(approval_id).cloned()
        }
    }

    impl ApprovalStore for TestApprovalStore {
        fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
            Ok(self.records.read().values().cloned().collect())
        }

        fn put(&self, record: &ApprovalRecord) -> Result<(), ApprovalStoreError> {
            let put_call = self.put_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if self
                .fail_on_put_call
                .is_some_and(|fail_on_put_call| fail_on_put_call == put_call)
            {
                return Err(ApprovalStoreError::Backend(
                    "injected approval store failure".to_string(),
                ));
            }
            self.records
                .write()
                .insert(record.approval_id.clone(), record.clone());
            Ok(())
        }

        fn is_persistent(&self) -> bool {
            true
        }
    }

    fn principal(value: &str) -> ApprovalPrincipalId {
        ApprovalPrincipalId::new(value).expect("valid principal")
    }

    fn request_with_allowed(allowed_decisions: BTreeSet<ApprovalDecision>) -> ApprovalRequest {
        ApprovalRequest {
            requester: principal("human:alice"),
            owner: ApprovalOwnerRef::Session {
                session_id: SessionId::new(),
            },
            resource: ApprovalResourceRef {
                kind: ApprovalResourceKind::ShellCommand,
                id: ApprovalResourceId::new("shell:rm"),
            },
            proposed_action: ApprovalProposedAction {
                kind: ApprovalActionKind::ShellCommand,
                summary: "run destructive command".to_string(),
                body: Some(json!({"cmd": "rm -rf target/tmp"})),
            },
            risk: ApprovalRisk::High,
            request_body: Some(json!({"why": "cleanup"})),
            allowed_decisions,
            expires_at: None,
            metadata: SurfaceMetadata::default(),
            request_provenance: Some(json!({"tool_call_id": "call-1"})),
        }
    }

    fn request() -> ApprovalRequest {
        request_with_allowed(BTreeSet::from([
            ApprovalDecision::Approve,
            ApprovalDecision::Deny,
        ]))
    }

    #[test]
    fn approval_request_creates_pending_auditable_record() {
        let service = ApprovalService::new();
        let record = service.request(request()).expect("request accepted");
        assert_eq!(record.status, ApprovalStatus::Pending);
        assert_eq!(record.requester.as_str(), "human:alice");
        assert_eq!(
            record.request_provenance,
            Some(json!({"tool_call_id": "call-1"}))
        );
        assert!(record.decision.is_none());
    }

    #[test]
    fn decide_preserves_request_provenance_and_records_decision_audit() {
        let service = ApprovalService::new();
        let record = service.request(request()).expect("request accepted");
        let decided = service
            .decide(
                &record.approval_id,
                ApprovalDecision::Approve,
                principal("human:bob"),
                Some("looks intentional".to_string()),
                Some(json!({"client": "mobile"})),
            )
            .expect("decision accepted");

        assert_eq!(decided.status, ApprovalStatus::Approved);
        assert_eq!(decided.request_provenance, record.request_provenance);
        let decision = decided.decision.expect("decision audit");
        assert_eq!(decision.actor.as_str(), "human:bob");
        assert_eq!(decision.provenance, Some(json!({"client": "mobile"})));
    }

    #[test]
    fn invalid_decision_is_rejected() {
        let service = ApprovalService::new();
        let record = service
            .request(request_with_allowed(BTreeSet::from([
                ApprovalDecision::Deny,
            ])))
            .expect("request accepted");
        let err = service
            .decide(
                &record.approval_id,
                ApprovalDecision::Approve,
                principal("human:bob"),
                None,
                None,
            )
            .expect_err("approval should reject disallowed decision");
        assert!(matches!(
            err,
            ApprovalError::InvalidDecision {
                decision: ApprovalDecision::Approve
            }
        ));
    }

    #[test]
    fn empty_allowed_decisions_are_rejected_by_generated_authority() {
        let service = ApprovalService::new();
        let err = service
            .request(request_with_allowed(BTreeSet::new()))
            .expect_err("empty allowed decisions rejected");
        assert!(matches!(err, ApprovalError::EmptyAllowedDecisions));
    }

    #[test]
    fn duplicate_decision_is_rejected() {
        let service = ApprovalService::new();
        let record = service.request(request()).expect("request accepted");
        service
            .decide(
                &record.approval_id,
                ApprovalDecision::Deny,
                principal("human:bob"),
                None,
                None,
            )
            .expect("first decision accepted");
        let err = service
            .decide(
                &record.approval_id,
                ApprovalDecision::Deny,
                principal("human:bob"),
                None,
                None,
            )
            .expect_err("duplicate rejected");
        assert!(matches!(err, ApprovalError::AlreadyDecided { .. }));
    }

    #[test]
    fn failed_decision_persist_keeps_approval_pending_for_retry() {
        let store = Arc::new(TestApprovalStore::new(Some(2)));
        let service = ApprovalService::with_store(store.clone()).expect("service");
        let record = service.request(request()).expect("request accepted");

        let err = service
            .decide(
                &record.approval_id,
                ApprovalDecision::Approve,
                principal("human:bob"),
                None,
                None,
            )
            .expect_err("decision write should fail");

        assert!(matches!(err, ApprovalError::Store(_)));
        let cached = service.get(&record.approval_id).expect("cached record");
        assert_eq!(cached.status, ApprovalStatus::Pending);
        assert!(cached.decision.is_none());
        let persisted = store.record(&record.approval_id).expect("persisted record");
        assert_eq!(persisted.status, ApprovalStatus::Pending);
        assert!(persisted.decision.is_none());

        let retried = service
            .decide(
                &record.approval_id,
                ApprovalDecision::Deny,
                principal("human:bob"),
                Some("changed my mind".to_string()),
                None,
            )
            .expect("retry should decide approval");
        assert_eq!(retried.status, ApprovalStatus::Denied);
    }

    #[test]
    fn restore_rejects_inconsistent_persisted_status() {
        let store = Arc::new(TestApprovalStore::new(None));
        let mut record = ApprovalService::new()
            .request(request())
            .expect("request accepted");
        record.status = ApprovalStatus::Approved;
        record.updated_at = Utc::now();
        store
            .records
            .write()
            .insert(record.approval_id.clone(), record);

        let restored = ApprovalService::with_store(store);
        assert!(matches!(restored, Err(ApprovalError::Store(_))));
    }

    #[test]
    fn unavailable_approval_service_fails_closed() {
        let service = ApprovalService::unavailable("persistent approval restore failed");
        let err = service
            .list(ApprovalListFilter::default())
            .expect_err("unavailable service should reject reads");
        assert!(matches!(
            err,
            ApprovalError::Store(message) if message.contains("persistent approval restore failed")
        ));
        let err = service
            .request(request())
            .expect_err("unavailable service should reject writes");
        assert!(matches!(
            err,
            ApprovalError::Store(message) if message.contains("persistent approval restore failed")
        ));
    }

    #[test]
    fn expired_approval_cannot_be_decided() {
        let service = ApprovalService::new();
        let mut request = request();
        request.expires_at = Some(Utc::now() - Duration::seconds(1));
        let record = service.request(request).expect("request accepted");
        let err = service
            .decide(
                &record.approval_id,
                ApprovalDecision::Approve,
                principal("human:bob"),
                None,
                None,
            )
            .expect_err("expired approval rejected");
        assert!(matches!(err, ApprovalError::Expired { .. }));
        assert_eq!(
            service.get(&record.approval_id).expect("record").status,
            ApprovalStatus::Expired
        );
    }

    #[test]
    fn deciding_expired_approval_persists_expiry_transition() {
        let store = Arc::new(TestApprovalStore::new(None));
        let service = ApprovalService::with_store(store.clone()).expect("service");
        let mut request = request();
        request.expires_at = Some(Utc::now() - Duration::seconds(1));
        let record = service.request(request).expect("request accepted");

        let err = service
            .decide(
                &record.approval_id,
                ApprovalDecision::Approve,
                principal("human:bob"),
                None,
                None,
            )
            .expect_err("expired approval rejected");

        assert!(matches!(err, ApprovalError::Expired { .. }));
        let persisted = store.record(&record.approval_id).expect("persisted record");
        assert_eq!(persisted.status, ApprovalStatus::Expired);
        assert!(persisted.decision.is_none());
    }

    #[test]
    fn nonexistent_approval_cannot_be_decided() {
        let service = ApprovalService::new();
        let err = service
            .decide(
                &ApprovalId::new(),
                ApprovalDecision::Approve,
                principal("human:bob"),
                None,
                None,
            )
            .expect_err("unknown approval rejected");
        assert!(matches!(err, ApprovalError::NotFound { .. }));
    }

    #[test]
    fn reserved_metadata_spoofing_is_rejected() {
        let service = ApprovalService::new();
        let mut request = request();
        request
            .metadata
            .labels
            .insert("meerkat.approval_id".to_string(), "spoof".to_string());
        let err = service
            .request(request)
            .expect_err("reserved metadata rejected");
        assert!(matches!(
            err,
            ApprovalError::InvalidMetadata(crate::SurfaceMetadataError::ReservedLabelKey { .. })
        ));
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod review_owner_tests {
    //! Generated review attempt ownership: identity binding, single use,
    //! retirement ordering and memory-only state.

    use super::*;
    use crate::authorization::{
        AuthorizationOperation, OperationAuthorizationError, OperationAuthorizationFacts,
        PreparedAuthorizationBinding, PreparedOperationAuthorization, PreparedOperationCheck,
        SourceAuthorizationFacts, SourceAuthorizationTarget, SourceAuthorizationUse,
        WorkAuthorization, WorkAuthorizationContext,
    };
    use crate::exact_operation::OperationExecutionScope;
    use crate::memory::MemorySearchScope;
    use crate::ops::OperationId;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Raw generated owner transitions, without the reserved-commit
    /// settlement policy (which retires refused commits); these tests pin
    /// the transitions themselves. The binding check is the production one.
    fn record(
        approvals: &ApprovalService,
        handle: &ReviewAttemptHandle,
        verdict: ReviewVerdict,
    ) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
        approvals.apply_review(|authority| {
            authority.record_review_verdict(handle.id().to_owned(), verdict)
        })
    }

    fn spend(
        approvals: &ApprovalService,
        handle: &ReviewAttemptHandle,
        check: &PreparedOperationCheck,
        work: &WorkAuthorizationContext,
    ) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
        if !handle.bound_to(check.binding(), work) {
            return Err(ReviewOwnerError::Mismatch);
        }
        approvals
            .apply_review(|authority| authority.consume_review_for_entry(handle.id().to_owned()))
    }

    fn binding() -> PreparedAuthorizationBinding {
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: OperationId(Uuid::nil()),
            execution_scope: OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::Memory(MemorySearchScope::for_session(
                    SessionId::from_uuid(Uuid::nil()),
                )),
                usage: SourceAuthorizationUse::Read,
            }),
        })
    }

    struct Allow;

    impl PreparedOperationAuthorization for Allow {
        fn review_tier(&self) -> crate::authorization::OperationReviewTier {
            crate::authorization::OperationReviewTier::R1
        }

        fn check_current(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<(), OperationAuthorizationError> {
            Ok(())
        }
    }

    struct AllowWork;

    impl WorkAuthorization for AllowWork {
        fn prepare(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            Ok(Arc::new(Allow))
        }
    }

    fn work() -> WorkAuthorizationContext {
        WorkAuthorizationContext::new(Arc::new(AllowWork), OperationExecutionScope::Domain)
    }

    fn check(
        work: &WorkAuthorizationContext,
        binding: &PreparedAuthorizationBinding,
    ) -> PreparedOperationCheck {
        PreparedOperationCheck::prepare(work.clone(), binding.clone()).expect("prepared")
    }

    #[test]
    fn late_verdict_after_retirement_is_rejected_and_cannot_enter() {
        let approvals = ApprovalService::new();
        let (work, binding) = (work(), binding());
        let handle = approvals.begin_review(&binding, &work).expect("attempt");
        assert_eq!(
            approvals.retire_review(&handle, ReviewRetirementReason::DeadlineExpired),
            Ok(ReviewAttemptStatus::Retired)
        );
        assert_eq!(
            record(&approvals, &handle, ReviewVerdict::Allow),
            Err(ReviewOwnerError::Rejected(
                ApprovalLifecycleRejectionReason::ReviewRetired
            ))
        );
        assert_eq!(
            spend(&approvals, &handle, &check(&work, &binding), &work),
            Err(ReviewOwnerError::Rejected(
                ApprovalLifecycleRejectionReason::ReviewRetired
            ))
        );
    }

    #[test]
    fn allow_is_spent_once_and_used_is_never_relabelled() {
        let approvals = ApprovalService::new();
        let (work, binding) = (work(), binding());
        let entering = check(&work, &binding);
        let handle = approvals.begin_review(&binding, &work).expect("attempt");
        assert_eq!(
            spend(&approvals, &handle, &entering, &work),
            Err(ReviewOwnerError::Rejected(
                ApprovalLifecycleRejectionReason::ReviewNotSatisfied
            )),
            "a pending review cannot be spent"
        );
        assert_eq!(
            record(&approvals, &handle, ReviewVerdict::Allow),
            Ok(ReviewAttemptStatus::Allowed)
        );
        assert_eq!(
            spend(&approvals, &handle, &entering, &work),
            Ok(ReviewAttemptStatus::Used)
        );
        assert_eq!(
            spend(&approvals, &handle, &entering, &work),
            Err(ReviewOwnerError::Rejected(
                ApprovalLifecycleRejectionReason::AlreadyDecided
            ))
        );
        assert_eq!(
            approvals.retire_review(&handle, ReviewRetirementReason::Abandoned),
            Err(ReviewOwnerError::Rejected(
                ApprovalLifecycleRejectionReason::AlreadyDecided
            )),
            "a spent allow is never relabelled as retired"
        );
    }

    #[test]
    fn denied_or_escalated_review_cannot_be_retired_or_spent() {
        for (verdict, settled) in [
            (ReviewVerdict::Deny, ReviewAttemptStatus::Denied),
            (ReviewVerdict::Escalate, ReviewAttemptStatus::Escalated),
        ] {
            let approvals = ApprovalService::new();
            let (work, binding) = (work(), binding());
            let handle = approvals.begin_review(&binding, &work).expect("attempt");
            assert_eq!(record(&approvals, &handle, verdict), Ok(settled));
            assert!(
                approvals
                    .retire_review(&handle, ReviewRetirementReason::ContextChanged)
                    .is_err()
            );
            assert!(spend(&approvals, &handle, &check(&work, &binding), &work).is_err());
        }
    }

    #[test]
    fn equal_facts_or_another_work_association_cannot_spend_an_allow() {
        let approvals = ApprovalService::new();
        let (work, binding) = (work(), binding());
        let handle = approvals.begin_review(&binding, &work).expect("attempt");
        record(&approvals, &handle, ReviewVerdict::Allow).expect("allowed");
        let equal_facts = PreparedAuthorizationBinding::new(binding.facts().clone());
        let other_work = self::work();
        assert_eq!(
            spend(&approvals, &handle, &check(&work, &equal_facts), &work),
            Err(ReviewOwnerError::Mismatch)
        );
        assert_eq!(
            spend(
                &approvals,
                &handle,
                &check(&other_work, &binding),
                &other_work
            ),
            Err(ReviewOwnerError::Mismatch)
        );
        assert_eq!(
            spend(&approvals, &handle, &check(&work, &binding), &work),
            Ok(ReviewAttemptStatus::Used),
            "the exact operation and work still enter once"
        );
    }

    #[test]
    fn attempts_are_fresh_and_released_attempts_are_unknown() {
        let approvals = ApprovalService::new();
        let (work, binding) = (work(), binding());
        let first = approvals.begin_review(&binding, &work).expect("attempt");
        let second = approvals.begin_review(&binding, &work).expect("attempt");
        assert_ne!(first.id(), second.id(), "attempt ids are never reused");
        assert_eq!(
            approvals.release_review(first),
            Err(ReviewOwnerError::Rejected(
                ApprovalLifecycleRejectionReason::ReviewPending
            )),
            "a pending attempt must be retired before disposal"
        );
        let first = approvals.begin_review(&binding, &work).expect("attempt");
        approvals
            .apply_review(|authority| authority.record_review_unavailable(first.id().to_owned()))
            .expect("unavailable");
        let id = first.id().to_owned();
        assert_eq!(
            approvals.release_review(first),
            Ok(ReviewAttemptStatus::Unavailable)
        );
        let copied = ReviewAttemptHandle::new(Arc::from(id), binding, work);
        assert_eq!(
            record(&approvals, &copied, ReviewVerdict::Allow),
            Err(ReviewOwnerError::Rejected(
                ApprovalLifecycleRejectionReason::NotFound
            )),
            "a disposed attempt cannot be revived by its id"
        );
    }

    #[derive(Default)]
    struct CountingStore(AtomicUsize);

    impl ApprovalStore for CountingStore {
        fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
            Ok(Vec::new())
        }

        fn put(&self, _record: &ApprovalRecord) -> Result<(), ApprovalStoreError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn is_persistent(&self) -> bool {
            true
        }
    }

    #[test]
    fn review_attempts_never_reach_the_approval_store() {
        let store = Arc::new(CountingStore::default());
        let approvals = ApprovalService::with_store(store.clone()).expect("service");
        let (work, binding) = (work(), binding());
        let handle = approvals.begin_review(&binding, &work).expect("attempt");
        record(&approvals, &handle, ReviewVerdict::Allow).expect("allowed");
        spend(&approvals, &handle, &check(&work, &binding), &work).expect("used");
        approvals.release_review(handle).expect("released");
        assert_eq!(store.0.load(Ordering::SeqCst), 0);
        // A reopened owner over the same store knows no review attempt.
        let reopened = ApprovalService::with_store(store).expect("reopened");
        assert!(
            reopened
                .list(ApprovalListFilter::default())
                .expect("list")
                .is_empty()
        );
    }

    #[test]
    fn unavailable_owner_refuses_new_attempts() {
        let approvals = ApprovalService::unavailable("fixture");
        assert_eq!(
            approvals.begin_review(&binding(), &work()).err(),
            Some(ReviewOwnerError::Unavailable)
        );
    }
}
