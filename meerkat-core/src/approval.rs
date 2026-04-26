//! Durable approval records and service contracts.
//!
//! The approval service owns approval record state for this first slice. Public
//! surfaces may request, list, read, and decide approvals, but they do not own
//! approval status transitions.

use crate::SurfaceMetadata;
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
/// The service owns approval transitions; stores persist full records and do
/// not decide status legality.
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

/// In-process approval service.
#[derive(Clone)]
pub struct ApprovalService {
    records: Arc<RwLock<BTreeMap<ApprovalId, ApprovalRecord>>>,
    store: Arc<dyn ApprovalStore>,
}

impl ApprovalService {
    #[must_use]
    pub fn new() -> Self {
        Self {
            records: Arc::new(RwLock::new(BTreeMap::new())),
            store: Arc::new(InMemoryApprovalStore::new()),
        }
    }

    pub fn with_store(store: Arc<dyn ApprovalStore>) -> Result<Self, ApprovalError> {
        let records = store
            .load_all()?
            .into_iter()
            .map(|record| (record.approval_id.clone(), record))
            .collect();
        Ok(Self {
            records: Arc::new(RwLock::new(records)),
            store,
        })
    }

    #[must_use]
    pub fn is_persistent(&self) -> bool {
        self.store.is_persistent()
    }

    pub fn request(&self, request: ApprovalRequest) -> Result<ApprovalRecord, ApprovalError> {
        if request.allowed_decisions.is_empty() {
            return Err(ApprovalError::EmptyAllowedDecisions);
        }
        if request.requester.as_str().trim().is_empty() {
            return Err(ApprovalError::InvalidPrincipal);
        }
        request.metadata.validate_public()?;
        let now = Utc::now();
        let record = ApprovalRecord {
            approval_id: ApprovalId::new(),
            status: ApprovalStatus::Pending,
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
        self.records
            .write()
            .insert(record.approval_id.clone(), record.clone());
        Ok(record)
    }

    pub fn get(&self, approval_id: &ApprovalId) -> Result<ApprovalRecord, ApprovalError> {
        self.refresh_expiry(approval_id)?;
        self.records
            .read()
            .get(approval_id)
            .cloned()
            .ok_or_else(|| ApprovalError::NotFound {
                approval_id: approval_id.clone(),
            })
    }

    pub fn list(&self, filter: ApprovalListFilter) -> Result<Vec<ApprovalRecord>, ApprovalError> {
        self.refresh_all_expiry()?;
        Ok(self
            .records
            .read()
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
        if actor.as_str().trim().is_empty() {
            return Err(ApprovalError::InvalidPrincipal);
        }
        let now = Utc::now();
        let mut records = self.records.write();
        let record = records
            .get(approval_id)
            .cloned()
            .ok_or_else(|| ApprovalError::NotFound {
                approval_id: approval_id.clone(),
            })?;

        if record.status == ApprovalStatus::Pending
            && record
                .expires_at
                .is_some_and(|expires_at| expires_at <= now)
        {
            let mut expired_record = record;
            expired_record.status = ApprovalStatus::Expired;
            expired_record.updated_at = now;
            self.store.put(&expired_record)?;
            records.insert(approval_id.clone(), expired_record);
            return Err(ApprovalError::Expired {
                approval_id: approval_id.clone(),
            });
        }

        match record.status {
            ApprovalStatus::Pending => {}
            ApprovalStatus::Expired => {
                return Err(ApprovalError::Expired {
                    approval_id: approval_id.clone(),
                });
            }
            ApprovalStatus::Approved | ApprovalStatus::Denied | ApprovalStatus::Cancelled => {
                return Err(ApprovalError::AlreadyDecided {
                    approval_id: approval_id.clone(),
                });
            }
        }

        if !record.allowed_decisions.contains(&decision) {
            return Err(ApprovalError::InvalidDecision { decision });
        }

        let mut decided_record = record;
        decided_record.status = match decision {
            ApprovalDecision::Approve => ApprovalStatus::Approved,
            ApprovalDecision::Deny => ApprovalStatus::Denied,
        };
        decided_record.updated_at = now;
        decided_record.decision = Some(ApprovalDecisionRecord {
            decision,
            actor,
            decided_at: now,
            reason,
            provenance,
        });
        self.store.put(&decided_record)?;
        records.insert(approval_id.clone(), decided_record.clone());
        Ok(decided_record)
    }

    fn refresh_expiry(&self, approval_id: &ApprovalId) -> Result<(), ApprovalError> {
        let now = Utc::now();
        let mut records = self.records.write();
        if let Some(record) = records.get(approval_id).cloned()
            && record.status == ApprovalStatus::Pending
            && record
                .expires_at
                .is_some_and(|expires_at| expires_at <= now)
        {
            let mut expired_record = record;
            expired_record.status = ApprovalStatus::Expired;
            expired_record.updated_at = now;
            self.store.put(&expired_record)?;
            records.insert(approval_id.clone(), expired_record);
        }
        Ok(())
    }

    fn refresh_all_expiry(&self) -> Result<(), ApprovalError> {
        let ids = self.records.read().keys().cloned().collect::<Vec<_>>();
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
