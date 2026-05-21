//! Durable approval records and service contracts.
//!
//! Generated approval lifecycle authority owns approval status transitions.
//! Public surfaces may request, list, read, and decide approvals; the service
//! stores and projects the generated lifecycle decisions.

use crate::SurfaceMetadata;
use crate::generated::approval_lifecycle::{
    ApprovalLifecycleDecision, ApprovalLifecycleMachineAuthority, ApprovalLifecycleOutcome,
    ApprovalLifecycleRejectionReason, ApprovalLifecycleStatus,
};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
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

/// Typed owner for an approval request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "owner_type", rename_all = "snake_case")]
pub enum ApprovalOwnerRef {
    Runtime,
    Session { session_id: String },
    Mob { mob_id: String },
    Run { run_id: String },
    ToolCall { tool_call_id: String },
    ExternalMember { mob_id: String, member_ref: String },
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
    pub id: String,
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
                    "generated approval lifecycle rejected {} with InvalidDecision but no decision context",
                    approval_id
                ));
            };
            ApprovalError::InvalidDecision { decision }
        }
        ApprovalLifecycleRejectionReason::EmptyAllowedDecisions => {
            ApprovalError::EmptyAllowedDecisions
        }
        ApprovalLifecycleRejectionReason::AlreadyExists
        | ApprovalLifecycleRejectionReason::InvalidRestoredRecord => ApprovalError::Store(format!(
            "generated approval lifecycle authority rejected {} with {:?}",
            approval_id, reason
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
}

impl ApprovalService {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(ApprovalServiceState::empty())),
            store: Arc::new(InMemoryApprovalStore::new()),
            unavailable_reason: None,
        }
    }

    pub fn with_store(store: Arc<dyn ApprovalStore>) -> Result<Self, ApprovalError> {
        let state = ApprovalServiceState::from_records(store.load_all()?)?;
        Ok(Self {
            state: Arc::new(RwLock::new(state)),
            store,
            unavailable_reason: None,
        })
    }

    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            state: Arc::new(RwLock::new(ApprovalServiceState::empty())),
            store: Arc::new(InMemoryApprovalStore::new()),
            unavailable_reason: Some(Arc::from(reason.into())),
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
        let (approve_allowed, deny_allowed) = allowed_decision_flags(&request.allowed_decisions);
        let mut state = self.state.write();
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
        self.state
            .read()
            .records
            .get(approval_id)
            .cloned()
            .ok_or_else(|| ApprovalError::NotFound {
                approval_id: approval_id.clone(),
            })
    }

    pub fn list(&self, filter: ApprovalListFilter) -> Result<Vec<ApprovalRecord>, ApprovalError> {
        self.ensure_available()?;
        self.refresh_all_expiry()?;
        Ok(self
            .state
            .read()
            .records
            .values()
            .filter(|record| filter.status.is_none_or(|status| record.status == status))
            .cloned()
            .collect())
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
                    "generated approval lifecycle emitted {:?} for missing approval {}",
                    status, approval_id
                ))),
            };
        }

        self.refresh_expiry_locked(&mut state, approval_id, now)?;
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

    fn refresh_expiry(&self, approval_id: &ApprovalId) -> Result<(), ApprovalError> {
        let now = Utc::now();
        let mut state = self.state.write();
        self.refresh_expiry_locked(&mut state, approval_id, now)
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
        if status != record.status {
            let mut expired_record = record;
            expired_record.status = status;
            expired_record.updated_at = now;
            self.store.put(&expired_record)?;
            state.authority = authority;
            state.records.insert(approval_id.clone(), expired_record);
        } else {
            state.authority = authority;
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
        for id in ids {
            self.refresh_expiry(&id)?;
        }
        Ok(())
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
                session_id: "session-1".to_string(),
            },
            resource: ApprovalResourceRef {
                kind: ApprovalResourceKind::ShellCommand,
                id: "shell:rm".to_string(),
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
