//! Host-injected persistence for Toolkit-owned facts.
//!
//! This crate owns atomic storage operations, not setup decisions, permissions,
//! credentials, readiness, or work scheduling. Domain owners validate transitions
//! and persist versioned domain records. Credentials belong in a credential
//! store; this store holds opaque references and redacted decision evidence.
//!
//! Every required method is explicit. Capability declarations describe the
//! backend; conformance against the deployed backend is still required. SQLite
//! provides local, durable, cross-process fencing on one supported filesystem.
//! It does not claim distributed fencing across independent database copies.

mod conditional;
mod sqlite;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;
use thiserror::Error;

pub use conditional::{ConditionalAdmission, ConditionalAdmissionStore, ConditionalTransition, RecordCondition};
pub use sqlite::SqliteStore;

/// Mechanical invalidation supplied by the actual installed record owner.
/// This does not interpret records or authorize mutations. An installed host
/// may couple its exact SQLite instance to an existing publication so a view
/// observed before an async read cannot survive a later record write.
pub trait RecordChangePublication: Send + Sync {
    fn begin_record_change(&self) -> StoreResult<Box<dyn RecordChangeGuard + '_>>;
}

/// Retained until the actual transaction commits or rolls back. It contains no
/// record data and must not perform a store read on drop.
pub trait RecordChangeGuard {}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("the physical store owner is unavailable")]
    OwnerUnavailable,
    #[error("invalid store request: {0}")]
    Invalid(String),
    #[error("record does not exist")]
    NotFound,
    #[error("record already exists")]
    AlreadyExists,
    #[error("admission key was previously used for different content")]
    AdmissionConflict,
    #[error("revision or expected payload does not match")]
    Conflict,
    #[error("conditional admission deadline expired")]
    AdmissionExpired,
    #[error("an effect or its idempotency key already exists")]
    EffectConflict,
    #[error("another executor holds the claim")]
    Claimed,
    #[error("claim is stale, expired, or from another store")]
    Fenced,
    #[error("effect is terminal or not in the requested dispatch state")]
    EffectState,
    #[error("unsupported store format version: {0}")]
    UnsupportedFormat(i64),
    #[error("store synchronization failed")]
    Synchronization,
    #[error("store worker did not complete")]
    Worker,
    #[error("storage failure: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("invalid stored encoding")]
    Encoding(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Durability {
    Volatile,
    Durable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreCapabilities {
    pub durability: Durability,
    pub revision_cas: bool,
    pub unique_admission: bool,
    pub atomic_record_events_effects: bool,
    pub cross_process_fencing: bool,
    pub store_clock: bool,
}

impl StoreCapabilities {
    /// Reject unsupported required capabilities at feature composition time.
    /// This declaration is not a replacement for backend conformance tests.
    pub fn require_durable_contract(&self) -> StoreResult<()> {
        if self.durability != Durability::Durable
            || !self.revision_cas
            || !self.unique_admission
            || !self.atomic_record_events_effects
            || !self.cross_process_fencing
            || !self.store_clock
        {
            return Err(StoreError::Invalid(
                "backend does not declare the required durable storage contract".into(),
            ));
        }
        Ok(())
    }
}

/// A namespace is supplied by the authenticated host, not trusted tool input.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StoreScope {
    pub realm: String,
    pub owner: String,
}

impl StoreScope {
    pub fn validate(&self) -> StoreResult<()> {
        identifier(&self.realm)?;
        identifier(&self.owner)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StoreKey {
    pub realm: String,
    pub owner: String,
    pub id: String,
}

impl StoreKey {
    pub fn scope(&self) -> StoreScope {
        StoreScope {
            realm: self.realm.clone(),
            owner: self.owner.clone(),
        }
    }

    pub fn validate(&self) -> StoreResult<()> {
        self.scope().validate()?;
        identifier(&self.id)
    }
}

/// The domain owner supplies its schema and version. This storage envelope does
/// not turn arbitrary JSON into a valid setup decision or credential record.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordData {
    pub schema: String,
    pub version: u32,
    pub body: Value,
}

impl fmt::Debug for RecordData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordData")
            .field("schema", &self.schema)
            .field("version", &self.version)
            .field("body", &"[redacted]")
            .finish()
    }
}

impl RecordData {
    pub fn validate(&self) -> StoreResult<()> {
        identifier(&self.schema)?;
        if self.version == 0 {
            return Err(StoreError::Invalid(
                "schema version must be positive".into(),
            ));
        }
        if serde_json::to_vec(self)?.len() > 2 * 1024 * 1024 {
            return Err(StoreError::Invalid("record exceeds 2 MiB".into()));
        }
        Ok(())
    }

