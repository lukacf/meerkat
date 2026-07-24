//! Runtime-owned durable delivery inbox.
//!
//! `RuntimeDeliveryMachine` is the semantic authority for idempotent sequence
//! assignment and ordered application. `RuntimeStore` implementations retain
//! its exact state with CAS and atomically insert opaque inbox rows.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::identifiers::LogicalRuntimeId;
use crate::store::{
    RuntimeDeliveryAuthorityCasOutcome, RuntimeDeliveryAuthorityRecord, RuntimeDeliveryStoreRecord,
    RuntimeStore, RuntimeStoreError,
};

pub mod dsl;

const AUTHORITY_ENVELOPE_VERSION: u16 = 1;
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
        }
    }
}

impl From<PersistedAuthorityState> for dsl::RuntimeDeliveryMachineState {
    fn from(state: PersistedAuthorityState) -> Self {
        Self {
            lifecycle_phase: dsl::RuntimeDeliveryPhase::Active,
            delivery_ids: state.delivery_ids,
            delivery_sequences: state.delivery_sequences,
            delivery_source_sequences: state.delivery_source_sequences,
            committed_sequences: state.committed_sequences,
            next_sequence: state.next_sequence,
            applied_cursor: state.applied_cursor,
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
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SubmissionEnvelope {
    version: u16,
    submission: RuntimeDeliverySubmission,
}

#[derive(Clone)]
pub struct RuntimeDeliveryInbox {
    store: Arc<dyn RuntimeStore>,
}

impl std::fmt::Debug for RuntimeDeliveryInbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeDeliveryInbox")
            .finish_non_exhaustive()
    }
}

impl RuntimeDeliveryInbox {
    pub fn new(store: Arc<dyn RuntimeStore>) -> Self {
        Self { store }
    }

    pub async fn submit(
        &self,
        runtime_id: &LogicalRuntimeId,
        submission: RuntimeDeliverySubmission,
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
                return Ok(RuntimeDeliveryReceipt {
                    delivery_id,
                    sequence,
                    deduplicated: true,
                });
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
    serde_json::to_vec(&AuthorityEnvelope {
        version: AUTHORITY_ENVELOPE_VERSION,
        state: PersistedAuthorityState::from(authority.state()),
    })
    .map_err(|error| RuntimeDeliveryError::Corrupt(error.to_string()))
}

fn decode_authority(
    record: &RuntimeDeliveryAuthorityRecord,
) -> Result<dsl::RuntimeDeliveryMachineAuthority, RuntimeDeliveryError> {
    let envelope: AuthorityEnvelope = serde_json::from_slice(record.state_json())
        .map_err(|error| RuntimeDeliveryError::Corrupt(error.to_string()))?;
    if envelope.version != AUTHORITY_ENVELOPE_VERSION {
        return Err(RuntimeDeliveryError::Corrupt(format!(
            "unsupported runtime delivery authority envelope version {}",
            envelope.version
        )));
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
