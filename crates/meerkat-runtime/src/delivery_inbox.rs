//! Runtime-owned durable delivery inbox.
//!
//! `RuntimeDeliveryMachine` is the semantic authority for idempotent sequence
//! assignment and ordered application. `RuntimeStore` implementations retain
//! its exact state with CAS and atomically insert opaque inbox rows.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::identifiers::LogicalRuntimeId;
use crate::store::{
    RuntimeDeliveryAuthorityCasOutcome, RuntimeDeliveryAuthorityRecord, RuntimeDeliveryStoreRecord,
    RuntimeStore, RuntimeStoreError,
};

pub mod dsl;

/// Authority envelope version for a runtime without refused settlements;
/// every released reader accepts it.
const AUTHORITY_ENVELOPE_VERSION: u16 = 1;
/// Authority envelope version once a runtime holds a refused settlement.
/// Readers that predate refused settlement accept only
/// [`AUTHORITY_ENVELOPE_VERSION`], so they refuse such a runtime instead of
/// reading its refused rows as applied through the cursor.
const REFUSAL_AUTHORITY_ENVELOPE_VERSION: u16 = 2;
// Enrollment itself fences whole-row readers, before the first effect.
const RECIPIENT_AUTHORITY_ENVELOPE_VERSION: u16 = 3;
const SUBMISSION_ENVELOPE_VERSION: u16 = 1;
const MAX_CAS_ATTEMPTS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeDeliveryId(String);

