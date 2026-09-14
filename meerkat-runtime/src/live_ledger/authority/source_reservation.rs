use super::*;
use crate::live_grant::LiveExecutionGrant;
use crate::live_ledger::record::LiveLedgerRecord;
use crate::live_ledger::source::LiveSourceRow;
use crate::live_ledger::transcript_authority::dsl as transcript;
use crate::live_source::{
    LiveSourceContextReference, LiveSourceDisposition, LiveSourceEntryRecord,
    LiveSourceFingerprint, LiveSourceReadCoverage, LiveSourceRecordError,
    LiveSourceReservationParts, LiveSourceReservationRecord,
};
use crate::store::live_history::{LiveHistoryReadError, LiveHistoryReadRequest};
use crate::store::live_read::{
    LIVE_COMPOSITE_MAX_RECORDS, LiveCompositeReadRequest, read_live_composite,
};
use meerkat_core::live_execution::evidence::{
    LIVE_REQUEST_TEXT_MAX_BYTES, LiveObservationInterval, LiveRequestEvidence, LiveRequestText,
};
use meerkat_core::live_execution::request::{LiveRequestEvidenceKind, LiveSourceKey};
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub enum LiveSourceReservationOutcome {
    Retained(Box<LiveSourceEntryRecord>),
    AwaitingObservationDurability,
    UnacceptedAtCapacity,
}