    pub fn digest(&self) -> StoreResult<String> {
        self.validate()?;
        digest(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredRecord {
    pub key: StoreKey,
    pub revision: u64,
    pub data: RecordData,
    pub content_digest: String,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventWrite {
    pub kind: String,
    pub data: RecordData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredEvent {
    pub sequence: u64,
    pub key: StoreKey,
    pub record_revision: u64,
    pub kind: String,
    pub data: RecordData,
    pub recorded_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplaySafety {
    /// The adapter proves replay with the same idempotency key is safe. A new
    /// worker must retain the same exact persisted request and key.
    Idempotent,
    /// Expiry or a lost acknowledgment requires owner reconciliation, never an
    /// automatic repeat of the potentially completed external action.
    ReconcileBeforeRetry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectIntent {
    pub id: String,
    pub kind: String,
    pub request: RecordData,
    pub idempotency_key: String,
    pub replay_safety: ReplaySafety,
}

impl EffectIntent {
    /// Exact identity of the complete persisted dispatch request, including its
    /// key and replay contract, not only the domain payload.
    pub fn digest(&self) -> StoreResult<String> {
        digest(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionRequest {
    pub key: StoreKey,
    pub admission_key: String,
    pub data: RecordData,
    pub events: Vec<EventWrite>,
    pub effects: Vec<EffectIntent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionOutcome {
    Created(StoredRecord),
    Existing(StoredRecord),
}

impl AdmissionOutcome {
    pub fn record(&self) -> &StoredRecord {
        match self {
            Self::Created(record) | Self::Existing(record) => record,
        }
    }
}

/// Storage claims fence executor mutations. WorkGraph still owns work claims,
/// scheduling, dependencies and setup-worker admission where composed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordLease {
    pub store_id: String,
    pub key: StoreKey,
    pub worker: String,
    pub fence: u64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    pub key: StoreKey,
    pub expected_revision: u64,
    pub expected_digest: Option<String>,
    /// Required by a domain contract for executor-owned transitions. An owner
    /// may make other authorized CAS transitions, such as cancellation, without
    /// a worker lease. This store does not decide who has domain authority.
    pub claim: Option<RecordLease>,
    pub data: RecordData,
    pub events: Vec<EventWrite>,
    pub effects: Vec<EffectIntent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectKey {
    pub record: StoreKey,
    pub id: String,
}

impl EffectKey {
    pub fn validate(&self) -> StoreResult<()> {
        self.record.validate()?;
        identifier(&self.id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectStatus {
    /// Eligible for a currently authorized dispatch claim. After prior attempts,
    /// this does not assert that the remote effect has not already happened.
    Pending,
    InFlight,
    Uncertain,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectClaimKind {
    Dispatch,
    Reconcile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectLease {
    pub store_id: String,
    pub key: EffectKey,
    pub worker: String,
    pub fence: u64,
    pub expires_at_ms: i64,
    pub kind: EffectClaimKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredEffect {
    pub key: EffectKey,
    pub record_revision: u64,
    pub intent: EffectIntent,
    pub request_digest: String,
    pub status: EffectStatus,
    pub attempt: u64,
    pub lease: Option<EffectLease>,
    pub outcome: Option<EffectOutcome>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectClaimOutcome {
    Claimed(StoredEffect),
    NeedsReconciliation(StoredEffect),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectOutcome {
    Succeeded { receipt: RecordData },
    Failed { evidence: RecordData },
    Uncertain { evidence: RecordData },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reconciliation {
    Applied {
        receipt: RecordData,
    },
    /// Only the effect/domain owner may establish that it did not happen and
    /// that no outstanding old call can apply later. The evidence is retained;
    /// the next dispatch still requires current authority and available budget.
    NotApplied {
        evidence: RecordData,
    },
    /// The owner guarantees that replaying the exact persisted request and key
    /// cannot duplicate the effect, including against late old calls. This
    /// guarantee must survive process loss for the complete retry lifetime.
    /// It does not assert non-application, grant dispatch authority or reset an
    /// attempt budget. A missing lookup, timeout or lease expiry is not proof.
    RetrySafe {
        evidence: RecordData,
    },
    /// The owner has no conclusive outcome yet. Retain uncertainty, persist
    /// this observation and release only the current reconciliation claim.
    Pending {
        evidence: RecordData,
    },
    Failed {
        evidence: RecordData,
    },
}

/// No required method has a default implementation. Every call is scoped. The
/// authenticated application owner is responsible for authorizing the scope.
#[async_trait]
pub trait ToolkitStore: Send + Sync {
    fn capabilities(&self) -> StoreCapabilities;
    async fn now_ms(&self) -> StoreResult<i64>;
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome>;
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>>;
    async fn list_records(
        &self,
        scope: &StoreScope,
        after_id: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredRecord>>;
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord>;
    async fn claim_record(
        &self,
        key: &StoreKey,
        expected_revision: u64,
        worker: &str,
        ttl_ms: u64,
    ) -> StoreResult<RecordLease>;
    async fn renew_record_claim(
        &self,
        lease: &RecordLease,
        ttl_ms: u64,
    ) -> StoreResult<RecordLease>;
    /// Recover the current token after losing the result of a renewal. The
    /// caller must still be authorized for the scope and original worker. Only
    /// the same live worker/fence can be recovered; this never reacquires an
    /// expired, released or superseded claim. Direct mutations continue to
    /// require the current exact token, including its renewed expiry.
    async fn recover_record_claim(&self, prior: &RecordLease) -> StoreResult<RecordLease>;
    async fn release_record_claim(&self, lease: &RecordLease) -> StoreResult<()>;
    async fn read_events(
        &self,
        key: &StoreKey,
        after_sequence: u64,
        limit: u32,
    ) -> StoreResult<Vec<StoredEvent>>;
    async fn get_effect(&self, key: &EffectKey) -> StoreResult<Option<StoredEffect>>;
    async fn list_effects(
        &self,
        key: &StoreKey,
        after_id: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredEffect>>;
    /// Atomically fence dispatch admission against the owner's revision that
    /// the caller just authorized. A domain executor must also honor revocation
    /// and cancellation at its own effect boundary; this method cannot infer
    /// policy from a record's JSON or undo an already dispatched external call.
    async fn claim_effect(
        &self,
        key: &EffectKey,
        expected_record_revision: u64,
        worker: &str,
        ttl_ms: u64,
    ) -> StoreResult<EffectClaimOutcome>;
    async fn acknowledge_effect(
        &self,
        lease: &EffectLease,
        outcome: EffectOutcome,
    ) -> StoreResult<StoredEffect>;
    /// Observe a previously attempted effect without consuming a dispatch
    /// attempt. Pending effects with prior attempts remain observable after
    /// retry safety, cancellation or budget exhaustion. Fresh Pending effects
    /// and any effect with a live claim are refused. The owner still enforces
    /// current observation authority; this claim does not authorize dispatch.
    async fn claim_reconciliation(
        &self,
        key: &EffectKey,
        worker: &str,
        ttl_ms: u64,
    ) -> StoreResult<StoredEffect>;
    async fn reconcile_effect(
        &self,
        lease: &EffectLease,
        resolution: Reconciliation,
    ) -> StoreResult<StoredEffect>;
}

pub(crate) fn identifier(value: &str) -> StoreResult<()> {
    if value.is_empty()
        || value.len() > 512
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(StoreError::Invalid(
            "invalid or oversized identifier".into(),
        ));
    }
    Ok(())
}

pub(crate) fn digest(value: &impl Serialize) -> StoreResult<String> {
    fn canonical(value: &Value, bytes: &mut Vec<u8>) -> StoreResult<()> {
        match value {
            Value::Object(map) => {
                bytes.push(b'{');
                let mut entries = map.iter().collect::<Vec<_>>();
                entries.sort_unstable_by_key(|(key, _)| *key);
                for (index, (key, value)) in entries.into_iter().enumerate() {
                    if index != 0 {
                        bytes.push(b',');
                    }
                    bytes.extend(serde_json::to_vec(key)?);
                    bytes.push(b':');
                    canonical(value, bytes)?;
                }
                bytes.push(b'}');
            }
            Value::Array(values) => {
                bytes.push(b'[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        bytes.push(b',');
                    }
                    canonical(value, bytes)?;
                }
                bytes.push(b']');
            }
            scalar => bytes.extend(serde_json::to_vec(scalar)?),
        }
        Ok(())
    }
    let mut bytes = Vec::new();
    canonical(&serde_json::to_value(value)?, &mut bytes)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub(crate) fn validate_limit(limit: u32) -> StoreResult<()> {
    if !(1..=1000).contains(&limit) {
        return Err(StoreError::Invalid("read limit must be 1-1000".into()));
    }
    Ok(())
}

pub(crate) fn validate_ttl(ttl_ms: u64) -> StoreResult<i64> {
    if !(1..=86_400_000).contains(&ttl_ms) {
        return Err(StoreError::Invalid(
            "lease TTL must be 1-86400000 ms".into(),
        ));
    }
    Ok(ttl_ms as i64)
}

pub(crate) fn validate_effects_and_events(
    effects: &[EffectIntent],
    events: &[EventWrite],
) -> StoreResult<()> {
    if effects.len() > 1000 || events.len() > 1000 {
        return Err(StoreError::Invalid(
            "atomic batch exceeds 1000 entries".into(),
        ));
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut keys = std::collections::BTreeSet::new();
    for effect in effects {
        identifier(&effect.id)?;
        identifier(&effect.kind)?;
        identifier(&effect.idempotency_key)?;
        effect.request.validate()?;
        if !ids.insert(&effect.id) || !keys.insert(&effect.idempotency_key) {
            return Err(StoreError::EffectConflict);
        }
    }
    for event in events {
        identifier(&event.kind)?;
        event.data.validate()?;
    }
    Ok(())
}
