//! Prepared head/event/source persistence handoff.
//!
//! The prepared carrier has no public constructor or deserializer. Generated
//! Live authority will produce it; stores only verify and persist its exact
//! successor. No handwritten lifecycle reducer lives at this boundary.

use std::sync::Arc;

use meerkat_core::SessionId;
use sha2::{Digest, Sha256};

use super::record::LiveLedgerRecord;
use super::source::{LiveSourceChargeDelta, PreparedLiveSourceMutation};
use super::transcript::{LiveHeadReference, LiveLedgerFormatV1, LiveLedgerPrefixDigest};
use crate::live_resources::LiveResourceCharge;
#[cfg(not(target_arch = "wasm32"))]
use crate::store::MachineLifecycleObservationVersion;
use crate::store::{MachineLifecycleExpectedVersion, RuntimeSessionAuthority, RuntimeStoreError};

#[cfg(not(target_arch = "wasm32"))]
#[path = "write/transcript.rs"]
pub(super) mod transcript_commit;

/// Seed only the reservation prerequisite for external native-owner tests.
/// Admission, staging, physical claims, and feedback still use production APIs.
#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "test-support")))]
#[doc(hidden)]
pub async fn seed_live_reservation_for_test(
    store: &dyn crate::store::RuntimeStore,
    grant: &crate::live_grant::LiveExecutionGrant<()>,
    row: super::source::LiveSourceRow,
    fence: Arc<dyn crate::store::RuntimeStoreWriteFence>,
) -> Result<(), RuntimeStoreError> {
    use super::authority::dsl;
    let invalid = |error: &dyn std::fmt::Display| RuntimeStoreError::WriteFailed(error.to_string());
    let crate::live_source::LiveSourceEntryRecord::Reservation { record } = row.record()? else {
        return Err(RuntimeStoreError::WriteFailed(
            "fixture requires a reservation".into(),
        ));
    };
    let session_id = row.source().session_id();
    if session_id != &grant.record().executor().binding.session_id {
        return Err(RuntimeStoreError::WriteFailed(
            "fixture grant session mismatch".into(),
        ));
    }
    let ops = store
        .live_ledger_ops()
        .ok_or_else(|| RuntimeStoreError::Unsupported("fixture Live ledger".into()))?;
    let before = ops
        .load_live_head(session_id)
        .await?
        .ok_or_else(|| RuntimeStoreError::WriteFailed("fixture grant head missing".into()))?;
    let owner = dsl::LiveRequestMachineAuthority::recover_from_state(
        crate::generated::live_request_state::decode(&before.payload.request_snapshot)
            .map_err(|error| invalid(&error))?,
    )
    .map_err(|error| invalid(&error))?;
    let mut candidate = owner.prepare_authority();
    let grant_ref = grant.record().grant_ref();
    let budget = super::authority::store::request_credits::RequestCompletionBudget::measured()?;
    dsl::LiveRequestMachineMutator::apply(
        &mut candidate,
        dsl::LiveRequestInput::Reserve {
            content_complete: true,
            content_discontinuous: false,
            content_empty: false,
            content_fits: true,
            request_id: record.request_id().to_string(),
            source: serde_json::to_string(record.source()).map_err(|error| invalid(&error))?,
            payload: serde_json::to_string(
                &record.frozen_digest().map_err(|error| invalid(&error))?,
            )
            .map_err(|error| invalid(&error))?,
            evidence: record
                .frozen_request()
                .ok_or_else(|| invalid(&"fixture evidence missing"))?
                .kind(),
            profile_revision: serde_json::to_string(&grant.record().declaration().profile_revision)
                .map_err(|error| invalid(&error))?,
            parent_scope: String::new(),
            grant_id: grant_ref.id.as_uuid().to_string(),
            generation: grant_ref.generation.get(),
            executor: serde_json::to_string(&grant.record().executor().binding)
                .map_err(|error| invalid(&error))?,
            now: u64::try_from(chrono::Utc::now().timestamp_millis())
                .map_err(|error| invalid(&error))?,
            credit_records: budget.envelope.total().records,
            credit_bytes: budget.envelope.total().encoded_bytes,
            snapshot_ceiling: budget.snapshot_ceiling,
        },
    )
    .map_err(|error| invalid(&error))?;
    let mut prepared =
        PreparedLiveLedgerCommit::from_request_transition(session_id, Some(&before), &candidate)?;
    prepared.expected_actor = store
        .load_session_boundary_authority(&crate::LogicalRuntimeId::for_session(session_id))
        .await?;
    prepared.successor.payload.used = prepared
        .successor
        .payload
        .used
        .checked_add(row.charge()?)
        .map_err(|error| invalid(&error))?;
    prepared.sources.push(PreparedLiveSourceMutation {
        expected: None,
        replacement: row,
    });
    match ops.commit_live_ledger(prepared, fence).await? {
        LiveLedgerCommitOutcome::Committed { .. } => Ok(()),
        outcome => Err(RuntimeStoreError::WriteFailed(format!(
            "fixture reservation not committed: {outcome:?}"
        ))),
    }
}

/// Fixed head keys, counters, digest, and row/index custody use the same
/// conservative bookkeeping allowance as an event, plus the exact last-commit
/// digest. Snapshot bytes are extra.
pub const LIVE_HEAD_STORAGE_ALLOWANCE_BYTES: u64 =
    crate::live_resources::LIVE_RECORD_STORAGE_ALLOWANCE_BYTES + 32;

#[derive(Clone, PartialEq, Eq)]
pub struct LiveLedgerPayloadState {
    pub used: LiveResourceCharge,
    pub reserved: LiveResourceCharge,
    pub ingress_generation: u64,
    pub transcript_snapshot: Arc<Vec<u8>>,
    pub request_snapshot: Arc<Vec<u8>>,
}

impl std::fmt::Debug for LiveLedgerPayloadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveLedgerPayloadState")
            .field("used", &self.used)
            .field("reserved", &self.reserved)
            .field("ingress_generation", &self.ingress_generation)
            .field("transcript_snapshot_bytes", &self.transcript_snapshot.len())
            .field("request_snapshot_bytes", &self.request_snapshot.len())
            .finish()
    }
}