#[derive(Debug, thiserror::Error)]
pub enum LiveSourceReservationError {
    #[error("source selection does not belong to the current channel or composite head")]
    SelectionChanged,
    #[error("a previous source publication must be reconciled before selecting another source")]
    PriorSourceUnconfirmed,
    #[error("source publication was not newly committed: {0:?}")]
    NotCommitted(Box<LiveLedgerCommitOutcome>),
    #[error(transparent)]
    Store(#[from] RuntimeStoreError),
    #[error(transparent)]
    Record(#[from] LiveSourceRecordError),
    #[error(transparent)]
    History(#[from] LiveHistoryReadError),
    #[error(transparent)]
    Codec(#[from] serde_json::Error),
    #[error(transparent)]
    Request(#[from] dsl::LiveRequestMachineTransitionError),
    #[error(transparent)]
    Transcript(#[from] transcript::LiveTranscriptMachineTransitionError),
    #[error(transparent)]
    Evidence(#[from] meerkat_core::live_execution::evidence::LiveEvidenceError),
}

pub(in crate::live_ledger) struct SourceSelection {
    pub source: LiveSourceKey,
    pub fingerprint: LiveSourceFingerprint,
    pub received: u64,
    pub ingress_generation: u64,
    pub explicit: Option<LiveObservationInterval>,
}

impl LiveRequestStoreOwner {
    pub(in crate::live_ledger) async fn reserve_selected_source(
        &self,
        selection: SourceSelection,
        grant: Option<&LiveExecutionGrant<()>>,
        registration: Arc<dyn RuntimeStoreWriteFence>,
        unconfirmed: &mut Option<LiveSourceKey>,
    ) -> Result<LiveSourceReservationOutcome, LiveSourceReservationError> {
        let source = &selection.source;
        if source.session_id() != &self.session_id {
            return Err(LiveSourceReservationError::SelectionChanged);
        }
        let ops = self
            .store
            .live_ledger_ops()
            .ok_or_else(|| RuntimeStoreError::Unsupported("Live source reservation".into()))?;
        if let Some(prior) = unconfirmed.as_ref() {
            if ops.lookup_live_source(prior).await?.is_none() {
                return Err(LiveSourceReservationError::PriorSourceUnconfirmed);
            }
            *unconfirmed = None;
        }
        // Replay precedes even the head read. It never depends on later text,
        // current permission, or whether the old channel is still accepting.
        if let Some(row) = ops.lookup_live_source(source).await? {
            let retained = row.record()?;
            if let LiveSourceEntryRecord::Reservation { record } = &retained {
                record.replay(source, selection.fingerprint)?;
            }
            return Ok(LiveSourceReservationOutcome::Retained(Box::new(retained)));
        }
        let before = ops
            .load_live_head(&self.session_id)
            .await?
            .ok_or(LiveSourceReservationError::SelectionChanged)?;
        if !ops.ledger_write_profile().supports_lifecycle_fence() {
            return Err(
                RuntimeStoreError::Unsupported("Live source lifecycle fence".into()).into(),
            );
        }
        let lifecycle = self
            .store
            .observe_machine_lifecycle(&crate::LogicalRuntimeId::for_session(&self.session_id))
            .await?
            .version()
            .ok_or(LiveSourceReservationError::SelectionChanged)?
            .clone();
        before.validate_payload()?;
        let transcript_owner = transcript::LiveTranscriptMachineAuthority::recover_from_state(
            crate::generated::live_transcript_state::decode(&before.payload.transcript_snapshot)?,
        )?;
        let state = transcript_owner.state();
        let channel = source.channel_id().as_str();
        let interval = match selection.explicit {
            Some(interval) => interval,
            None => LiveObservationInterval::new(
                *state
                    .reservation_frontiers
                    .get(channel)
                    .ok_or(LiveSourceReservationError::SelectionChanged)?,
                *state
                    .durable_watermarks
                    .get(channel)
                    .ok_or(LiveSourceReservationError::SelectionChanged)?,
            )?,
        };
        let input = match selection.explicit {
            Some(_) => transcript::LiveTranscriptInput::SelectExplicitRange {
                channel: channel.into(),
                after: interval.after(),
                through: interval.through(),
                ingress_generation: selection.ingress_generation,
            },
            None => transcript::LiveTranscriptInput::ReservePrefix {
                channel: channel.into(),
                after: interval.after(),
                through: interval.through(),
                received_through: selection.received,
                ingress_generation: selection.ingress_generation,
            },
        };
        let mut transcript_candidate = transcript_owner.prepare_authority();
        let selected =
            transcript::LiveTranscriptMachineMutator::apply(&mut transcript_candidate, input)?;
        let discontinuous = match selected.effects() {
            [transcript::LiveTranscriptEffect::AwaitingObservationDurability { .. }] => {
                return Ok(LiveSourceReservationOutcome::AwaitingObservationDurability);
            }
            [
                transcript::LiveTranscriptEffect::RangeSelected {
                    channel: selected_channel,
                    after,
                    through,
                    discontinuous,
                },
            ] if selected_channel == channel
                && *after == interval.after()
                && *through == interval.through() =>
            {
                *discontinuous
            }
            _ => return Err(LiveSourceReservationError::SelectionChanged),
        };
        let read = read_live_composite(
            ops,
            LiveCompositeReadRequest::new(
                self.session_id.clone(),
                Some(source.channel_id().clone()),
                interval.after(),
                LIVE_COMPOSITE_MAX_RECORDS,
            )?,
        )
        .await?
        .ok_or(LiveSourceReservationError::SelectionChanged)?;
        if read.authority().live_head() != Some(&before.reference) {
            return Err(LiveSourceReservationError::SelectionChanged);
        }
        let mut context = LiveSourceContextReference::from_composite(&read, source, interval)?;
        let mut content = SelectedContent::new(interval);
        content.observe(read.records())?;
        let mut has_more = read.has_more();
        while has_more && content.after < interval.through() {
            let page = ops
                .read_live_history(&LiveHistoryReadRequest::new(
                    before.reference.clone(),
                    Some(source.channel_id().clone()),
                    content.after,
                    LIVE_COMPOSITE_MAX_RECORDS,
                )?)
                .await?;
            content.observe(page.records())?;
            has_more = page.has_more();
        }
        if selection.explicit.is_none() && !interval.is_empty() && !content.through_seen {
            return Err(RuntimeStoreError::ReadFailed(
                "selected channel watermark is missing from its accepted record range".into(),
            )
            .into());
        }
        context.read_coverage = LiveSourceReadCoverage::CompleteSelectedInterval;
        context.record_window_digest = content.digest.finalize().into();
        let request = if content.fits && !content.empty {
            Some(LiveRequestEvidence::ApplicationSnapshot {
                observations: interval,
                request: LiveRequestText::new(content.text)?,
            })
        } else {
            None
        };
        let owner = dsl::LiveRequestMachineAuthority::recover_from_state(
            crate::generated::live_request_state::decode(&before.payload.request_snapshot)?,
        )?;
        let grant_record = grant.map(LiveExecutionGrant::record);
        let grant_ref = grant_record.map(|record| record.grant_ref());
        let mut parts = LiveSourceReservationParts {
            source: source.clone(),
            request_id: meerkat_core::ops::OperationId(uuid::Uuid::new_v4()),
            fingerprint: selection.fingerprint,
            context,
            frozen_request: request,
            grant: grant_ref.clone(),
            cancellation: None,
            disposition: LiveSourceDisposition::Reserved {},
        };
        let source_key = serde_json::to_string(source)?;
        let payload = serde_json::to_string(&parts.frozen_digest()?)?;
        let budget = request_credits::RequestCompletionBudget::measured()?;
        let input = dsl::LiveRequestInput::Reserve {
            request_id: parts.request_id.to_string(),
            source: source_key.clone(),
            payload: payload.clone(),
            evidence: LiveRequestEvidenceKind::ApplicationSnapshot,
            profile_revision: grant_record
                .map(|record| serde_json::to_string(&record.declaration().profile_revision))
                .transpose()?
                .unwrap_or_default(),
            parent_scope: String::new(),
            grant_id: grant_ref
                .as_ref()
                .map(|reference| reference.id.as_uuid().to_string())
                .unwrap_or_default(),
            generation: grant_ref
                .as_ref()
                .map_or(0, |reference| reference.generation.get()),
            executor: grant_record
                .map(|record| serde_json::to_string(&record.executor().binding))
                .transpose()?
                .unwrap_or_default(),
            now: (self.clock)()?,
            credit_records: budget.envelope.total().records,
            credit_bytes: budget.envelope.total().encoded_bytes,
            snapshot_ceiling: budget.snapshot_ceiling,
            content_complete: true,
            content_discontinuous: discontinuous,
            content_empty: content.empty,
            content_fits: content.fits,
        };
        let mut candidate = owner.prepare_authority();
        let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, input.clone())?;
        parts.disposition = match transition.effects() {
            [
                dsl::LiveRequestEffect::SourceReserved {
                    source: echoed,
                    payload: digest,
                    request_id,
                },
            ] if echoed == &source_key
                && digest == &payload
                && request_id == &parts.request_id.to_string() =>
            {
                LiveSourceDisposition::Reserved {}
            }
            [
                dsl::LiveRequestEffect::SourceRefused {
                    source: echoed,
                    payload: digest,
                    reason,
                },
            ] if echoed == &source_key && digest == &payload => {
                LiveSourceDisposition::Refused { reason: *reason }
            }
            _ => return Err(LiveSourceReservationError::SelectionChanged),
        };
        let record = LiveSourceReservationRecord::try_from(parts)?;
        let retained = LiveSourceEntryRecord::Reservation {
            record: Box::new(record),
        };
        let commit = match PreparedLiveLedgerCommit::from_request_transition(
            &self.session_id,
            Some(&before),
            &candidate,
        ) {
            Ok(commit) => commit,
            Err(RuntimeStoreError::LiveLedgerCapacityExceeded) => {
                return Ok(LiveSourceReservationOutcome::UnacceptedAtCapacity);
            }
            Err(error) => return Err(error.into()),
        }
        .with_source_selection(
            &read,
            &transcript_candidate,
            LiveSourceRow::encode(&retained)?,
        )?
        .with_lifecycle_fence(lifecycle);
        let total = commit
            .successor()
            .payload
            .used
            .checked_add(commit.successor().payload.reserved)
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        let maximum = crate::live_resources::LIVE_LEDGER_MAX_CHARGE;
        if total.records > maximum.records || total.encoded_bytes > maximum.encoded_bytes {
            return Ok(LiveSourceReservationOutcome::UnacceptedAtCapacity);
        }
        let fence = Arc::new(CurrentLiveRequestFence {
            time: LiveRequestTimeFence {
                predecessor: owner,
                input,
                expected_snapshot: Arc::clone(&commit.successor().payload.request_snapshot),
                clock: Arc::clone(&self.clock),
            },
            registration,
        });
        *unconfirmed = Some(source.clone());
        let outcome = match ops.commit_live_ledger(commit, fence).await {
            Ok(outcome) => outcome,
            Err(
                error @ (RuntimeStoreError::WriteFenceConflict { .. }
                | RuntimeStoreError::WriteFenceBackoff { .. }
                | RuntimeStoreError::MachineLifecycleVersionConflict { .. }
                | RuntimeStoreError::LiveRequestPublicationRejected { .. }
                | RuntimeStoreError::LiveLedgerCapacityExceeded),
            ) => {
                *unconfirmed = None;
                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
        };
        // Typed outcomes establish a completed transaction. An error or
        // cancelled await leaves the source held for exact lookup.
        *unconfirmed = None;
        match outcome {
            LiveLedgerCommitOutcome::Committed { .. } => {
                Ok(LiveSourceReservationOutcome::Retained(Box::new(retained)))
            }
            outcome => Err(LiveSourceReservationError::NotCommitted(Box::new(outcome))),
        }
    }
}

struct SelectedContent {
    interval: LiveObservationInterval,
    after: u64,
    text: String,
    empty: bool,
    fits: bool,
    through_seen: bool,
    digest: Sha256,
}

impl SelectedContent {
    fn new(interval: LiveObservationInterval) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"meerkat.live-source-selected-records.v1\0");
        Self {
            interval,
            after: interval.after(),
            text: String::new(),
            empty: true,
            fits: true,
            through_seen: false,
            digest,
        }
    }

    fn observe(&mut self, records: &[LiveLedgerRecord]) -> Result<(), LiveSourceReservationError> {
        for record in records {
            self.after = record.sequence().get();
            if self.after > self.interval.through() {
                break;
            }
            self.through_seen |= self.after == self.interval.through();
            let encoded = record
                .encode()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            self.digest.update((encoded.len() as u64).to_be_bytes());
            self.digest.update(encoded);
            if let LiveLedgerRecord::Observation(record) = record {
                let text = record.record().observation.text();
                self.empty &= text.trim().is_empty();
                if self.fits && text.len() <= LIVE_REQUEST_TEXT_MAX_BYTES - self.text.len() {
                    self.text.push_str(text);
                } else {
                    self.fits = false;
                    self.text.clear();
                }
            }
        }
        Ok(())
    }
}