impl RuntimeDeliveryId {
    pub fn new(value: impl Into<String>) -> Result<Self, RuntimeDeliveryError> {
        Ok(Self(validate_component("delivery id", value.into())?))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RuntimeDeliveryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeDeliveryKind {
    JobTerminal,
    JobNotification,
    /// An original-task continuation: a result delivered to its owner as a
    /// new durable input, keyed by a host-owned continuation key.
    Continuation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDeliverySubmission {
    delivery_id: RuntimeDeliveryId,
    kind: RuntimeDeliveryKind,
    source_id: String,
    source_sequence: u64,
    interaction_lineage_id: String,
    payload: Vec<u8>,
}

impl RuntimeDeliverySubmission {
    pub fn new(
        delivery_id: RuntimeDeliveryId,
        kind: RuntimeDeliveryKind,
        source_id: impl Into<String>,
        source_sequence: u64,
        interaction_lineage_id: impl Into<String>,
        payload: Vec<u8>,
    ) -> Result<Self, RuntimeDeliveryError> {
        let submission = Self {
            delivery_id,
            kind,
            source_id: source_id.into(),
            source_sequence,
            interaction_lineage_id: interaction_lineage_id.into(),
            payload,
        };
        submission.validate()?;
        Ok(submission)
    }

    pub fn delivery_id(&self) -> &RuntimeDeliveryId {
        &self.delivery_id
    }

    pub const fn kind(&self) -> RuntimeDeliveryKind {
        self.kind
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub const fn source_sequence(&self) -> u64 {
        self.source_sequence
    }

    pub fn interaction_lineage_id(&self) -> &str {
        &self.interaction_lineage_id
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    fn validate(&self) -> Result<(), RuntimeDeliveryError> {
        validate_component_ref("delivery id", self.delivery_id.as_str())?;
        validate_component_ref("delivery source id", &self.source_id)?;
        validate_component_ref(
            "delivery interaction lineage id",
            &self.interaction_lineage_id,
        )?;
        if self.source_sequence == 0 {
            return Err(RuntimeDeliveryError::InvalidInput(
                "delivery source sequence must be positive".into(),
            ));
        }
        if self.payload.is_empty() {
            return Err(RuntimeDeliveryError::InvalidInput(
                "delivery payload must not be empty".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDeliveryReceipt {
    pub delivery_id: RuntimeDeliveryId,
    pub sequence: u64,
    pub deduplicated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDeliveryRecord {
    pub sequence: u64,
    pub submission: RuntimeDeliverySubmission,
}

/// Exact subscription identity and target supplied by the trusted producer
/// owner after decoding the complete immutable delivery payload. These bytes
/// are a binding, not proof of source, audience, or execution permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDeliveryRecipient {
    id: String,
    target_binding: String,
}

impl RuntimeDeliveryRecipient {
    pub fn new(
        id: impl Into<String>,
        target_binding: impl Into<String>,
    ) -> Result<Self, RuntimeDeliveryError> {
        let value = Self {
            id: validate_component("recipient id", id.into())?,
            target_binding: target_binding.into(),
        };
        if value.target_binding.is_empty() {
            return Err(RuntimeDeliveryError::InvalidInput(
                "recipient target binding must not be empty".into(),
            ));
        }
        Ok(value)
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn target_binding(&self) -> &str {
        &self.target_binding
    }
}

/// A completed local disposition. Transport, store, observation, and unknown
/// effect failures are not dispositions and must remain pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeDeliveryRecipientOutcome {
    Applied,
    Refused,
    OperationAuthorizationUnavailable,
}

impl From<RuntimeDeliveryRecipientOutcome> for dsl::DeliveryRecipientOutcome {
    fn from(value: RuntimeDeliveryRecipientOutcome) -> Self {
        match value {
            RuntimeDeliveryRecipientOutcome::Applied => Self::Applied,
            RuntimeDeliveryRecipientOutcome::Refused => Self::Refused,
            RuntimeDeliveryRecipientOutcome::OperationAuthorizationUnavailable => {
                Self::OperationAuthorizationUnavailable
            }
        }
    }
}
impl From<dsl::DeliveryRecipientOutcome> for RuntimeDeliveryRecipientOutcome {
    fn from(value: dsl::DeliveryRecipientOutcome) -> Self {
        match value {
            dsl::DeliveryRecipientOutcome::Applied => Self::Applied,
            dsl::DeliveryRecipientOutcome::Refused => Self::Refused,
            dsl::DeliveryRecipientOutcome::OperationAuthorizationUnavailable => {
                Self::OperationAuthorizationUnavailable
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeDeliveryRecipientGroupOutcome {
    AllApplied,
    AllRefused,
    AllAuthorizationUnavailable,
    Mixed,
}

impl From<RuntimeDeliveryRecipientGroupOutcome> for dsl::DeliveryRecipientGroupOutcome {
    fn from(value: RuntimeDeliveryRecipientGroupOutcome) -> Self {
        match value {
            RuntimeDeliveryRecipientGroupOutcome::AllApplied => Self::AllApplied,
            RuntimeDeliveryRecipientGroupOutcome::AllRefused => Self::AllRefused,
            RuntimeDeliveryRecipientGroupOutcome::AllAuthorizationUnavailable => {
                Self::AllAuthorizationUnavailable
            }
            RuntimeDeliveryRecipientGroupOutcome::Mixed => Self::Mixed,
        }
    }
}
impl From<dsl::DeliveryRecipientGroupOutcome> for RuntimeDeliveryRecipientGroupOutcome {
    fn from(value: dsl::DeliveryRecipientGroupOutcome) -> Self {
        match value {
            dsl::DeliveryRecipientGroupOutcome::AllApplied => Self::AllApplied,
            dsl::DeliveryRecipientGroupOutcome::AllRefused => Self::AllRefused,
            dsl::DeliveryRecipientGroupOutcome::AllAuthorizationUnavailable => {
                Self::AllAuthorizationUnavailable
            }
            dsl::DeliveryRecipientGroupOutcome::Mixed => Self::Mixed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDeliveryRecipientState {
    pub recipient: RuntimeDeliveryRecipient,
    pub outcome: Option<RuntimeDeliveryRecipientOutcome>,
}

// A read result from the committed owner, scoped to the requested delivery.
// Returning it must not clone historical deliveries after every recipient CAS.
struct RecipientTransitionProjection {
    outcomes: std::collections::BTreeMap<String, RuntimeDeliveryRecipientOutcome>,
    group: Option<RuntimeDeliveryRecipientGroupOutcome>,
}

impl RecipientTransitionProjection {
    fn from_state(
        state: &dsl::RuntimeDeliveryMachineState,
        delivery_id: &str,
    ) -> Result<Self, RuntimeDeliveryError> {
        let outcomes = state.recipient_outcomes.get(delivery_id).ok_or_else(|| {
            RuntimeDeliveryError::Authority("recipient operation emitted no outcome map".into())
        })?;
        Ok(Self {
            outcomes: outcomes
                .iter()
                .map(|(recipient, outcome)| (recipient.clone(), (*outcome).into()))
                .collect(),
            group: state
                .recipient_group_outcomes
                .get(delivery_id)
                .copied()
                .map(Into::into),
        })
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum RuntimeDeliveryError {
    #[error("invalid runtime delivery: {0}")]
    InvalidInput(String),
    #[error("runtime delivery idempotency conflict for {0}")]
    IdempotencyConflict(RuntimeDeliveryId),
    #[error(
        "runtime delivery {delivery_id} is out of order: next sequence is {expected}, received {actual}"
    )]
    OutOfOrder {
        delivery_id: RuntimeDeliveryId,
        expected: u64,
        actual: u64,
    },
    #[error("runtime delivery persistence is corrupt: {0}")]
    Corrupt(String),
    #[error("runtime delivery authority rejected the transition: {0}")]
    Authority(String),
    #[error(transparent)]
    Store(#[from] RuntimeStoreError),
}

/// Why a delivery was settled as refused: a terminal policy outcome. The
/// cursor passes a refused delivery, so the rows after it proceed, and it is
/// never applied. Infrastructure failures are not refusals: they stay pending
/// and block the ordered inbox until they succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeDeliveryRefusalReason {
    /// The delivery's original native work binding is missing or invalid, and
    /// immutably so: it can never be admitted on a governed runtime.
    NoAdmissibleWorkBinding,
    /// The native work authorization owner's current verdict on the
    /// delivery's original work is an actual denial (never an unavailable
    /// owner, an observation failure or a store error).
    AuthorityDenied,
    /// The native work authorization owner actually reported the operation
    /// authorization as unavailable for this delivery: a settled verdict,
    /// never an observation or store failure (those stay pending).
    OperationAuthorizationUnavailable,
}

impl From<RuntimeDeliveryRefusalReason> for dsl::DeliveryRefusalReason {
    fn from(reason: RuntimeDeliveryRefusalReason) -> Self {
        match reason {
            RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding => Self::NoAdmissibleWorkBinding,
            RuntimeDeliveryRefusalReason::AuthorityDenied => Self::AuthorityDenied,
            RuntimeDeliveryRefusalReason::OperationAuthorizationUnavailable => {
                Self::OperationAuthorizationUnavailable
            }
        }
    }
}

impl From<dsl::DeliveryRefusalReason> for RuntimeDeliveryRefusalReason {
    fn from(reason: dsl::DeliveryRefusalReason) -> Self {
        match reason {
            dsl::DeliveryRefusalReason::NoAdmissibleWorkBinding => Self::NoAdmissibleWorkBinding,
            dsl::DeliveryRefusalReason::AuthorityDenied => Self::AuthorityDenied,
            dsl::DeliveryRefusalReason::OperationAuthorizationUnavailable => {
                Self::OperationAuthorizationUnavailable
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuthorityEnvelope {
    version: u16,
    state: PersistedAuthorityState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedAuthorityState {
    delivery_ids: std::collections::BTreeSet<String>,
    delivery_sequences: std::collections::BTreeMap<String, u64>,
    delivery_source_sequences: std::collections::BTreeMap<String, u64>,
    committed_sequences: std::collections::BTreeSet<u64>,
    next_sequence: u64,
    applied_cursor: u64,
    // Absent on authorities written before out-of-band acknowledgement.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    acknowledged_sequences: std::collections::BTreeSet<u64>,
    // Absent on authorities written before refused settlement.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    refused_deliveries: std::collections::BTreeMap<String, RuntimeDeliveryRefusalReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recipients: Option<PersistedRecipientState>,
}

// The v3 cell is all-or-nothing. Missing fields must not silently erase
// partial progress on restore, while v1/v2 retain their exact old shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedRecipientState {
    bindings: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    outcomes: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, RuntimeDeliveryRecipientOutcome>,
    >,
    groups: std::collections::BTreeMap<String, RuntimeDeliveryRecipientGroupOutcome>,
}

impl From<&dsl::RuntimeDeliveryMachineState> for PersistedAuthorityState {
    fn from(state: &dsl::RuntimeDeliveryMachineState) -> Self {
        Self {
            delivery_ids: state.delivery_ids.clone(),
            delivery_sequences: state.delivery_sequences.clone(),
            delivery_source_sequences: state.delivery_source_sequences.clone(),
            committed_sequences: state.committed_sequences.clone(),
            next_sequence: state.next_sequence,
            applied_cursor: state.applied_cursor,
            acknowledged_sequences: state.acknowledged_sequences.clone(),
            recipients: (!state.delivery_recipient_bindings.is_empty()).then(|| {
                PersistedRecipientState {
                    bindings: state.delivery_recipient_bindings.clone(),
                    outcomes: state
                        .recipient_outcomes
                        .iter()
                        .map(|(id, outcomes)| {
                            (
                                id.clone(),
                                outcomes
                                    .iter()
                                    .map(|(recipient, outcome)| {
                                        (recipient.clone(), (*outcome).into())
                                    })
                                    .collect(),
                            )
                        })
                        .collect(),
                    groups: state
                        .recipient_group_outcomes
                        .iter()
                        .map(|(id, outcome)| (id.clone(), (*outcome).into()))
                        .collect(),
                }
            }),
            refused_deliveries: state
                .refused_deliveries
                .iter()
                .map(|(id, reason)| (id.clone(), (*reason).into()))
                .collect(),
        }
    }
}

impl From<PersistedAuthorityState> for dsl::RuntimeDeliveryMachineState {
    fn from(state: PersistedAuthorityState) -> Self {
        let recipients = state.recipients.unwrap_or_default();
        Self {
            lifecycle_phase: dsl::RuntimeDeliveryPhase::Active,
            delivery_ids: state.delivery_ids,
            delivery_sequences: state.delivery_sequences,
            delivery_source_sequences: state.delivery_source_sequences,
            committed_sequences: state.committed_sequences,
            next_sequence: state.next_sequence,
            applied_cursor: state.applied_cursor,
            acknowledged_sequences: state.acknowledged_sequences,
            delivery_recipient_bindings: recipients.bindings,
            recipient_outcomes: recipients
                .outcomes
                .into_iter()
                .map(|(id, outcomes)| {
                    (
                        id,
                        outcomes
                            .into_iter()
                            .map(|(recipient, outcome)| (recipient, outcome.into()))
                            .collect(),
                    )
                })
                .collect(),
            recipient_group_outcomes: recipients
                .groups
                .into_iter()
                .map(|(id, outcome)| (id, outcome.into()))
                .collect(),
            refused_deliveries: state
                .refused_deliveries
                .into_iter()
                .map(|(id, reason)| (id, reason.into()))
                .collect(),
        }
    }
}

impl PersistedAuthorityState {
    fn validate(&self) -> Result<(), RuntimeDeliveryError> {
        for delivery_id in &self.delivery_ids {
            validate_component_ref("persisted delivery id", delivery_id)
                .map_err(|error| RuntimeDeliveryError::Corrupt(error.to_string()))?;
        }
        let sequence_keys = self
            .delivery_sequences
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let source_keys = self
            .delivery_source_sequences
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if sequence_keys != self.delivery_ids || source_keys != self.delivery_ids {
            return Err(RuntimeDeliveryError::Corrupt(
                "runtime delivery authority indexes disagree on delivery identity".into(),
            ));
        }
        if self
            .delivery_source_sequences
            .values()
            .any(|sequence| *sequence == 0)
        {
            return Err(RuntimeDeliveryError::Corrupt(
                "runtime delivery authority contains a zero source sequence".into(),
            ));
        }
        let mapped_sequences = self
            .delivery_sequences
            .values()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        if mapped_sequences != self.committed_sequences {
            return Err(RuntimeDeliveryError::Corrupt(
                "runtime delivery sequence index disagrees with committed sequence authority"
                    .into(),
            ));
        }
        let committed_count = u64::try_from(self.committed_sequences.len()).map_err(|_| {
            RuntimeDeliveryError::Corrupt(
                "runtime delivery committed sequence count exceeds u64".into(),
            )
        })?;
        let has_exact_bounds = if self.next_sequence == 0 {
            self.committed_sequences.is_empty()
        } else {
            self.committed_sequences.first() == Some(&1)
                && self.committed_sequences.last() == Some(&self.next_sequence)
        };
        if committed_count != self.next_sequence || !has_exact_bounds {
            return Err(RuntimeDeliveryError::Corrupt(
                "runtime delivery committed sequences are not contiguous through the high-water mark"
                    .into(),
            ));
        }
        if self.applied_cursor > self.next_sequence {
            return Err(RuntimeDeliveryError::Corrupt(
                "runtime delivery applied cursor exceeds the committed high-water mark".into(),
            ));
        }
        if self
            .acknowledged_sequences
            .iter()
            .any(|sequence| *sequence <= self.applied_cursor || *sequence > self.next_sequence)
        {
            return Err(RuntimeDeliveryError::Corrupt(
                "runtime delivery acknowledgement lies outside the pending committed range".into(),
            ));
        }
        if self.refused_deliveries.keys().any(|delivery_id| {
            self.delivery_sequences
                .get(delivery_id)
                .is_none_or(|sequence| *sequence > self.applied_cursor)
        }) {
            return Err(RuntimeDeliveryError::Corrupt(
                "runtime delivery refusal names a delivery the cursor has not passed".into(),
            ));
        }
        if let Some(recipients) = &self.recipients {
            if recipients.bindings.is_empty() {
                return Err(RuntimeDeliveryError::Corrupt(
                    "recipient authority has no enrolled group".into(),
                ));
            }
            for bindings in recipients.bindings.values() {
                for (recipient, target) in bindings {
                    if target.is_empty() {
                        return Err(RuntimeDeliveryError::Corrupt(
                            "invalid runtime delivery recipient binding".into(),
                        ));
                    }
                    validate_component_ref("persisted recipient id", recipient)
                        .map_err(|error| RuntimeDeliveryError::Corrupt(error.to_string()))?;
                }
            }
        }
        Ok(())
    }
}

/// Outcome of [`RuntimeDeliveryInbox::acknowledge`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeDeliveryAcknowledgement {
    /// The delivery is at or below the applied cursor (applied now or earlier).
    Applied { applied_cursor: u64 },
    /// The delivery is ahead of the cursor behind an unapplied row. It is
    /// recorded and consumed, without re-application, when the cursor reaches
    /// it; nothing behind it is blocked by the out-of-order acknowledgement.
    Recorded { applied_cursor: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SubmissionEnvelope {
    version: u16,
    submission: RuntimeDeliverySubmission,
}

/// Durable, ordered runtime delivery inbox over one [`RuntimeStore`].
///
/// Clones share one in-process commit signal ([`Self::subscribe_commits`]).
/// A process should hold one inbox per runtime store and hand out clones, so
/// every consumer observes every commit made through it; the persistence
/// bundle owns that instance.
#[derive(Clone)]
pub struct RuntimeDeliveryInbox {
    store: Arc<dyn RuntimeStore>,
    commits: Arc<crate::tokio::sync::watch::Sender<u64>>,
    /// Runtimes that received a newly committed row since the last
    /// [`Self::take_committed_runtimes`], shared by all clones.
    committed_runtimes: Arc<Mutex<HashSet<LogicalRuntimeId>>>,
    /// Whether a [`RuntimeDeliveryOwnership`] is outstanding, shared by all
    /// clones.
    owner_claimed: Arc<AtomicBool>,
}

/// The generated authority's verdict for one delivery id.
///
/// Read from the store only, through the machine's read-only
/// `ClassifyDeliveryStatus`, whose arms partition every id: exactly one
/// verdict holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeDeliveryStatus {
    /// No row under this id in this runtime.
    NotCommitted,
    /// Every bound recipient settled with differing outcomes. Exact retained
    /// target bindings and dispositions are readable without replaying effects.
    Mixed {
        delivery_sequence: u64,
        recipients: Vec<RuntimeDeliveryRecipientState>,
    },
    /// Committed and not yet applied.
    Pending { delivery_sequence: u64 },
    /// Committed ahead of the cursor and acknowledged out of band: its effect
    /// reached the runtime by another path while an earlier row is pending.
    AcknowledgedAhead { delivery_sequence: u64 },
    /// Applied as a legacy whole row or as an all-applied recipient group.
    Applied { delivery_sequence: u64 },
    /// Settled as refused: the cursor passed it and it was never applied.
    Refused {
        delivery_sequence: u64,
        reason: RuntimeDeliveryRefusalReason,
    },
}

/// Lowercase hex SHA-256 of `parts`, each length-prefixed so the encoding
/// is unambiguous. Used for continuation submission digests and derived
/// delivery ids.
pub fn delivery_digest_hex(parts: &[&[u8]]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut rendered = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        // Writing to a String is infallible; the formatter error is discarded
        // deliberately rather than unwrapped.
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

/// The first binding a keyed submit records, minus what the inbox decides
/// (the address and delivery id come from the submission itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationKeyClaim {
    pub owner: String,
    pub key: String,
    pub submission_digest: String,
    pub committed_at_ms: u64,
}

/// Outcome of [`RuntimeDeliveryInbox::submit_with_key_claim`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyedSubmitOutcome {
    /// The row and the key's first binding committed together.
    Committed {
        receipt: RuntimeDeliveryReceipt,
        binding: crate::store::ContinuationKeyBinding,
    },
    /// The `(owner, key)` pair was already bound; nothing was written. The
    /// caller compares digests to tell a replay from a conflict.
    AlreadyBound(crate::store::ContinuationKeyBinding),
}

/// Exclusive delivery ownership of one [`RuntimeDeliveryInbox`].
///
/// At most one ownership exists per inbox (across its clones) at a time; it
/// is released when dropped. Only the owner takes the runtimes of new
/// commits, so a second consumer can never steal another's wakeups.
pub struct RuntimeDeliveryOwnership {
    inbox: RuntimeDeliveryInbox,
}

impl std::fmt::Debug for RuntimeDeliveryOwnership {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeDeliveryOwnership")
            .finish_non_exhaustive()
    }
}

impl RuntimeDeliveryOwnership {
    pub fn inbox(&self) -> &RuntimeDeliveryInbox {
        &self.inbox
    }

    /// Take the runtimes that received a newly committed row through the
    /// inbox (or a clone) since the previous call.
    ///
    /// A runtime is recorded before the commit generation advances, so an
    /// owner that observes a generation change and then takes the set always
    /// sees the runtime of that commit. Like the commit signal, this is
    /// in-process only.
    pub fn take_committed_runtimes(&self) -> Vec<LogicalRuntimeId> {
        let mut committed = self
            .inbox
            .committed_runtimes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        committed.drain().collect()
    }
}

impl Drop for RuntimeDeliveryOwnership {
    fn drop(&mut self) {
        self.inbox.owner_claimed.store(false, Ordering::Release);
    }
}

/// A second delivery owner was armed on an inbox that already has one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("runtime delivery inbox already has a delivery owner")]
pub struct RuntimeDeliveryOwnerAlreadyArmed;

impl std::fmt::Debug for RuntimeDeliveryInbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeDeliveryInbox")
            .finish_non_exhaustive()
    }
}

impl RuntimeDeliveryInbox {
    pub fn new(store: Arc<dyn RuntimeStore>) -> Self {
        let (commits, _) = crate::tokio::sync::watch::channel(0);
        Self {
            store,
            commits: Arc::new(commits),
            committed_runtimes: Arc::new(Mutex::new(HashSet::new())),
            owner_claimed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Claim exclusive delivery ownership of this inbox.
    ///
    /// Fails with [`RuntimeDeliveryOwnerAlreadyArmed`] while another
    /// ownership (through this inbox or any clone) is outstanding. The set of
    /// committed runtimes is reset on claim: rows committed before the claim
    /// are found by the owner's reconcile read, not by the set.
    pub fn claim_delivery_ownership(
        &self,
    ) -> Result<RuntimeDeliveryOwnership, RuntimeDeliveryOwnerAlreadyArmed> {
        if self
            .owner_claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(RuntimeDeliveryOwnerAlreadyArmed);
        }
        self.committed_runtimes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        Ok(RuntimeDeliveryOwnership {
            inbox: self.clone(),
        })
    }

    /// Observe newly committed deliveries made through this inbox or any of
    /// its clones.
    ///
    /// The value is a monotonically increasing commit generation; it advances
    /// once per newly inserted row, after the store commit. An exact replay
    /// (deduplicated submit) does not advance it. The signal is in-process
    /// only: rows committed by another process or another inbox instance are
    /// not observed and must be found by reading delivery authority (for
    /// example [`Self::runtimes_with_pending_deliveries`]).
    pub fn subscribe_commits(&self) -> crate::tokio::sync::watch::Receiver<u64> {
        self.commits.subscribe()
    }

    /// Record a newly committed row for the delivery owner, then advance the
    /// commit generation (in that order, see `take_committed_runtimes`).
    fn record_committed_runtime(&self, runtime_id: &LogicalRuntimeId) {
        self.committed_runtimes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(runtime_id.clone());
        self.commits
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }

    /// The store's durable delivery generation (#1813); see
    /// [`RuntimeStore::load_delivery_generation`].
    pub async fn delivery_generation(&self) -> Result<u64, RuntimeDeliveryError> {
        Ok(self.store.load_delivery_generation().await?)
    }

    /// How sessions on this inbox's store are hosted across processes
    /// (#1813): delivery owners route rows and wake by it.
    pub fn hosting_capability(&self) -> crate::session_hosting::HostingCapability {
        self.store.hosting_capability()
    }

    /// Whether `other` shares this inbox's commit signal, i.e. is the same
    /// owned instance (or a clone of it) rather than a second inbox over the
    /// same store.
    pub fn shares_commit_signal_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.commits, &other.commits)
    }

    pub async fn submit(
        &self,
        runtime_id: &LogicalRuntimeId,
        submission: RuntimeDeliverySubmission,
    ) -> Result<RuntimeDeliveryReceipt, RuntimeDeliveryError> {
        self.submit_with_acknowledgement(runtime_id, submission, false)
            .await
    }

    /// Commit a delivery whose effect its producer applies itself, already
    /// acknowledged, in the same authority compare-and-swap as the insert.
    ///
    /// No applier ever runs a sink for the new row: it is consumed when the
    /// cursor reaches it, exactly like a row acknowledged out of band, but
    /// with no window in which a delivery owner can observe it unacknowledged.
    /// The producer must be able to re-derive its effect after a crash from
    /// its own durable state. An exact replay of an existing row acknowledges
    /// it as [`Self::acknowledge`] would.
    pub async fn submit_acknowledged(
        &self,
        runtime_id: &LogicalRuntimeId,
        submission: RuntimeDeliverySubmission,
    ) -> Result<RuntimeDeliveryReceipt, RuntimeDeliveryError> {
        self.submit_with_acknowledgement(runtime_id, submission, true)
            .await
    }

    async fn submit_with_acknowledgement(
        &self,
        runtime_id: &LogicalRuntimeId,
        submission: RuntimeDeliverySubmission,
        acknowledge: bool,
    ) -> Result<RuntimeDeliveryReceipt, RuntimeDeliveryError> {
        for _ in 0..MAX_CAS_ATTEMPTS {
            let observed = self
                .store
                .load_runtime_delivery_authority(runtime_id)
                .await?;
            let mut authority = decode_or_new_authority(observed.as_ref())?;
            let delivery_id = submission.delivery_id.clone();
            let transition = dsl::RuntimeDeliveryMachineMutator::apply(
                &mut authority,
                dsl::RuntimeDeliveryInput::CommitDelivery {
                    delivery_id: delivery_id.as_str().to_string(),
                    source_sequence: submission.source_sequence,
                },
            )
            .map_err(|error| RuntimeDeliveryError::Authority(format!("{error:?}")))?;

            let (sequence, deduplicated) = classify_commit_effects(
                transition.effects(),
                &delivery_id,
                submission.source_sequence,
            )?;
            if deduplicated {
                let stored = self
                    .store
                    .load_runtime_delivery_record(runtime_id, delivery_id.as_str())
                    .await?
                    .ok_or_else(|| {
                        RuntimeDeliveryError::Corrupt(format!(
                            "generated authority remembers {delivery_id}, but its inbox row is missing"
                        ))
                    })?;
                let persisted = decode_submission(&stored)?;
                if persisted != submission {
                    return Err(RuntimeDeliveryError::IdempotencyConflict(delivery_id));
                }
                if stored.sequence() != sequence {
                    return Err(RuntimeDeliveryError::Corrupt(format!(
                        "delivery {delivery_id} row sequence {} disagrees with generated sequence {sequence}",
                        stored.sequence()
                    )));
                }
                if acknowledge {
                    self.acknowledge(runtime_id, &delivery_id, sequence).await?;
                }
                return Ok(RuntimeDeliveryReceipt {
                    delivery_id,
                    sequence,
                    deduplicated: true,
                });
            }

            if acknowledge {
                let transition = dsl::RuntimeDeliveryMachineMutator::apply(
                    &mut authority,
                    dsl::RuntimeDeliveryInput::AcknowledgeDelivery {
                        delivery_id: delivery_id.as_str().to_string(),
                        delivery_sequence: sequence,
                    },
                )
                .map_err(|error| RuntimeDeliveryError::Authority(format!("{error:?}")))?;
                classify_acknowledgement_effects(transition.effects(), &delivery_id, sequence)?;
                advance_acknowledged_prefix(&mut authority)?;
            }
            let next_revision = observed
                .as_ref()
                .map_or(Ok(1), |record| next_revision(record.revision()))?;
            let replacement = RuntimeDeliveryAuthorityRecord::from_parts(
                next_revision,
                encode_authority(&authority)?,
            );
            let inserted = RuntimeDeliveryStoreRecord::from_parts(
                delivery_id.as_str(),
                sequence,
                encode_submission(&submission)?,
            );
            match self
                .store
                .compare_and_swap_runtime_delivery_authority(
                    runtime_id,
                    observed
                        .as_ref()
                        .map(RuntimeDeliveryAuthorityRecord::revision),
                    replacement,
                    Some(inserted),
                )
                .await?
            {
                RuntimeDeliveryAuthorityCasOutcome::Applied(_) => {
                    self.record_committed_runtime(runtime_id);
                    return Ok(RuntimeDeliveryReceipt {
                        delivery_id,
                        sequence,
                        deduplicated: false,
                    });
                }
                RuntimeDeliveryAuthorityCasOutcome::Conflict(_) => continue,
            }
        }
        Err(RuntimeDeliveryError::Store(RuntimeStoreError::WriteFailed(
            format!("runtime delivery CAS did not converge after {MAX_CAS_ATTEMPTS} attempts"),
        )))
    }

    pub async fn list_pending(
        &self,
        runtime_id: &LogicalRuntimeId,
        limit: usize,
    ) -> Result<Vec<RuntimeDeliveryRecord>, RuntimeDeliveryError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let authority = self
            .store
            .load_runtime_delivery_authority(runtime_id)
            .await?;
        let authority = decode_or_new_authority(authority.as_ref())?;
        let cursor = authority.state().applied_cursor;
        let pending_count = authority.state().next_sequence - cursor;
        let expected_count = pending_count.min(u64::try_from(limit).unwrap_or(u64::MAX));
        let rows = self
            .store
            .list_runtime_delivery_records(runtime_id, cursor, limit)
            .await?;
        let mut expected_sequence = cursor.checked_add(1);
        let records = rows
            .into_iter()
            .map(|row| {
                let sequence = row.sequence();
                if expected_sequence != Some(sequence) {
                    return Err(RuntimeDeliveryError::Corrupt(format!(
                        "runtime delivery inbox has a sequence gap after cursor {cursor}: expected {expected_sequence:?}, found {sequence}"
                    )));
                }
                expected_sequence = sequence.checked_add(1);
                let submission = decode_submission(&row)?;
                let expected = authority
                    .state()
                    .delivery_sequences
                    .get(submission.delivery_id.as_str())
                    .copied()
                    .ok_or_else(|| {
                        RuntimeDeliveryError::Corrupt(format!(
                            "inbox row {} has no generated delivery authority",
                            submission.delivery_id
                        ))
                    })?;
                if expected != sequence {
                    return Err(RuntimeDeliveryError::Corrupt(format!(
                        "inbox row {} sequence {sequence} disagrees with generated sequence {expected}",
                        submission.delivery_id
                    )));
                }
                let expected_source_sequence = authority
                    .state()
                    .delivery_source_sequences
                    .get(submission.delivery_id.as_str())
                    .copied()
                    .ok_or_else(|| {
                        RuntimeDeliveryError::Corrupt(format!(
                            "inbox row {} has no generated source-sequence authority",
                            submission.delivery_id
                        ))
                    })?;
                if expected_source_sequence != submission.source_sequence {
                    return Err(RuntimeDeliveryError::Corrupt(format!(
                        "inbox row {} source sequence {} disagrees with generated source sequence {expected_source_sequence}",
                        submission.delivery_id, submission.source_sequence
                    )));
                }
                Ok(RuntimeDeliveryRecord {
                    sequence,
                    submission,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if u64::try_from(records.len()).unwrap_or(u64::MAX) != expected_count {
            return Err(RuntimeDeliveryError::Corrupt(format!(
                "runtime delivery inbox contains {} rows after cursor {cursor}, but generated authority requires {expected_count}",
                records.len()
            )));
        }
        Ok(records)
    }

    /// Enroll the complete recipient manifest decoded by the trusted producer
    /// owner from this exact stored row, including any legacy/default target.
    /// This API re-reads and compares the immutable row and binds the complete
    /// manifest once. It does not authenticate caller-created records, infer
    /// recipients from opaque payload bytes, or confer execution authority.
    ///
    /// Existing pending grouped rows may enroll without rewriting their
    /// payload. The authority becomes version 3 before any recipient executes.
    /// Retry uses the returned settlements to skip every completed recipient.
    /// Only job terminal and job notification deliveries support enrollment;
    /// single-target delivery kinds must retain their own admission path.
    pub async fn bind_recipients(
        &self,
        runtime_id: &LogicalRuntimeId,
        record: &RuntimeDeliveryRecord,
        recipients: &[RuntimeDeliveryRecipient],
    ) -> Result<Vec<RuntimeDeliveryRecipientState>, RuntimeDeliveryError> {
        if !matches!(
            record.submission.kind(),
            RuntimeDeliveryKind::JobTerminal | RuntimeDeliveryKind::JobNotification
        ) {
            return Err(RuntimeDeliveryError::InvalidInput(
                "this delivery kind does not support recipient enrollment".into(),
            ));
        }
        let mut bindings = std::collections::BTreeMap::new();
        for recipient in recipients {
            // Revalidate because this type also supports deserialization.
            RuntimeDeliveryRecipient::new(recipient.id.clone(), recipient.target_binding.clone())?;
            if bindings
                .insert(recipient.id.clone(), recipient.target_binding.clone())
                .is_some()
            {
                return Err(RuntimeDeliveryError::InvalidInput(
                    "duplicate delivery recipient id".into(),
                ));
            }
        }
        let state = self
            .apply_recipient_transition(
                runtime_id,
                record,
                dsl::RuntimeDeliveryInput::BindDeliveryRecipients {
                    delivery_id: record.submission.delivery_id().as_str().into(),
                    delivery_sequence: record.sequence,
                    recipients: bindings,
                },
            )
            .await?;
        recipients
            .iter()
            .map(|recipient| {
                Ok(RuntimeDeliveryRecipientState {
                    recipient: recipient.clone(),
                    outcome: state.outcomes.get(&recipient.id).copied(),
                })
            })
            .collect()
    }

    /// Persist a truthful completed local outcome for one exact bound target.
    /// Exact repeats observe it; a different outcome or target is rejected.
    /// This is not atomic with a sink effect and never makes an unknown
    /// external outcome safe to replay.
    pub async fn settle_recipient(
        &self,
        runtime_id: &LogicalRuntimeId,
        record: &RuntimeDeliveryRecord,
        recipient: &RuntimeDeliveryRecipient,
        outcome: RuntimeDeliveryRecipientOutcome,
    ) -> Result<RuntimeDeliveryRecipientState, RuntimeDeliveryError> {
        self.apply_recipient_transition(
            runtime_id,
            record,
            dsl::RuntimeDeliveryInput::SettleDeliveryRecipient {
                delivery_id: record.submission.delivery_id().as_str().into(),
                delivery_sequence: record.sequence,
                recipient_id: recipient.id.clone(),
                target_binding: recipient.target_binding.clone(),
                outcome: outcome.into(),
            },
        )
        .await?;
        Ok(RuntimeDeliveryRecipientState {
            recipient: recipient.clone(),
            outcome: Some(outcome),
        })
    }

    /// Advance the ordered cursor only when every bound recipient has a
    /// completed local disposition. The generated owner derives the summary.
    /// An empty committed manifest is an all-applied zero-effect group.
    pub async fn finish_recipients(
        &self,
        runtime_id: &LogicalRuntimeId,
        record: &RuntimeDeliveryRecord,
    ) -> Result<RuntimeDeliveryRecipientGroupOutcome, RuntimeDeliveryError> {
        let state = self
            .apply_recipient_transition(
                runtime_id,
                record,
                dsl::RuntimeDeliveryInput::FinishDeliveryRecipients {
                    delivery_id: record.submission.delivery_id().as_str().into(),
                    delivery_sequence: record.sequence,
                },
            )
            .await?;
        state.group.ok_or_else(|| {
            RuntimeDeliveryError::Authority("recipient finish emitted no group outcome".into())
        })
    }

    async fn apply_recipient_transition(
        &self,
        runtime_id: &LogicalRuntimeId,
        record: &RuntimeDeliveryRecord,
        input: dsl::RuntimeDeliveryInput,
    ) -> Result<RecipientTransitionProjection, RuntimeDeliveryError> {
        for _ in 0..MAX_CAS_ATTEMPTS {
            let observed = self
                .store
                .load_runtime_delivery_authority(runtime_id)
                .await?
                .ok_or_else(|| {
                    RuntimeDeliveryError::Corrupt(
                        "recipient operation has no committed authority".into(),
                    )
                })?;
            let stored = self
                .store
                .load_runtime_delivery_record(runtime_id, record.submission.delivery_id().as_str())
                .await?
                .ok_or_else(|| {
                    RuntimeDeliveryError::Corrupt("recipient operation has no committed row".into())
                })?;
            if stored.sequence() != record.sequence
                || decode_submission(&stored)? != record.submission
            {
                return Err(RuntimeDeliveryError::IdempotencyConflict(
                    record.submission.delivery_id().clone(),
                ));
            }
            let mut authority = decode_authority(&observed)?;
            if authority
                .state()
                .delivery_source_sequences
                .get(record.submission.delivery_id().as_str())
                != Some(&record.submission.source_sequence())
            {
                return Err(RuntimeDeliveryError::Corrupt(
                    "recipient row disagrees with committed source sequence".into(),
                ));
            }
            let transition =
                dsl::RuntimeDeliveryMachineMutator::apply(&mut authority, input.clone())
                    .map_err(|error| RuntimeDeliveryError::Authority(format!("{error:?}")))?;
            if transition.effects().len() != 1
                || !matches!(
                    (&input, &transition.effects()[0]),
                    (
                        dsl::RuntimeDeliveryInput::BindDeliveryRecipients { .. },
                        dsl::RuntimeDeliveryEffect::DeliveryRecipientsBound { .. }
                    ) | (
                        dsl::RuntimeDeliveryInput::SettleDeliveryRecipient { .. },
                        dsl::RuntimeDeliveryEffect::DeliveryRecipientSettled { .. }
                    ) | (
                        dsl::RuntimeDeliveryInput::FinishDeliveryRecipients { .. },
                        dsl::RuntimeDeliveryEffect::DeliveryRecipientsSettled { .. }
                    )
                )
            {
                return Err(RuntimeDeliveryError::Authority(
                    "recipient operation emitted an unexpected effect".into(),
                ));
            }
            advance_acknowledged_prefix(&mut authority)?;
            let bytes = encode_authority(&authority)?;
            if bytes == observed.state_json() {
                return RecipientTransitionProjection::from_state(
                    authority.state(),
                    record.submission.delivery_id().as_str(),
                );
            }
            let replacement = RuntimeDeliveryAuthorityRecord::from_parts(
                next_revision(observed.revision())?,
                bytes,
            );
            match self
                .store
                .compare_and_swap_runtime_delivery_authority(
                    runtime_id,
                    Some(observed.revision()),
                    replacement,
                    None,
                )
                .await?
            {
                RuntimeDeliveryAuthorityCasOutcome::Applied(_) => {
                    return RecipientTransitionProjection::from_state(
                        authority.state(),
                        record.submission.delivery_id().as_str(),
                    );
                }
                RuntimeDeliveryAuthorityCasOutcome::Conflict(_) => continue,
            }
        }
        Err(RuntimeDeliveryError::Store(RuntimeStoreError::WriteFailed(
            format!(
                "runtime delivery recipient CAS did not converge after {MAX_CAS_ATTEMPTS} attempts"
            ),
        )))
    }

    pub async fn mark_applied(
        &self,
        runtime_id: &LogicalRuntimeId,
        delivery_id: &RuntimeDeliveryId,
        sequence: u64,
    ) -> Result<u64, RuntimeDeliveryError> {
        for _ in 0..MAX_CAS_ATTEMPTS {
            let observed = self
                .store
                .load_runtime_delivery_authority(runtime_id)
                .await?
                .ok_or_else(|| {
                    RuntimeDeliveryError::Corrupt(format!(
                        "runtime {runtime_id} has no delivery authority"
                    ))
                })?;
            let mut authority = decode_authority(&observed)?;
            let current = authority.state().applied_cursor;
            let expected_sequence = authority
                .state()
                .delivery_sequences
                .get(delivery_id.as_str())
                .copied()
                .ok_or_else(|| {
                    RuntimeDeliveryError::Corrupt(format!(
                        "generated authority has no delivery {delivery_id}"
                    ))
                })?;
            if expected_sequence != sequence {
                return Err(RuntimeDeliveryError::Corrupt(format!(
                    "delivery {delivery_id} sequence {sequence} disagrees with generated sequence {expected_sequence}"
                )));
            }
            if sequence > current && sequence - 1 != current {
                return Err(RuntimeDeliveryError::OutOfOrder {
                    delivery_id: delivery_id.clone(),
                    expected: current.saturating_add(1),
                    actual: sequence,
                });
            }

            let transition = dsl::RuntimeDeliveryMachineMutator::apply(
                &mut authority,
                dsl::RuntimeDeliveryInput::MarkDeliveryApplied {
                    delivery_id: delivery_id.as_str().to_string(),
                    delivery_sequence: sequence,
                },
            )
            .map_err(|error| RuntimeDeliveryError::Authority(format!("{error:?}")))?;
            classify_applied_effects(transition.effects(), delivery_id, sequence)?;
            advance_acknowledged_prefix(&mut authority)?;
            let applied_cursor = authority.state().applied_cursor;
            if applied_cursor == current {
                return Ok(applied_cursor);
            }

            let replacement = RuntimeDeliveryAuthorityRecord::from_parts(
                next_revision(observed.revision())?,
                encode_authority(&authority)?,
            );
            match self
                .store
                .compare_and_swap_runtime_delivery_authority(
                    runtime_id,
                    Some(observed.revision()),
                    replacement,
                    None,
                )
                .await?
            {
                RuntimeDeliveryAuthorityCasOutcome::Applied(_) => return Ok(applied_cursor),
                RuntimeDeliveryAuthorityCasOutcome::Conflict(_) => continue,
            }
        }
        Err(RuntimeDeliveryError::Store(RuntimeStoreError::WriteFailed(
            format!(
                "runtime delivery cursor CAS did not converge after {MAX_CAS_ATTEMPTS} attempts"
            ),
        )))
    }

    /// Settle the delivery at the cursor as refused for `reason`: a terminal
    /// policy outcome. The cursor passes it without applying it, so the rows
    /// after it proceed; repeating the settlement observes it. Only the row
    /// at the cursor can be refused, and never one whose effect already
    /// reached its runtime out of band.
    pub async fn mark_refused(
        &self,
        runtime_id: &LogicalRuntimeId,
        delivery_id: &RuntimeDeliveryId,
        sequence: u64,
        reason: RuntimeDeliveryRefusalReason,
    ) -> Result<u64, RuntimeDeliveryError> {
        for _ in 0..MAX_CAS_ATTEMPTS {
            let observed = self
                .store
                .load_runtime_delivery_authority(runtime_id)
                .await?
                .ok_or_else(|| {
                    RuntimeDeliveryError::Corrupt(format!(
                        "runtime {runtime_id} has no delivery authority"
                    ))
                })?;
            let mut authority = decode_authority(&observed)?;
            let current = authority.state().applied_cursor;
            let transition = dsl::RuntimeDeliveryMachineMutator::apply(
                &mut authority,
                dsl::RuntimeDeliveryInput::SettleRefusedDelivery {
                    delivery_id: delivery_id.as_str().to_string(),
                    delivery_sequence: sequence,
                    reason: reason.into(),
                },
            )
            .map_err(|error| RuntimeDeliveryError::Authority(format!("{error:?}")))?;
            let refused = transition
                .effects()
                .iter()
                .filter(|effect| {
                    matches!(
                        effect,
                        dsl::RuntimeDeliveryEffect::DeliveryRefused {
                            delivery_id: emitted_id,
                            delivery_sequence: emitted_sequence,
                            reason: emitted_reason,
                        } if emitted_id == delivery_id.as_str()
                            && *emitted_sequence == sequence
                            && RuntimeDeliveryRefusalReason::from(*emitted_reason) == reason
                    )
                })
                .count();
            if refused != 1 {
                return Err(RuntimeDeliveryError::Authority(format!(
                    "generated refusal emitted {refused} matching settlements"
                )));
            }
            advance_acknowledged_prefix(&mut authority)?;
            let applied_cursor = authority.state().applied_cursor;
            if applied_cursor == current {
                return Ok(applied_cursor);
            }

            let replacement = RuntimeDeliveryAuthorityRecord::from_parts(
                next_revision(observed.revision())?,
                encode_authority(&authority)?,
            );
            match self
                .store
                .compare_and_swap_runtime_delivery_authority(
                    runtime_id,
                    Some(observed.revision()),
                    replacement,
                    None,
                )
                .await?
            {
                RuntimeDeliveryAuthorityCasOutcome::Applied(_) => return Ok(applied_cursor),
                RuntimeDeliveryAuthorityCasOutcome::Conflict(_) => continue,
            }
        }
        Err(RuntimeDeliveryError::Store(RuntimeStoreError::WriteFailed(
            format!(
                "runtime delivery cursor CAS did not converge after {MAX_CAS_ATTEMPTS} attempts"
            ),
        )))
    }

    /// Record that `delivery_id`'s effect already reached its runtime by
    /// another path (for example a shell completion projection), without
    /// applying it through a sink.
    ///
    /// At the cursor this applies the row and then advances over any
    /// contiguous rows acknowledged earlier. Ahead of the cursor it records
    /// the acknowledgement and returns [`RuntimeDeliveryAcknowledgement::Recorded`]
    /// instead of failing out of order, so an acknowledgement that arrives
    /// before an earlier row is applied never wedges the runtime's queue. The
    /// cursor itself still only moves in sequence order.
    pub async fn acknowledge(
        &self,
        runtime_id: &LogicalRuntimeId,
        delivery_id: &RuntimeDeliveryId,
        sequence: u64,
    ) -> Result<RuntimeDeliveryAcknowledgement, RuntimeDeliveryError> {
        for _ in 0..MAX_CAS_ATTEMPTS {
            let observed = self
                .store
                .load_runtime_delivery_authority(runtime_id)
                .await?
                .ok_or_else(|| {
                    RuntimeDeliveryError::Corrupt(format!(
                        "runtime {runtime_id} has no delivery authority"
                    ))
                })?;
            let mut authority = decode_authority(&observed)?;
            let before = authority.state().clone();
            let transition = dsl::RuntimeDeliveryMachineMutator::apply(
                &mut authority,
                dsl::RuntimeDeliveryInput::AcknowledgeDelivery {
                    delivery_id: delivery_id.as_str().to_string(),
                    delivery_sequence: sequence,
                },
            )
            .map_err(|error| RuntimeDeliveryError::Authority(format!("{error:?}")))?;
            let recorded =
                classify_acknowledgement_effects(transition.effects(), delivery_id, sequence)?;
            advance_acknowledged_prefix(&mut authority)?;
            let state = authority.state();
            let applied_cursor = state.applied_cursor;
            let outcome = if recorded && applied_cursor < sequence {
                RuntimeDeliveryAcknowledgement::Recorded { applied_cursor }
            } else {
                RuntimeDeliveryAcknowledgement::Applied { applied_cursor }
            };
            if state.applied_cursor == before.applied_cursor
                && state.acknowledged_sequences == before.acknowledged_sequences
            {
                return Ok(outcome);
            }
            let replacement = RuntimeDeliveryAuthorityRecord::from_parts(
                next_revision(observed.revision())?,
                encode_authority(&authority)?,
            );
            match self
                .store
                .compare_and_swap_runtime_delivery_authority(
                    runtime_id,
                    Some(observed.revision()),
                    replacement,
                    None,
                )
                .await?
            {
                RuntimeDeliveryAuthorityCasOutcome::Applied(_) => return Ok(outcome),
                RuntimeDeliveryAuthorityCasOutcome::Conflict(_) => continue,
            }
        }
        Err(RuntimeDeliveryError::Store(RuntimeStoreError::WriteFailed(
            format!(
                "runtime delivery acknowledgement CAS did not converge after {MAX_CAS_ATTEMPTS} attempts"
            ),
        )))
    }

    /// The first binding of a continuation key, if any.
    pub async fn continuation_key_binding(
        &self,
        owner: &str,
        key: &str,
    ) -> Result<Option<crate::store::ContinuationKeyBinding>, RuntimeDeliveryError> {
        Ok(self.store.load_continuation_key_binding(owner, key).await?)
    }

    /// The admission recorded for one continuation delivery, if any.
    pub async fn continuation_admission(
        &self,
        address: &LogicalRuntimeId,
        delivery_id: &RuntimeDeliveryId,
    ) -> Result<Option<crate::store::ContinuationAdmission>, RuntimeDeliveryError> {
        Ok(self
            .store
            .load_continuation_admission(address, delivery_id.as_str())
            .await?)
    }

    /// Mint the request to resume retained work from the committed delivery
    /// `delivery_id` at `sequence`, which must be the unacknowledged row at
    /// the head of `runtime_id`'s inbox (the row the delivery owner is
    /// draining). The row is reread from the store: `identity_from_row`
    /// extracts the retained work from the stored submission bytes, and the
    /// request binds that exact committed submission. Nothing the caller
    /// holds about the row is trusted.
    pub async fn retained_resume_request(
        &self,
        runtime_id: &LogicalRuntimeId,
        delivery_id: &RuntimeDeliveryId,
        sequence: u64,
        identity_from_row: impl FnOnce(
            &RuntimeDeliverySubmission,
        ) -> Result<
            meerkat_core::retained_work::RetainedWorkIdentity,
            String,
        >,
    ) -> Result<crate::retained_work::RetainedResumeRequest, RuntimeDeliveryError> {
        let observed = self
            .store
            .load_runtime_delivery_authority(runtime_id)
            .await?;
        let authority = decode_or_new_authority(observed.as_ref())?;
        let state = authority.state();
        if state.delivery_sequences.get(delivery_id.as_str()) != Some(&sequence)
            || sequence != state.applied_cursor.saturating_add(1)
            || state.acknowledged_sequences.contains(&sequence)
            || state.refused_deliveries.contains_key(delivery_id.as_str())
        {
            return Err(RuntimeDeliveryError::Authority(format!(
                "delivery {delivery_id} is not the pending head of runtime {runtime_id}"
            )));
        }
        let head = self
            .list_pending(runtime_id, 1)
            .await?
            .into_iter()
            .next()
            .filter(|row| row.sequence == sequence && row.submission.delivery_id() == delivery_id)
            .ok_or_else(|| {
                RuntimeDeliveryError::Authority(format!(
                    "delivery {delivery_id} is not the committed head row"
                ))
            })?;
        let submission = &head.submission;
        let identity = identity_from_row(submission).map_err(RuntimeDeliveryError::Corrupt)?;
        let submission_digest = delivery_digest_hex(&[
            b"retained-resume-delivery",
            submission.delivery_id().as_str().as_bytes(),
            submission.source_id().as_bytes(),
            &submission.source_sequence().to_be_bytes(),
            submission.payload(),
        ]);
        Ok(crate::retained_work::RetainedResumeRequest {
            identity,
            delivery: crate::retained_work::RetainedResumeDelivery {
                address: runtime_id.clone(),
                delivery_id: delivery_id.clone(),
                delivery_sequence: sequence,
                submission_digest,
            },
        })
    }

    /// Change the admission index entry of one continuation delivery; see
    /// [`crate::store::ContinuationAdmissionTransition`].
    pub async fn transition_continuation_admission(
        &self,
        address: &LogicalRuntimeId,
        delivery_id: &RuntimeDeliveryId,
        transition: crate::store::ContinuationAdmissionTransition,
    ) -> Result<crate::store::ContinuationAdmissionOutcome, RuntimeDeliveryError> {
        Ok(self
            .store
            .transition_continuation_admission(address, delivery_id.as_str(), transition)
            .await?)
    }

    /// Commit a new delivery together with the first binding of its
    /// continuation key, in one store transaction.
    ///
    /// When the key is already bound nothing is written and the existing
    /// binding is returned. A new key never reuses a committed delivery id:
    /// ids are derived from the first-bound address and the key, so a
    /// committed row without its binding is corruption.
    pub async fn submit_with_key_claim(
        &self,
        runtime_id: &LogicalRuntimeId,
        submission: RuntimeDeliverySubmission,
        claim: ContinuationKeyClaim,
    ) -> Result<KeyedSubmitOutcome, RuntimeDeliveryError> {
        for _ in 0..MAX_CAS_ATTEMPTS {
            if let Some(existing) = self
                .store
                .load_continuation_key_binding(&claim.owner, &claim.key)
                .await?
            {
                return Ok(KeyedSubmitOutcome::AlreadyBound(existing));
            }
            let observed = self
                .store
                .load_runtime_delivery_authority(runtime_id)
                .await?;
            let mut authority = decode_or_new_authority(observed.as_ref())?;
            let delivery_id = submission.delivery_id.clone();
            let transition = dsl::RuntimeDeliveryMachineMutator::apply(
                &mut authority,
                dsl::RuntimeDeliveryInput::CommitDelivery {
                    delivery_id: delivery_id.as_str().to_string(),
                    source_sequence: submission.source_sequence,
                },
            )
            .map_err(|error| RuntimeDeliveryError::Authority(format!("{error:?}")))?;
            let (sequence, deduplicated) = classify_commit_effects(
                transition.effects(),
                &delivery_id,
                submission.source_sequence,
            )?;
            if deduplicated {
                return Err(RuntimeDeliveryError::Corrupt(format!(
                    "continuation delivery {delivery_id} is committed without its key binding"
                )));
            }
            let next_revision = observed
                .as_ref()
                .map_or(Ok(1), |record| next_revision(record.revision()))?;
            let replacement = RuntimeDeliveryAuthorityRecord::from_parts(
                next_revision,
                encode_authority(&authority)?,
            );
            let inserted = RuntimeDeliveryStoreRecord::from_parts(
                delivery_id.as_str(),
                sequence,
                encode_submission(&submission)?,
            );
            let binding = crate::store::ContinuationKeyBinding {
                owner: claim.owner.clone(),
                key: claim.key.clone(),
                address: runtime_id.clone(),
                delivery_id: delivery_id.as_str().to_string(),
                submission_digest: claim.submission_digest.clone(),
                committed_at_ms: claim.committed_at_ms,
            };
            match self
                .store
                .compare_and_swap_runtime_delivery_authority_with_key_binding(
                    runtime_id,
                    observed
                        .as_ref()
                        .map(RuntimeDeliveryAuthorityRecord::revision),
                    replacement,
                    inserted,
                    binding.clone(),
                )
                .await?
            {
                crate::store::KeyedRuntimeDeliveryCasOutcome::Applied(_) => {
                    self.record_committed_runtime(runtime_id);
                    return Ok(KeyedSubmitOutcome::Committed {
                        receipt: RuntimeDeliveryReceipt {
                            delivery_id,
                            sequence,
                            deduplicated: false,
                        },
                        binding,
                    });
                }
                crate::store::KeyedRuntimeDeliveryCasOutcome::KeyAlreadyBound(existing) => {
                    return Ok(KeyedSubmitOutcome::AlreadyBound(existing));
                }
                crate::store::KeyedRuntimeDeliveryCasOutcome::Conflict(_) => continue,
            }
        }
        Err(RuntimeDeliveryError::Store(RuntimeStoreError::WriteFailed(
            format!(
                "keyed runtime delivery CAS did not converge after {MAX_CAS_ATTEMPTS} attempts"
            ),
        )))
    }

    /// Classify one delivery id against this runtime's durable authority.
    ///
    /// A store read only: it never consults live state and never writes. A
    /// store error is an error, never `NotCommitted`.
    pub async fn delivery_status(
        &self,
        runtime_id: &LogicalRuntimeId,
        delivery_id: &RuntimeDeliveryId,
    ) -> Result<RuntimeDeliveryStatus, RuntimeDeliveryError> {
        let observed = self
            .store
            .load_runtime_delivery_authority(runtime_id)
            .await?;
        let mut authority = decode_or_new_authority(observed.as_ref())?;
        let transition = dsl::RuntimeDeliveryMachineMutator::apply(
            &mut authority,
            dsl::RuntimeDeliveryInput::ClassifyDeliveryStatus {
                delivery_id: delivery_id.as_str().to_string(),
            },
        )
        .map_err(|error| RuntimeDeliveryError::Authority(format!("{error:?}")))?;
        let mut verdicts = transition
            .effects()
            .iter()
            .filter_map(|effect| match effect {
                dsl::RuntimeDeliveryEffect::DeliveryStatusNotCommitted { delivery_id: id }
                    if id == delivery_id.as_str() =>
                {
                    Some(RuntimeDeliveryStatus::NotCommitted)
                }
                dsl::RuntimeDeliveryEffect::DeliveryStatusPending {
                    delivery_id: id,
                    delivery_sequence,
                } if id == delivery_id.as_str() => Some(RuntimeDeliveryStatus::Pending {
                    delivery_sequence: *delivery_sequence,
                }),
                dsl::RuntimeDeliveryEffect::DeliveryStatusAcknowledgedAhead {
                    delivery_id: id,
                    delivery_sequence,
                } if id == delivery_id.as_str() => Some(RuntimeDeliveryStatus::AcknowledgedAhead {
                    delivery_sequence: *delivery_sequence,
                }),
                dsl::RuntimeDeliveryEffect::DeliveryStatusApplied {
                    delivery_id: id,
                    delivery_sequence,
                } if id == delivery_id.as_str() => Some(RuntimeDeliveryStatus::Applied {
                    delivery_sequence: *delivery_sequence,
                }),
                dsl::RuntimeDeliveryEffect::DeliveryStatusMixed {
                    delivery_id: id,
                    delivery_sequence,
                    bindings,
                    outcomes,
                } if id == delivery_id.as_str() => Some(RuntimeDeliveryStatus::Mixed {
                    delivery_sequence: *delivery_sequence,
                    // The canonical group invariants require an outcome for
                    // every bound target. Project only this delivery's maps.
                    recipients: bindings
                        .iter()
                        .map(|(recipient, target)| RuntimeDeliveryRecipientState {
                            recipient: RuntimeDeliveryRecipient {
                                id: recipient.clone(),
                                target_binding: target.clone(),
                            },
                            outcome: outcomes.get(recipient).copied().map(Into::into),
                        })
                        .collect(),
                }),
                dsl::RuntimeDeliveryEffect::DeliveryStatusRefused {
                    delivery_id: id,
                    delivery_sequence,
                    reason,
                } if id == delivery_id.as_str() => Some(RuntimeDeliveryStatus::Refused {
                    delivery_sequence: *delivery_sequence,
                    reason: (*reason).into(),
                }),
                _ => None,
            });
        let verdict = verdicts.next().ok_or_else(|| {
            RuntimeDeliveryError::Authority("generated classification emitted no verdict".into())
        })?;
        if verdicts.next().is_some() {
            return Err(RuntimeDeliveryError::Authority(
                "generated classification emitted more than one verdict".into(),
            ));
        }
        Ok(verdict)
    }

    /// Sequences of this runtime's pending rows that were acknowledged out of
    /// band and await the cursor. An applier marks such a row applied without
    /// re-running its sink.
    pub async fn acknowledged_pending_sequences(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<std::collections::BTreeSet<u64>, RuntimeDeliveryError> {
        let observed = self
            .store
            .load_runtime_delivery_authority(runtime_id)
            .await?;
        Ok(decode_or_new_authority(observed.as_ref())?
            .state()
            .acknowledged_sequences
            .clone())
    }

    /// Total committed-but-unapplied deliveries across every runtime in this
    /// store.
    ///
    /// Scope is the durable runtime store, which is one file per realm ROOT.
    /// One such store serves every session the host built, including sessions
    /// built under another logical realm id (mob members build under
    /// `mob.<mob_id>`), and no runtime id carries a realm. So this is a
    /// host-store total and cannot be narrowed to a logical realm; a caller
    /// that needs a realm-scoped answer does not have one available here and
    /// must not pretend otherwise. Over-inclusion is the safe direction: every
    /// counted row is a real undrained delivery in this host.
    ///
    /// Uncapped by construction: the population is one row per runtime that
    /// ever received a delivery, and a cap would hide exactly the backlog the
    /// caller is asking about.
    pub async fn pending_delivery_total(&self) -> Result<u64, RuntimeDeliveryError> {
        let authorities = self.store.list_runtime_delivery_authorities().await?;
        let mut total = 0_u64;
        for (runtime_id, record) in authorities {
            total = total.saturating_add(pending_delivery_count(&runtime_id, &record)?);
        }
        Ok(total)
    }

    /// Every runtime holding committed-but-unapplied deliveries, read from
    /// the delivery authority itself.
    ///
    /// This is the population a drain must visit. Deriving it from another
    /// record set (for example job rows read through a bounded window) misses
    /// runtimes whose producers aged out of that window while their rows stay
    /// pending. Uncapped for the same reason as
    /// [`Self::pending_delivery_total`]; ordered by runtime id.
    pub async fn runtimes_with_pending_deliveries(
        &self,
    ) -> Result<Vec<LogicalRuntimeId>, RuntimeDeliveryError> {
        let authorities = self.store.list_runtime_delivery_authorities().await?;
        let mut runtimes = Vec::new();
        for (runtime_id, record) in authorities {
            if pending_delivery_count(&runtime_id, &record)? > 0 {
                runtimes.push(runtime_id);
            }
        }
        runtimes.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(runtimes)
    }

    pub async fn applied_cursor(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<u64, RuntimeDeliveryError> {
        let observed = self
            .store
            .load_runtime_delivery_authority(runtime_id)
            .await?;
        Ok(decode_or_new_authority(observed.as_ref())?
            .state()
            .applied_cursor)
    }
}

fn validate_component(label: &str, value: String) -> Result<String, RuntimeDeliveryError> {
    validate_component_ref(label, &value)?;
    Ok(value)
}

fn validate_component_ref(label: &str, value: &str) -> Result<(), RuntimeDeliveryError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed != value || trimmed.chars().any(char::is_control) {
        return Err(RuntimeDeliveryError::InvalidInput(format!(
            "{label} must be non-empty, canonical, and contain no control characters"
        )));
    }
    Ok(())
}

fn next_revision(current: u64) -> Result<u64, RuntimeDeliveryError> {
    current.checked_add(1).ok_or_else(|| {
        RuntimeDeliveryError::Store(RuntimeStoreError::WriteFailed(
            "runtime delivery authority revision exhausted u64".into(),
        ))
    })
}

fn encode_authority(
    authority: &dsl::RuntimeDeliveryMachineAuthority,
) -> Result<Vec<u8>, RuntimeDeliveryError> {
    let state = PersistedAuthorityState::from(authority.state());
    let version = if state.recipients.is_some() {
        RECIPIENT_AUTHORITY_ENVELOPE_VERSION
    } else if state.refused_deliveries.is_empty() {
        AUTHORITY_ENVELOPE_VERSION
    } else {
        REFUSAL_AUTHORITY_ENVELOPE_VERSION
    };
    serde_json::to_vec(&AuthorityEnvelope { version, state })
        .map_err(|error| RuntimeDeliveryError::Corrupt(error.to_string()))
}

/// Committed-but-unapplied delivery count for one runtime's authority.
///
/// The machine declares `applied_cursor <= next_sequence`. A store that
/// violates it is corrupt, and a saturating subtraction would answer 0,
/// substituting "nothing pending" for "this file is broken", which is the one
/// answer that must never be fabricated.
fn pending_delivery_count(
    runtime_id: &LogicalRuntimeId,
    record: &RuntimeDeliveryAuthorityRecord,
) -> Result<u64, RuntimeDeliveryError> {
    let authority = decode_authority(record)?;
    let state = authority.state();
    state
        .next_sequence
        .checked_sub(state.applied_cursor)
        .ok_or_else(|| {
            RuntimeDeliveryError::Corrupt(format!(
                "runtime {runtime_id} applied cursor {} is ahead of committed sequence {}",
                state.applied_cursor, state.next_sequence
            ))
        })
}

fn decode_authority(
    record: &RuntimeDeliveryAuthorityRecord,
) -> Result<dsl::RuntimeDeliveryMachineAuthority, RuntimeDeliveryError> {
    let envelope: AuthorityEnvelope = serde_json::from_slice(record.state_json())
        .map_err(|error| RuntimeDeliveryError::Corrupt(error.to_string()))?;
    if envelope.version < RECIPIENT_AUTHORITY_ENVELOPE_VERSION
        && envelope.state.recipients.is_some()
    {
        return Err(RuntimeDeliveryError::Corrupt(
            "legacy runtime delivery authority carries recipient state".into(),
        ));
    }
    if envelope.version == RECIPIENT_AUTHORITY_ENVELOPE_VERSION
        && envelope.state.recipients.is_none()
    {
        return Err(RuntimeDeliveryError::Corrupt(
            "runtime delivery authority version 3 is missing recipient state".into(),
        ));
    }
    match envelope.version {
        AUTHORITY_ENVELOPE_VERSION if envelope.state.refused_deliveries.is_empty() => {}
        AUTHORITY_ENVELOPE_VERSION => {
            return Err(RuntimeDeliveryError::Corrupt(
                "runtime delivery authority envelope version 1 carries refused settlements".into(),
            ));
        }
        REFUSAL_AUTHORITY_ENVELOPE_VERSION | RECIPIENT_AUTHORITY_ENVELOPE_VERSION => {}
        version => {
            return Err(RuntimeDeliveryError::Corrupt(format!(
                "unsupported runtime delivery authority envelope version {version}"
            )));
        }
    }
    envelope.state.validate()?;
    dsl::RuntimeDeliveryMachineAuthority::recover_from_state(envelope.state.into())
        .map_err(|error| RuntimeDeliveryError::Corrupt(format!("{error:?}")))
}

fn decode_or_new_authority(
    record: Option<&RuntimeDeliveryAuthorityRecord>,
) -> Result<dsl::RuntimeDeliveryMachineAuthority, RuntimeDeliveryError> {
    match record {
        Some(record) => decode_authority(record),
        None => Ok(dsl::RuntimeDeliveryMachineAuthority::new()),
    }
}

fn encode_submission(
    submission: &RuntimeDeliverySubmission,
) -> Result<Vec<u8>, RuntimeDeliveryError> {
    serde_json::to_vec(&SubmissionEnvelope {
        version: SUBMISSION_ENVELOPE_VERSION,
        submission: submission.clone(),
    })
    .map_err(|error| RuntimeDeliveryError::Corrupt(error.to_string()))
}

fn decode_submission(
    record: &RuntimeDeliveryStoreRecord,
) -> Result<RuntimeDeliverySubmission, RuntimeDeliveryError> {
    let envelope: SubmissionEnvelope = serde_json::from_slice(record.submission_json())
        .map_err(|error| RuntimeDeliveryError::Corrupt(error.to_string()))?;
    if envelope.version != SUBMISSION_ENVELOPE_VERSION {
        return Err(RuntimeDeliveryError::Corrupt(format!(
            "unsupported runtime delivery submission envelope version {}",
            envelope.version
        )));
    }
    if envelope.submission.delivery_id.as_str() != record.delivery_id() {
        return Err(RuntimeDeliveryError::Corrupt(format!(
            "runtime delivery row key {} disagrees with payload key {}",
            record.delivery_id(),
            envelope.submission.delivery_id
        )));
    }
    envelope
        .submission
        .validate()
        .map_err(|error| RuntimeDeliveryError::Corrupt(error.to_string()))?;
    Ok(envelope.submission)
}

fn classify_commit_effects(
    effects: &[dsl::RuntimeDeliveryEffect],
    delivery_id: &RuntimeDeliveryId,
    source_sequence: u64,
) -> Result<(u64, bool), RuntimeDeliveryError> {
    // The same delivery id was committed with another source sequence: the
    // generated machine refuses it with a typed verdict and the existing row
    // stands.
    if effects.iter().any(|effect| {
        matches!(
            effect,
            dsl::RuntimeDeliveryEffect::CommitRejectedSourceSequenceConflict {
                delivery_id: emitted_id,
                ..
            } if emitted_id == delivery_id.as_str()
        )
    }) {
        return Err(RuntimeDeliveryError::IdempotencyConflict(
            delivery_id.clone(),
        ));
    }
    let mut matching = effects.iter().filter_map(|effect| match effect {
        dsl::RuntimeDeliveryEffect::DeliveryCommitted {
            delivery_id: emitted_id,
            source_sequence: emitted_source_sequence,
            delivery_sequence,
        } if emitted_id == delivery_id.as_str() && *emitted_source_sequence == source_sequence => {
            Some((*delivery_sequence, false))
        }
        dsl::RuntimeDeliveryEffect::DeliveryReused {
            delivery_id: emitted_id,
            source_sequence: emitted_source_sequence,
            delivery_sequence,
        } if emitted_id == delivery_id.as_str() && *emitted_source_sequence == source_sequence => {
            Some((*delivery_sequence, true))
        }
        _ => None,
    });
    let first = matching.next().ok_or_else(|| {
        RuntimeDeliveryError::Authority(
            "generated commit emitted no matching delivery acknowledgement".into(),
        )
    })?;
    if matching.next().is_some() {
        return Err(RuntimeDeliveryError::Authority(
            "generated commit emitted multiple delivery acknowledgements".into(),
        ));
    }
    Ok(first)
}

/// Drive `AdvanceAcknowledgedPrefix` until the generated machine reports the
/// prefix at rest: each advance either carries the cursor over the next
/// sequence that was acknowledged out of band, or is the typed no-op. The
/// machine decides which; exactly one matching effect is required.
fn advance_acknowledged_prefix(
    authority: &mut dsl::RuntimeDeliveryMachineAuthority,
) -> Result<(), RuntimeDeliveryError> {
    loop {
        let cursor = authority.state().applied_cursor;
        let transition = dsl::RuntimeDeliveryMachineMutator::apply(
            authority,
            dsl::RuntimeDeliveryInput::AdvanceAcknowledgedPrefix {},
        )
        .map_err(|error| RuntimeDeliveryError::Authority(format!("{error:?}")))?;
        let mut advanced = 0usize;
        let mut at_rest = 0usize;
        for effect in transition.effects() {
            match effect {
                dsl::RuntimeDeliveryEffect::AcknowledgedPrefixAdvanced { delivery_sequence }
                    if cursor.checked_add(1) == Some(*delivery_sequence) =>
                {
                    advanced += 1;
                }
                dsl::RuntimeDeliveryEffect::AcknowledgedPrefixAtRest { applied_cursor }
                    if *applied_cursor == cursor =>
                {
                    at_rest += 1;
                }
                _ => {}
            }
        }
        match (advanced, at_rest) {
            (1, 0) => {}
            (0, 1) => return Ok(()),
            _ => {
                return Err(RuntimeDeliveryError::Authority(format!(
                    "generated prefix advance at cursor {cursor} emitted {advanced} advances and {at_rest} at-rest reports"
                )));
            }
        }
    }
}

/// `true` when the acknowledgement was recorded ahead of the cursor, `false`
/// when it applied (or had already applied) the row. Exactly one matching
/// effect is required.
fn classify_acknowledgement_effects(
    effects: &[dsl::RuntimeDeliveryEffect],
    delivery_id: &RuntimeDeliveryId,
    sequence: u64,
) -> Result<bool, RuntimeDeliveryError> {
    let mut recorded = 0;
    let mut applied = 0;
    for effect in effects {
        match effect {
            dsl::RuntimeDeliveryEffect::DeliveryAcknowledged {
                delivery_id: emitted_id,
                delivery_sequence: emitted_sequence,
            } if emitted_id == delivery_id.as_str() && *emitted_sequence == sequence => {
                recorded += 1;
            }
            dsl::RuntimeDeliveryEffect::DeliveryApplied {
                delivery_id: emitted_id,
                delivery_sequence: emitted_sequence,
            } if emitted_id == delivery_id.as_str() && *emitted_sequence == sequence => {
                applied += 1;
            }
            _ => {}
        }
    }
    match (recorded, applied) {
        (1, 0) => Ok(true),
        (0, 1) => Ok(false),
        _ => Err(RuntimeDeliveryError::Authority(format!(
            "generated acknowledgement emitted {recorded} recorded and {applied} applied verdicts"
        ))),
    }
}

fn classify_applied_effects(
    effects: &[dsl::RuntimeDeliveryEffect],
    delivery_id: &RuntimeDeliveryId,
    sequence: u64,
) -> Result<(), RuntimeDeliveryError> {
    let count = effects
        .iter()
        .filter(|effect| {
            matches!(
                effect,
                dsl::RuntimeDeliveryEffect::DeliveryApplied {
                    delivery_id: emitted_id,
                    delivery_sequence: emitted_sequence,
                } if emitted_id == delivery_id.as_str() && *emitted_sequence == sequence
            )
        })
        .count();
    if count != 1 {
        return Err(RuntimeDeliveryError::Authority(format!(
            "generated apply emitted {count} matching acknowledgements"
        )));
    }
    Ok(())
}