impl LiveLedgerPayloadState {
    fn snapshot_bytes(&self) -> Result<u64, RuntimeStoreError> {
        (self.transcript_snapshot.len() as u64)
            .checked_add(self.request_snapshot.len() as u64)
            .ok_or_else(|| {
                RuntimeStoreError::WriteFailed("live snapshot byte count overflow".into())
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveLedgerStoredHead {
    pub reference: LiveHeadReference,
    pub payload: LiveLedgerPayloadState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiveLedgerCommitDigest(pub [u8; 32]);

#[derive(Debug, Clone)]
pub(crate) struct StoredLiveLedgerCommit {
    pub head: LiveLedgerStoredHead,
    pub operation: LiveLedgerCommitDigest,
}

impl LiveLedgerStoredHead {
    pub(crate) fn validate_payload(&self) -> Result<(), RuntimeStoreError> {
        let invalid =
            || RuntimeStoreError::ReadFailed("invalid live head accounting or format".into());
        let total = self
            .payload
            .used
            .checked_add(self.payload.reserved)
            .map_err(|_| invalid())?;
        let minimum_bytes = self
            .payload
            .snapshot_bytes()?
            .checked_add(LIVE_HEAD_STORAGE_ALLOWANCE_BYTES)
            .ok_or_else(invalid)?;
        if self.reference.format != LiveLedgerFormatV1::V1
            || self.reference.generation == 0
            || self.reference.revision == 0
            || self.payload.ingress_generation == 0
            || total.records > crate::live_resources::LIVE_LEDGER_MAX_CHARGE.records
            || total.encoded_bytes > crate::live_resources::LIVE_LEDGER_MAX_CHARGE.encoded_bytes
            || self.payload.used.records < self.reference.event_count
            || self.payload.used.encoded_bytes < minimum_bytes
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Canonical changes can reach a store only through a prepared owner handoff,
/// not through fields supplied by a surface or deserialized source record.
///
/// ```compile_fail
/// use meerkat_runtime::live_ledger::write::PreparedLiveLedgerCommit;
/// let forged = serde_json::from_str::<PreparedLiveLedgerCommit>("{}");
/// ```
pub struct PreparedLiveLedgerCommit {
    purpose: LiveLedgerWritePurpose,
    expected: Option<LiveHeadReference>,
    expected_actor: Option<RuntimeSessionAuthority>,
    expected_lifecycle: Option<MachineLifecycleExpectedVersion>,
    successor: LiveLedgerStoredHead,
    records: Vec<LiveLedgerRecord>,
    sources: Vec<PreparedLiveSourceMutation>,
    input_admission: Option<crate::input_state::InputStatePersistenceRecord>,
    input_stage: Option<Box<LiveInputStageMutation>>,
    input_read_fences: Vec<LiveInputReadFence>,
    quota: LiveResourceCharge,
}

/// Read-only storage operation class, not a constructor for prepared authority.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub enum LiveLedgerWritePurpose {
    ComponentMutation,
    ArchiveIngressFence,
}

/// An exact ordinary-input comparison, not an input mutation or effect permit.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LiveInputReadFence {
    input_id: meerkat_core::lifecycle::InputId,
    expected_row_digest: String,
    #[cfg(not(target_arch = "wasm32"))]
    purpose: LiveInputReadFencePurpose,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum LiveInputReadFencePurpose {
    ActiveExecution,
    FinalizedCompletion,
    CallbackAdmission,
}

impl LiveInputReadFence {
    pub fn input_id(&self) -> &meerkat_core::lifecycle::InputId {
        &self.input_id
    }

    pub fn expected_row_digest(&self) -> &str {
        &self.expected_row_digest
    }
}

pub struct LiveInputStageMutation {
    input: crate::input_state::InputStatePersistenceRecord,
    lifecycle: crate::store::MachineLifecycleCommit,
}

impl LiveInputStageMutation {
    pub fn input(&self) -> &crate::input_state::InputStatePersistenceRecord {
        &self.input
    }

    pub fn lifecycle(&self) -> &crate::store::MachineLifecycleCommit {
        &self.lifecycle
    }
}

impl PreparedLiveLedgerCommit {
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn from_request_transition(
        session_id: &SessionId,
        before: Option<&LiveLedgerStoredHead>,
        prepared: &super::authority::dsl::LiveRequestMachinePreparedAuthority,
    ) -> Result<Self, RuntimeStoreError> {
        Self::from_request_transition_with_completions(session_id, before, prepared, Vec::new())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn from_request_transition_with_completions(
        session_id: &SessionId,
        before: Option<&LiveLedgerStoredHead>,
        prepared: &super::authority::dsl::LiveRequestMachinePreparedAuthority,
        completions: Vec<super::completion::LiveCompletionRecord>,
    ) -> Result<Self, RuntimeStoreError> {
        let snapshot = crate::generated::live_request_state::encode(prepared.state())
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        let previous_state = match before {
            Some(head) if !head.payload.request_snapshot.is_empty() => {
                let state =
                    crate::generated::live_request_state::decode(&head.payload.request_snapshot)
                        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                Some(state)
            }
            _ => None,
        };
        let previous_effect_charge = previous_state
            .as_ref()
            .map(super::authority::store::effect_credits::reserved_completion_charge)
            .transpose()?
            .unwrap_or_default();
        let previous_cancellation_charge = previous_state
            .as_ref()
            .map(super::authority::store::cancellation::reserved_charge)
            .transpose()?
            .unwrap_or_default();
        let changed_claims: std::collections::BTreeSet<_> = prepared
            .state()
            .claim_credit_spent_records
            .iter()
            .filter(|(claim, spent)| {
                let old = previous_state
                    .as_ref()
                    .and_then(|state| state.claim_credit_spent_records.get(*claim))
                    .copied()
                    .unwrap_or_default();
                let old_bytes = previous_state
                    .as_ref()
                    .and_then(|state| state.claim_credit_spent_bytes.get(*claim))
                    .copied()
                    .unwrap_or_default();
                **spent != old
                    || prepared
                        .state()
                        .claim_credit_spent_bytes
                        .get(*claim)
                        .copied()
                        != Some(old_bytes)
            })
            .map(|(claim, _)| claim)
            .collect();
        let completed_claims: std::collections::BTreeSet<_> = completions
            .iter()
            .filter_map(|record| match &record.event {
                super::completion::LiveCompletionEvent::EffectTerminal { claim_id, .. }
                | super::completion::LiveCompletionEvent::CallbackSuspended { claim_id, .. } => {
                    Some(claim_id.to_string())
                }
                _ => None,
            })
            .collect();
        if changed_claims.len() != completed_claims.len()
            || !changed_claims
                .iter()
                .all(|claim| completed_claims.contains(*claim))
        {
            return Err(RuntimeStoreError::WriteFailed(
                "every changed effect settlement requires its exact completion record".into(),
            ));
        }
        let changed_requests: std::collections::BTreeSet<_> = prepared
            .state()
            .request_credit_spent_records
            .iter()
            .filter(|(request, spent)| {
                let old = previous_state
                    .as_ref()
                    .and_then(|state| state.request_credit_spent_records.get(*request))
                    .copied()
                    .unwrap_or_default();
                let old_bytes = previous_state
                    .as_ref()
                    .and_then(|state| state.request_credit_spent_bytes.get(*request))
                    .copied()
                    .unwrap_or_default();
                **spent != old
                    || prepared
                        .state()
                        .request_credit_spent_bytes
                        .get(*request)
                        .copied()
                        != Some(old_bytes)
            })
            .map(|(request, _)| request)
            .collect();
        let completed_requests: std::collections::BTreeSet<_> = completions
            .iter()
            .filter_map(|record| match &record.event {
                super::completion::LiveCompletionEvent::RequestOutcome { request_id, .. } => {
                    Some(request_id.to_string())
                }
                _ => None,
            })
            .collect();
        if changed_requests.len() != completed_requests.len()
            || !changed_requests
                .iter()
                .all(|request| completed_requests.contains(*request))
        {
            return Err(RuntimeStoreError::WriteFailed(
                "every changed request settlement requires its exact outcome record".into(),
            ));
        }
        let next_effect_charge =
            super::authority::store::effect_credits::reserved_completion_charge(prepared.state())?;
        let next_cancellation_charge =
            super::authority::store::cancellation::reserved_charge(prepared.state())?;
        let (expected, mut successor) = match before {
            Some(before) => (Some(before.reference.clone()), before.clone()),
            None => (None, Self::initial_head(session_id)?),
        };
        successor.reference.revision = successor
            .reference
            .revision
            .checked_add(1)
            .ok_or_else(|| RuntimeStoreError::WriteFailed("live revision overflow".into()))?;
        successor.payload.used.encoded_bytes = successor
            .payload
            .used
            .encoded_bytes
            .checked_sub(successor.payload.request_snapshot.len() as u64)
            .and_then(|bytes| bytes.checked_add(snapshot.len() as u64))
            .ok_or_else(|| {
                RuntimeStoreError::WriteFailed("live snapshot charge overflow".into())
            })?;
        successor.payload.request_snapshot = Arc::new(snapshot);
        successor.payload.reserved = successor
            .payload
            .reserved
            .checked_sub(previous_effect_charge)
            .and_then(|remaining| remaining.checked_sub(previous_cancellation_charge))
            .and_then(|remaining| remaining.checked_add(next_effect_charge))
            .and_then(|remaining| remaining.checked_add(next_cancellation_charge))
            .map_err(|error| {
                RuntimeStoreError::WriteFailed(format!(
                    "Live completion reservation delta: {error}"
                ))
            })?;
        let mut encoded = Vec::with_capacity(completions.len());
        let mut records = Vec::with_capacity(completions.len());
        for completion in completions {
            let frame = completion
                .encode()
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            let prior_state = previous_state.as_ref().ok_or_else(|| {
                RuntimeStoreError::WriteFailed("Live completion requires retained authority".into())
            })?;
            match &completion.event {
                super::completion::LiveCompletionEvent::EffectTerminal { .. } => {
                    super::authority::store::effect_credits::validate_completion_delta(
                        prior_state,
                        prepared.state(),
                        &completion,
                        frame.charge(),
                    )?;
                }
                super::completion::LiveCompletionEvent::CallbackSuspended { .. } => {
                    super::authority::store::callback_credits::validate_completion_delta(
                        prior_state,
                        prepared.state(),
                        &completion,
                        frame.charge(),
                    )?;
                }
                super::completion::LiveCompletionEvent::RequestOutcome { .. } => {
                    super::authority::store::request_credits::validate_completion_delta(
                        prior_state,
                        prepared.state(),
                        &completion,
                        frame.charge(),
                    )?;
                }
                _ => {
                    return Err(RuntimeStoreError::WriteFailed(
                        "completion kind has no generated settlement realization".into(),
                    ));
                }
            }
            successor.reference.event_count = successor
                .reference
                .event_count
                .checked_add(1)
                .ok_or_else(|| {
                    RuntimeStoreError::WriteFailed("Live completion sequence overflow".into())
                })?;
            if completion.session_id != *session_id
                || completion.sequence.get() != successor.reference.event_count
            {
                return Err(RuntimeStoreError::WriteFailed(
                    "Live completion is not the next session record".into(),
                ));
            }
            successor.reference.prefix_digest = successor
                .reference
                .prefix_digest
                .appended(completion.sequence, frame.bytes());
            successor.payload.used = successor
                .payload
                .used
                .checked_add(frame.charge())
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            encoded.push(frame.bytes().to_vec());
            records.push(LiveLedgerRecord::Completion(completion));
        }
        let commit = Self {
            purpose: LiveLedgerWritePurpose::ComponentMutation,
            expected,
            expected_actor: None,
            expected_lifecycle: None,
            successor,
            records,
            sources: Vec::new(),
            input_admission: None,
            input_stage: None,
            input_read_fences: Vec::new(),
            quota: crate::live_resources::LIVE_LEDGER_MAX_CHARGE,
        };
        commit.validate(before, &encoded, LiveSourceChargeDelta::default())?;
        Ok(commit)
    }

    pub fn session_id(&self) -> &SessionId {
        &self.successor.reference.session_id
    }
    pub fn expected(&self) -> Option<&LiveHeadReference> {
        self.expected.as_ref()
    }
    pub fn expected_actor(&self) -> Option<&RuntimeSessionAuthority> {
        self.expected_actor.as_ref()
    }
    pub fn expected_lifecycle(&self) -> Option<&MachineLifecycleExpectedVersion> {
        self.expected_lifecycle.as_ref()
    }
    pub fn purpose(&self) -> LiveLedgerWritePurpose {
        self.purpose
    }

    pub(crate) fn is_archive_ingress_fence(&self) -> bool {
        matches!(self.purpose, LiveLedgerWritePurpose::ArchiveIngressFence)
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn with_lifecycle_fence(
        mut self,
        version: MachineLifecycleObservationVersion,
    ) -> Self {
        self.expected_lifecycle = Some(MachineLifecycleExpectedVersion::Version(version));
        self
    }

    #[cfg(all(not(target_arch = "wasm32"), any(feature = "live", test)))]
    pub(in crate::live_ledger) fn with_archive_existence_fence(
        mut self,
        lifecycle: MachineLifecycleExpectedVersion,
        actor: Option<RuntimeSessionAuthority>,
    ) -> Self {
        self.purpose = LiveLedgerWritePurpose::ArchiveIngressFence;
        self.expected_lifecycle = Some(lifecycle);
        self.expected_actor = actor;
        self
    }
    pub fn successor(&self) -> &LiveLedgerStoredHead {
        &self.successor
    }
    pub fn records(&self) -> &[LiveLedgerRecord] {
        &self.records
    }

    pub(crate) fn prefix_witnesses<'a>(
        &'a self,
        encoded: &'a [Vec<u8>],
    ) -> impl Iterator<Item = super::record::LiveEventPrefixWitness> + 'a {
        let prefix = self.expected.as_ref().map_or_else(
            || {
                LiveLedgerPrefixDigest::empty(
                    self.session_id(),
                    self.successor.reference.generation,
                )
            },
            |head| head.prefix_digest,
        );
        self.records
            .iter()
            .zip(encoded)
            .scan(prefix, |prefix, (record, bytes)| {
                *prefix = prefix.appended(record.sequence(), bytes);
                Some(super::record::LiveEventPrefixWitness {
                    commit_revision: self.successor.reference.revision,
                    prefix: *prefix,
                })
            })
    }
    pub(crate) fn sources(&self) -> &[PreparedLiveSourceMutation] {
        &self.sources
    }

    pub fn input_admission(&self) -> Option<&crate::input_state::InputStatePersistenceRecord> {
        self.input_admission.as_ref()
    }

    pub fn input_stage(&self) -> Option<&LiveInputStageMutation> {
        self.input_stage.as_deref()
    }

    pub fn input_read_fences(&self) -> &[LiveInputReadFence] {
        &self.input_read_fences
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn with_execution_fence(
        mut self,
        observation: &crate::store::ExactInputStateObservation,
        lifecycle: MachineLifecycleObservationVersion,
    ) -> Result<Self, RuntimeStoreError> {
        let state = observation.state();
        if state.seed.phase != crate::input_state::InputLifecycleState::Staged
            || state.seed.terminal_outcome.is_some()
            || state.seed.last_run_id.is_none()
            || !matches!(
                state.state.persisted_input,
                Some(crate::input::Input::LiveRequest(_))
            )
        {
            return Err(RuntimeStoreError::WriteFailed(
                "Live execution fence requires a staged Live input".into(),
            ));
        }
        self.input_read_fences = vec![LiveInputReadFence {
            input_id: state.state.input_id.clone(),
            expected_row_digest: observation.exact_row_digest().to_owned(),
            purpose: LiveInputReadFencePurpose::ActiveExecution,
        }];
        self.expected_lifecycle = Some(MachineLifecycleExpectedVersion::Version(lifecycle));
        self.validate_execution_fence()?;
        Ok(self)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn with_completion_fence(
        self,
        observation: &crate::store::ExactInputStateObservation,
    ) -> Result<Self, RuntimeStoreError> {
        self.with_completion_batch_fence(
            std::slice::from_ref(observation),
            &observation.state().state.input_id,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn with_completion_batch_fence(
        mut self,
        observations: &[crate::store::ExactInputStateObservation],
        input_id: &meerkat_core::lifecycle::InputId,
    ) -> Result<Self, RuntimeStoreError> {
        let inputs: Vec<_> = observations.iter().map(|row| row.state().clone()).collect();
        if crate::input_state::input_terminal_completion_outcome(&inputs, input_id)
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
            .is_none()
        {
            return Err(RuntimeStoreError::WriteFailed(
                "Live completion requires the exact finalized ordinary outcome".into(),
            ));
        }
        self.input_read_fences = observations
            .iter()
            .map(|observation| LiveInputReadFence {
                input_id: observation.state().state.input_id.clone(),
                expected_row_digest: observation.exact_row_digest().to_owned(),
                purpose: LiveInputReadFencePurpose::FinalizedCompletion,
            })
            .collect();
        self.validate_execution_fence()?;
        Ok(self)
    }

    fn validate_execution_fence(&self) -> Result<(), RuntimeStoreError> {
        crate::store::validate_input_state_batch_read_ids(
            &self
                .input_read_fences
                .iter()
                .map(|fence| fence.input_id.clone())
                .collect::<Vec<_>>(),
        )?;
        #[cfg(not(target_arch = "wasm32"))]
        for fence in &self.input_read_fences {
            let valid = match fence.purpose {
                LiveInputReadFencePurpose::ActiveExecution => {
                    self.input_read_fences.len() == 1
                        && matches!(
                            self.expected_lifecycle,
                            Some(MachineLifecycleExpectedVersion::Version(_))
                        )
                        && self.input_admission.is_none()
                }
                LiveInputReadFencePurpose::FinalizedCompletion => {
                    self.expected_lifecycle.is_none() && self.input_admission.is_none()
                }
                LiveInputReadFencePurpose::CallbackAdmission => {
                    self.input_read_fences.len() == 1
                        && matches!(
                            self.expected_lifecycle,
                            Some(MachineLifecycleExpectedVersion::Version(_))
                        )
                        && self.expected_actor.is_some()
                        && self.input_admission.as_ref().is_some_and(|input| {
                            input.as_stored().state.input_id != fence.input_id
                                && matches!(&input.as_stored().state.persisted_input,
                                    Some(crate::input::Input::LiveRequest(input))
                                        if input.request.is_callback_continuation())
                        })
                }
            };
            if self.input_stage.is_some() || !valid {
                return Err(RuntimeStoreError::WriteFailed(
                    "Live input observation has a contradictory mutation or lifecycle fence".into(),
                ));
            }
        }
        #[cfg(target_arch = "wasm32")]
        if !self.input_read_fences.is_empty() {
            return Err(RuntimeStoreError::Unsupported(
                "native Live input authority cannot be realized on wasm32".into(),
            ));
        }
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn with_callback_input_admission(
        mut self,
        input: crate::input_state::InputStatePersistenceRecord,
        source: super::source::LiveSourceRow,
        lifecycle: MachineLifecycleObservationVersion,
        origin: &crate::store::ExactInputStateObservation,
        callback: &crate::store::CommittedCallbackResultsObservation,
    ) -> Result<Self, RuntimeStoreError> {
        use crate::input_state::{
            InputTerminalCompletionFinalizationVerdict, InputTerminalCompletionPhase,
            input_terminal_completion_outcome,
        };
        let invalid = || {
            RuntimeStoreError::WriteFailed(
                "callback admission requires exact finalized origin and complete committed results"
                    .into(),
            )
        };
        let stored = origin.state();
        let outcome =
            input_terminal_completion_outcome(std::slice::from_ref(stored), &stored.state.input_id)
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                .ok_or_else(invalid)?;
        let completion = stored
            .state
            .terminal_completion
            .as_ref()
            .ok_or_else(invalid)?;
        let meerkat_core::session::StagedCallbackResultsObservation::Complete(results) =
            callback.results()
        else {
            return Err(invalid());
        };
        let Some(crate::input::Input::LiveRequest(admitted)) =
            &input.as_stored().state.persisted_input
        else {
            return Err(invalid());
        };
        let crate::live_request::LiveExecutionRequestRecord::CallbackContinuation {
            continuation,
            ..
        } = &admitted.request
        else {
            return Err(invalid());
        };
        if !matches!(
            completion.phase,
            InputTerminalCompletionPhase::Finalized {
                finalization: InputTerminalCompletionFinalizationVerdict::Succeeded,
                ..
            }
        ) || outcome.callback_identity() != Some(callback.target())
            || completion.batch_key.run_id() != Some(callback.target().run_id())
            || stored.seed.last_run_id.as_ref() != Some(callback.target().run_id())
            || completion.owner_input_id != stored.state.input_id
            || completion.completion_input_ids.as_deref()
                != Some(std::slice::from_ref(&stored.state.input_id))
            || callback.target().session_id() != self.session_id()
            || callback.authority().session_id() != self.session_id()
            || results.identity() != callback.target()
            || continuation.target != *callback.target()
            || &continuation.results_digest != results.digest()
        {
            return Err(invalid());
        }
        self.expected_actor = Some(callback.authority().clone());
        self.expected_lifecycle = Some(MachineLifecycleExpectedVersion::Version(lifecycle));
        self.input_read_fences = vec![LiveInputReadFence {
            input_id: stored.state.input_id.clone(),
            expected_row_digest: origin.exact_row_digest().to_owned(),
            purpose: LiveInputReadFencePurpose::CallbackAdmission,
        }];
        self.sources.push(PreparedLiveSourceMutation {
            expected: Some(source.digest()),
            replacement: source,
        });
        self.input_admission = Some(input);
        self.validate_input_admission()?;
        self.validate_execution_fence()?;
        Ok(self)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn with_input_stage(
        mut self,
        input: crate::input_state::InputStatePersistenceRecord,
        source: super::source::LiveSourceRow,
        lifecycle: crate::store::MachineLifecycleCommit,
        expected_lifecycle: MachineLifecycleObservationVersion,
    ) -> Result<Self, RuntimeStoreError> {
        self.sources.push(PreparedLiveSourceMutation {
            expected: Some(source.digest()),
            replacement: source,
        });
        self.expected_lifecycle =
            Some(MachineLifecycleExpectedVersion::Version(expected_lifecycle));
        self.input_stage = Some(Box::new(LiveInputStageMutation { input, lifecycle }));
        self.validate_input_stage()?;
        Ok(self)
    }

    fn validate_input_stage(&self) -> Result<(), RuntimeStoreError> {
        let Some(stage) = self.input_stage() else {
            return Ok(());
        };
        let invalid = || RuntimeStoreError::WriteFailed("invalid joint Live input stage".into());
        let bundle = stage.input.as_stored();
        let Some(crate::input::Input::LiveRequest(input)) = &bundle.state.persisted_input else {
            return Err(invalid());
        };
        let run_id = bundle.seed.last_run_id.as_ref().ok_or_else(invalid)?;
        if self.input_admission.is_some()
            || !matches!(
                self.expected_lifecycle,
                Some(MachineLifecycleExpectedVersion::Version(_))
            )
            || stage.input.expected_row_digest().is_none()
            || bundle.seed.phase != crate::input_state::InputLifecycleState::Staged
            || bundle.seed.terminal_outcome.is_some()
            || input.header.id != bundle.state.input_id
            || input.header.source != crate::input::InputOrigin::LiveRequest
            || input.header.durability != crate::input::InputDurability::Durable
            || bundle.state.durability != Some(crate::input::InputDurability::Durable)
            || bundle.state.idempotency_key != input.header.idempotency_key
            || stage.lifecycle.snapshot().run().current_run_id() != Some(run_id)
        {
            return Err(invalid());
        }
        let (provenance, source_row) = input.request.source_reference();
        let [source] = self.sources.as_slice() else {
            return Err(invalid());
        };
        let crate::live_source::LiveSourceEntryRecord::Reservation { record } =
            source.replacement.record()?
        else {
            return Err(invalid());
        };
        let crate::live_source::LiveSourceDisposition::Admitted { receipt } = record.disposition()
        else {
            return Err(invalid());
        };
        if source.expected != Some(source.replacement.digest())
            || provenance.source().session_id() != self.session_id()
            || provenance.source() != record.source()
            || provenance.request_id() != record.request_id()
            || record.frozen_digest().map_err(|_| invalid())? != *source_row
        {
            return Err(invalid());
        }
        match &input.request {
            crate::live_request::LiveExecutionRequestRecord::LiveRequest { .. } => {
                if receipt.input_id() != &bundle.state.input_id {
                    return Err(invalid());
                }
            }
            crate::live_request::LiveExecutionRequestRecord::CallbackContinuation { .. } => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let state = crate::generated::live_request_state::decode(
                        &self.successor.payload.request_snapshot,
                    )
                    .map_err(|_| invalid())?;
                    input
                        .request
                        .validate_continuation_binding(&state, &bundle.state.input_id)
                        .map_err(|_| invalid())?;
                    let run = run_id.to_string();
                    if state.run_inputs.get(&run) != Some(&bundle.state.input_id.to_string())
                        || state.request_runs.get(&provenance.request_id().to_string())
                            != Some(&run)
                    {
                        return Err(invalid());
                    }
                }
                #[cfg(target_arch = "wasm32")]
                return Err(invalid());
            }
        }
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn with_source_mutation(
        mut self,
        before: Option<&super::source::LiveSourceRow>,
        replacement: super::source::LiveSourceRow,
    ) -> Result<Self, RuntimeStoreError> {
        let replacement_charge = replacement.charge()?;
        self.successor.payload.used = self
            .successor
            .payload
            .used
            .checked_sub(before.map_or(Ok(LiveResourceCharge::default()), |row| row.charge())?)
            .and_then(|charge| charge.checked_add(replacement_charge))
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        self.sources.push(PreparedLiveSourceMutation {
            expected: before.map(super::source::LiveSourceRow::digest),
            replacement,
        });
        Ok(self)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::live_ledger) fn with_source_selection(
        mut self,
        read: &crate::store::live_read::LiveCompositeRead,
        transcript: &super::transcript_authority::dsl::LiveTranscriptMachinePreparedAuthority,
        row: super::source::LiveSourceRow,
    ) -> Result<Self, RuntimeStoreError> {
        if self.expected.as_ref() != read.authority().live_head()
            || self.session_id() != read.authority().actor().session_id()
        {
            return Err(RuntimeStoreError::WriteFailed(
                "source selection lost its composite predecessor".into(),
            ));
        }
        self = self.with_transcript_transition(transcript)?;
        self.expected_actor = Some(read.authority().actor().clone());
        self.with_source_mutation(None, row)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::live_ledger) fn with_transcript_transition(
        mut self,
        transcript: &super::transcript_authority::dsl::LiveTranscriptMachinePreparedAuthority,
    ) -> Result<Self, RuntimeStoreError> {
        let predecessor = crate::generated::live_transcript_state::decode(
            &self.successor.payload.transcript_snapshot,
        )
        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let next_reserved = transcript_commit::reserved_charge(transcript.state())?;
        self.successor.payload.reserved = self
            .successor
            .payload
            .reserved
            .checked_sub(transcript_commit::reserved_charge(&predecessor)?)
            .and_then(|charge| charge.checked_add(next_reserved))
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        let snapshot = crate::generated::live_transcript_state::encode(transcript.state())
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        self.successor.payload.used.encoded_bytes = self
            .successor
            .payload
            .used
            .encoded_bytes
            .checked_sub(self.successor.payload.transcript_snapshot.len() as u64)
            .and_then(|bytes| bytes.checked_add(snapshot.len() as u64))
            .ok_or_else(|| {
                RuntimeStoreError::WriteFailed("source selection snapshot charge overflow".into())
            })?;
        self.successor.payload.transcript_snapshot = Arc::new(snapshot);
        self.successor.payload.ingress_generation = transcript.state().ingress_generation;
        Ok(self)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn with_input_admission(
        mut self,
        input: crate::input_state::InputStatePersistenceRecord,
        before_source: &super::source::LiveSourceRow,
        replacement: super::source::LiveSourceRow,
        lifecycle: MachineLifecycleObservationVersion,
    ) -> Result<Self, RuntimeStoreError> {
        self = self.with_source_mutation(Some(before_source), replacement)?;
        self.expected_lifecycle = Some(MachineLifecycleExpectedVersion::Version(lifecycle));
        self.input_admission = Some(input);
        self.validate_input_admission()?;
        Ok(self)
    }

    fn validate_input_admission(&self) -> Result<(), RuntimeStoreError> {
        let Some(record) = &self.input_admission else {
            return Ok(());
        };
        let invalid =
            || RuntimeStoreError::WriteFailed("invalid joint Live input admission".into());
        let bundle = record.as_stored();
        let Some(crate::input::Input::LiveRequest(input)) = &bundle.state.persisted_input else {
            return Err(invalid());
        };
        if !matches!(
            self.expected_lifecycle,
            Some(MachineLifecycleExpectedVersion::Version(_))
        ) || record.expected_row_digest().is_some()
            || input.header.id != bundle.state.input_id
            || input.header.source != crate::input::InputOrigin::LiveRequest
            || input.header.durability != crate::input::InputDurability::Durable
            || input.header.supersession_key.is_some()
            || bundle.state.durability != Some(crate::input::InputDurability::Durable)
            || bundle.state.idempotency_key != input.header.idempotency_key
            || bundle.state.runtime_semantics.is_none_or(|semantics| {
                semantics.boundary()
                    != meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart
                    || semantics.execution_kind()
                        != if input.request.is_callback_continuation() {
                            meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending
                        } else {
                            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn
                        }
                    || semantics.live_interrupt_required
            })
            || bundle.seed.phase != crate::input_state::InputLifecycleState::Queued
            || bundle.seed.last_run_id.is_some()
            || bundle.seed.last_boundary_sequence.is_some()
            || bundle.seed.terminal_outcome.is_some()
            || bundle.seed.admission_sequence.is_none()
            || bundle.seed.attempt_count != 0
            || bundle.seed.recovery_lane != Some(meerkat_core::types::HandlingMode::Queue)
        {
            return Err(invalid());
        }
        let (provenance, source_row) = input.request.source_reference();
        let source = self
            .sources
            .iter()
            .find(|source| source.replacement.source() == provenance.source())
            .ok_or_else(invalid)?;
        let crate::live_source::LiveSourceEntryRecord::Reservation {
            record: source_record,
        } = source.replacement.record()?
        else {
            return Err(invalid());
        };
        let crate::live_source::LiveSourceDisposition::Admitted { receipt } =
            source_record.disposition()
        else {
            return Err(invalid());
        };
        if source.expected.is_none()
            || provenance.source().session_id() != self.session_id()
            || source_record.request_id() != provenance.request_id()
            || source_record.frozen_digest().map_err(|_| invalid())? != *source_row
        {
            return Err(invalid());
        }
        match &input.request {
            crate::live_request::LiveExecutionRequestRecord::LiveRequest { .. } => {
                if receipt.input_id() != &bundle.state.input_id
                    || !self.input_read_fences.is_empty()
                {
                    return Err(invalid());
                }
            }
            crate::live_request::LiveExecutionRequestRecord::CallbackContinuation {
                continuation,
                ..
            } => {
                if continuation.target.session_id() != self.session_id() {
                    return Err(invalid());
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let [fence] = self.input_read_fences.as_slice() else {
                        return Err(invalid());
                    };
                    if !matches!(fence.purpose, LiveInputReadFencePurpose::CallbackAdmission)
                        || source.expected != Some(source.replacement.digest())
                        || self.sources.len() != 1
                        || self.expected_actor.is_none()
                    {
                        return Err(invalid());
                    }
                    let state = crate::generated::live_request_state::decode(
                        &self.successor.payload.request_snapshot,
                    )
                    .map_err(|_| invalid())?;
                    input
                        .request
                        .validate_continuation_binding(&state, &bundle.state.input_id)
                        .map_err(|_| invalid())?;
                    let run = continuation.target.run_id().to_string();
                    if state.run_inputs.get(&run) != Some(&fence.input_id.to_string())
                        || state
                            .run_successors
                            .get(&run)
                            .is_none_or(|next| !next.is_empty())
                    {
                        return Err(invalid());
                    }
                }
                #[cfg(target_arch = "wasm32")]
                return Err(invalid());
            }
        }
        Ok(())
    }

    pub(crate) fn validate_new_source_context(
        &self,
        source: &PreparedLiveSourceMutation,
        current: Option<&super::source::LiveSourceRow>,
    ) -> Result<(), RuntimeStoreError> {
        if current.is_none()
            && let crate::live_source::LiveSourceEntryRecord::Reservation { record } =
                source.replacement.record()?
            && (Some(&record.context().live_head) != self.expected.as_ref()
                || self
                    .expected_actor
                    .as_ref()
                    .is_none_or(|actor| !record.context().actor.matches_authority(actor)))
        {
            return Err(RuntimeStoreError::WriteFailed(
                "new live source requires its exact composite actor/head fence".into(),
            ));
        }
        Ok(())
    }

    /// Bind the whole operation, not just the resulting prefix. Length framing
    /// is versioned and snapshots/record bytes feed the hash without copying.
    pub(crate) fn operation_digest(
        &self,
        encoded: &[Vec<u8>],
    ) -> Result<LiveLedgerCommitDigest, RuntimeStoreError> {
        let Self {
            purpose,
            expected,
            expected_actor,
            expected_lifecycle,
            successor,
            records,
            sources,
            input_admission,
            input_stage,
            input_read_fences,
            quota,
        } = self;
        let LiveLedgerStoredHead { reference, payload } = successor;
        let LiveLedgerPayloadState {
            used,
            reserved,
            ingress_generation,
            transcript_snapshot,
            request_snapshot,
        } = payload;
        if encoded.len() != records.len() {
            return Err(RuntimeStoreError::WriteFailed(
                "live operation record count differs".into(),
            ));
        }
        let mut hash = Sha256::new();
        hash.update(b"meerkat.live-ledger-commit.v6\0");
        hash_serialized_part(&mut hash, purpose)?;
        hash_serialized_part(&mut hash, expected)?;
        hash_serialized_part(
            &mut hash,
            &expected_lifecycle.as_ref().map(|expected| match expected {
                MachineLifecycleExpectedVersion::Missing => (true, None),
                MachineLifecycleExpectedVersion::Version(version) => {
                    (false, Some(version.as_str()))
                }
            }),
        )?;
        match expected_actor {
            None => hash.update([0]),
            Some(RuntimeSessionAuthority::WholeBlob(actor)) => {
                hash.update([1]);
                hash_serialized_part(
                    &mut hash,
                    &(
                        actor.authority_version(),
                        actor.session_id(),
                        actor.store_revision(),
                        actor.blob_sha256(),
                    ),
                )?;
            }
            Some(RuntimeSessionAuthority::HeadCanonical(actor)) => {
                hash.update([2]);
                hash_serialized_part(
                    &mut hash,
                    &(
                        actor.authority_version(),
                        actor.session_id(),
                        actor.store_revision(),
                        actor.boundary_head(),
                        actor.committed_head_token(),
                    ),
                )?;
            }
        }
        hash_serialized_part(
            &mut hash,
            &(reference, used, reserved, ingress_generation, quota),
        )?;
        hash_part(&mut hash, transcript_snapshot);
        hash_part(&mut hash, request_snapshot);
        hash.update((encoded.len() as u64).to_be_bytes());
        for record in encoded {
            hash_part(&mut hash, record);
        }
        hash.update((sources.len() as u64).to_be_bytes());
        for source in sources {
            hash_serialized_part(&mut hash, &(source.expected, source.replacement.source()))?;
            hash_part(&mut hash, source.replacement.bytes());
        }
        hash_serialized_part(
            &mut hash,
            &input_admission
                .as_ref()
                .map(|record| (record.as_stored(), record.expected_row_digest())),
        )?;
        hash_serialized_part(
            &mut hash,
            &input_stage
                .as_ref()
                .map(|stage| (stage.input.as_stored(), stage.input.expected_row_digest())),
        )?;
        if let Some(stage) = input_stage {
            hash_part(&mut hash, &stage.lifecycle.store_record().encode()?);
        }
        hash_serialized_part(&mut hash, &input_read_fences)?;
        Ok(LiveLedgerCommitDigest(hash.finalize().into()))
    }

    pub(crate) fn encoded_records(&self) -> Result<Vec<Vec<u8>>, RuntimeStoreError> {
        self.validate_execution_fence()?;
        self.validate_input_admission()?;
        self.validate_input_stage()?;
        let encoded: Vec<Vec<u8>> = self.records.iter().map(|record| {
            let bytes = record.encode().map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            if bytes.len() > crate::store::live_read::LIVE_COMPOSITE_MAX_RECORD_BYTES
                || record.channel_id().as_str().is_empty()
                || record.channel_id().as_str().len() > 128
                || matches!(record, LiveLedgerRecord::Completion(record) if &record.session_id != self.session_id())
            {
                return Err(RuntimeStoreError::WriteFailed("invalid live record owner or bound".into()));
            }
            Ok(bytes)
        })        .collect::<Result<_, _>>()?;
        // Validate even on replay: a subset of the originally committed batch
        // must not count as the same operation merely because its rows match.
        let invalid =
            || RuntimeStoreError::WriteFailed("invalid prepared live record prefix".into());
        let after = &self.successor.reference;
        let (revision, mut sequence, mut prefix) = match &self.expected {
            Some(before) => {
                if before.session_id != after.session_id || before.generation != after.generation {
                    return Err(invalid());
                }
                (before.revision, before.event_count, before.prefix_digest)
            }
            None => (
                0,
                0,
                LiveLedgerPrefixDigest::empty(self.session_id(), after.generation),
            ),
        };
        if revision.checked_add(1) != Some(after.revision) {
            return Err(invalid());
        }
        for (record, bytes) in self.records.iter().zip(&encoded) {
            sequence = sequence.checked_add(1).ok_or_else(invalid)?;
            if record.sequence().get() != sequence {
                return Err(invalid());
            }
            prefix = prefix.appended(record.sequence(), bytes);
        }
        if sequence != after.event_count || prefix != after.prefix_digest {
            return Err(invalid());
        }
        self.successor.validate_payload()?;
        let mut keys = std::collections::HashSet::new();
        for source in &self.sources {
            super::source::validate_source_storage_key(source.replacement.source())
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            if source.replacement.source().session_id() != self.session_id()
                || !keys.insert(source.replacement.source())
            {
                return Err(RuntimeStoreError::WriteFailed(
                    "duplicate or foreign source mutation".into(),
                ));
            }
        }
        Ok(encoded)
    }

    /// Mechanical validation against the state observed by this transaction.
    /// Counter, prefix, byte and revision checks never select a lifecycle step.
    pub(crate) fn validate(
        &self,
        before: Option<&LiveLedgerStoredHead>,
        encoded: &[Vec<u8>],
        sources: LiveSourceChargeDelta,
    ) -> Result<(), RuntimeStoreError> {
        let invalid =
            || RuntimeStoreError::WriteFailed("invalid prepared live head/event transition".into());
        #[cfg(not(target_arch = "wasm32"))]
        if self.is_archive_ingress_fence() {
            if self.expected_lifecycle.is_none()
                || !self.records.is_empty()
                || !self.sources.is_empty()
                || self.input_admission.is_some()
                || self.input_stage.is_some()
                || !self.input_read_fences.is_empty()
            {
                return Err(invalid());
            }
            let request = super::authority::dsl::LiveRequestMachineAuthority::recover_from_state(
                crate::generated::live_request_state::decode(
                    &self.successor.payload.request_snapshot,
                )
                .map_err(|_| invalid())?,
            )
            .map_err(|_| invalid())?;
            let transcript = super::transcript_authority::dsl::LiveTranscriptMachineAuthority::recover_from_state(
                crate::generated::live_transcript_state::decode(&self.successor.payload.transcript_snapshot)
                    .map_err(|_| invalid())?,
            ).map_err(|_| invalid())?;
            if request.state().ingress_open || transcript.state().ingress_open {
                return Err(invalid());
            }
        }
        let after = &self.successor;
        if self.expected.as_ref() != before.map(|head| &head.reference)
            || after.reference.format != LiveLedgerFormatV1::V1
            || after.reference.generation == 0
            || after.payload.ingress_generation == 0
            || self.records.len() != encoded.len()
            || self
                .expected_actor
                .as_ref()
                .is_some_and(|actor| actor.session_id() != self.session_id())
            || self.quota.records > crate::live_resources::LIVE_LEDGER_MAX_CHARGE.records
            || self.quota.encoded_bytes
                > crate::live_resources::LIVE_LEDGER_MAX_CHARGE.encoded_bytes
            || [
                after.reference.generation,
                after.reference.revision,
                after.reference.event_count,
                after.payload.ingress_generation,
            ]
            .into_iter()
            .any(|value| i64::try_from(value).is_err())
            || [after.reference.revision, after.reference.event_count]
                .into_iter()
                .any(|value| {
                    value
                        .checked_add(after.payload.reserved.records)
                        .is_none_or(|reserved_end| i64::try_from(reserved_end).is_err())
                })
        {
            return Err(invalid());
        }
        let (revision, count, mut prefix, used, old_snapshot_bytes) = match before {
            None => (
                0,
                0,
                LiveLedgerPrefixDigest::empty(self.session_id(), after.reference.generation),
                LiveResourceCharge {
                    records: 0,
                    encoded_bytes: LIVE_HEAD_STORAGE_ALLOWANCE_BYTES,
                },
                0,
            ),
            Some(before) => {
                if before.reference.session_id != *self.session_id()
                    || before.reference.generation != after.reference.generation
                    || before.payload.ingress_generation > after.payload.ingress_generation
                {
                    return Err(invalid());
                }
                (
                    before.reference.revision,
                    before.reference.event_count,
                    before.reference.prefix_digest,
                    before.payload.used,
                    before.payload.snapshot_bytes()?,
                )
            }
        };
        if revision.checked_add(1) != Some(after.reference.revision) {
            return Err(invalid());
        }
        let mut sequence = count;
        let mut appended = LiveResourceCharge::default();
        for (record, bytes) in self.records.iter().zip(encoded) {
            sequence = sequence.checked_add(1).ok_or_else(invalid)?;
            if record.sequence().get() != sequence
                || matches!(record, LiveLedgerRecord::Completion(record) if &record.session_id != self.session_id())
            {
                return Err(invalid());
            }
            prefix = prefix.appended(record.sequence(), bytes);
            appended = appended
                .checked_add(LiveResourceCharge::for_event_record(bytes).map_err(|_| invalid())?)
                .map_err(|_| invalid())?;
        }
        let new_snapshot_bytes = after.payload.snapshot_bytes()?;
        let expected_bytes = used
            .encoded_bytes
            .checked_sub(old_snapshot_bytes)
            .and_then(|bytes| bytes.checked_sub(sources.previous.encoded_bytes))
            .and_then(|bytes| bytes.checked_add(new_snapshot_bytes))
            .and_then(|bytes| bytes.checked_add(appended.encoded_bytes))
            .and_then(|bytes| bytes.checked_add(sources.replacement.encoded_bytes))
            .ok_or_else(invalid)?;
        if after.reference.event_count != sequence
            || after.reference.prefix_digest != prefix
            || used
                .records
                .checked_sub(sources.previous.records)
                .and_then(|records| records.checked_add(sources.replacement.records))
                .and_then(|records| records.checked_add(appended.records))
                != Some(after.payload.used.records)
            || after.payload.used.encoded_bytes != expected_bytes
        {
            return Err(invalid());
        }
        let total = after
            .payload
            .used
            .checked_add(after.payload.reserved)
            .map_err(|_| invalid())?;
        if total.records > self.quota.records || total.encoded_bytes > self.quota.encoded_bytes {
            return Err(RuntimeStoreError::LiveLedgerCapacityExceeded);
        }
        Ok(())
    }
}

fn hash_part(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

fn hash_serialized_part(
    hash: &mut Sha256,
    value: &impl serde::Serialize,
) -> Result<(), RuntimeStoreError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    hash_part(hash, &bytes);
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiveLedgerCommitOutcome {
    Committed {
        head: LiveHeadReference,
    },
    AlreadyCommitted {
        head: LiveHeadReference,
    },
    Conflict {
        current: Option<LiveHeadReference>,
    },
    ActorConflict {
        current: Option<RuntimeSessionAuthority>,
    },
    SourceConflict {
        source: meerkat_core::live_execution::request::LiveSourceKey,
        current: Option<super::source::LiveSourceRowDigest>,
    },
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) use tests::append_observation_fixture;
