use std::sync::Arc;

use meerkat_contracts::wire::live_observation::{
    LiveObservationRecord, LiveObservationWireCodecV1,
};
use meerkat_core::{
    SessionId,
    live_execution::LiveChannelId,
    live_observation::{LiveObservationSeq, LiveTranscriptObservation},
};

use super::{CommittedLiveProviderControl, dsl};
use crate::live_ledger::{
    completion::{
        LiveChannelControlOutcome, LiveCompletionEvent, LiveCompletionRecord, LiveCompletionText,
    },
    record::LiveLedgerRecord,
    transcript::{
        KnownLiveReceiveGap, LiveDiscontinuity, LiveHeadReference, LiveLedgerFormatV1,
        StoredLiveObservation,
    },
    write::{
        LiveLedgerCommitOutcome, LiveLedgerStoredHead, PreparedLiveLedgerCommit,
        transcript_commit::maximum_control_charge,
    },
};
use crate::store::{
    MachineLifecycleObservationVersion, RuntimeStore, RuntimeStoreError, RuntimeStoreWriteFence,
};

/// Native observation persistence. This owner grants no executor permission.
/// Hosts must install their current registration fence and perform cold-tail
/// recovery before issuing replacement channel incarnations.
pub struct LiveTranscriptStoreOwner {
    store: Arc<dyn RuntimeStore>,
    session: SessionId,
    fence: Arc<dyn RuntimeStoreWriteFence>,
}

/// Exclusive local receive identity. It cannot be cloned, deserialized, or
/// reconstructed for an old incarnation after process loss.
pub struct LiveTranscriptChannelIngress {
    owner: LiveTranscriptStoreOwner,
    channel: LiveChannelId,
    receive_clock: meerkat_core::live_observation::LiveObservationReceiveClock,
    last_delivered: u64,
    ingress_generation: u64,
    unconfirmed_source: Option<meerkat_core::live_execution::request::LiveSourceKey>,
}

#[derive(Debug)]
pub struct CommittedLiveObservation {
    head: LiveHeadReference,
    observation: StoredLiveObservation,
}

/// Source replay is observation only. Only a newly committed admission
/// carries the ordinary owner's sealed execution handoff.
pub enum LiveClientDelegationOutcome {
    Source(super::LiveSourceReservationOutcome),
    Admitted {
        authority: crate::live_request::AdmittedLiveExecutionAuthority,
        completion: Option<crate::completion::CompletionHandle>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum LiveClientDelegationError {
    #[error(transparent)]
    Source(#[from] super::LiveSourceReservationError),
    #[error(transparent)]
    Admission(#[from] crate::RuntimeDriverError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommittedLiveVoiceUsage {
    head: LiveHeadReference,
    sequence: meerkat_core::live_observation::LiveObservationSeq,
    snapshot: meerkat_core::live_execution::observation::LiveUsageSnapshot,
}

impl CommittedLiveVoiceUsage {
    pub fn head(&self) -> &LiveHeadReference {
        &self.head
    }
    pub fn sequence(&self) -> meerkat_core::live_observation::LiveObservationSeq {
        self.sequence
    }
    pub fn snapshot(&self) -> &meerkat_core::live_execution::observation::LiveUsageSnapshot {
        &self.snapshot
    }
}

impl CommittedLiveObservation {
    pub fn head(&self) -> &LiveHeadReference {
        &self.head
    }
    pub fn observation(&self) -> &StoredLiveObservation {
        &self.observation
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LiveTranscriptWriteError {
    #[error(transparent)]
    InvalidObservation(#[from] meerkat_core::live_observation::LiveObservationValueError),
    #[error("runtime store has no Live transcript realization")]
    Unsupported,
    #[error("Live transcript channel is absent, retired, or has foreign receive ownership")]
    Retired,
    #[error("Live transcript sequence exhausted")]
    SequenceExhausted,
    #[error("Live usage head conflict retry budget exhausted")]
    UsageHeadContention,
    #[error("channel source-fence head conflict retry budget exhausted")]
    SourceFenceHeadContention,
    #[error("Live provider control head conflict retry budget exhausted")]
    ProviderControlHeadContention,
    #[error("timed out observing a committed Live provider start")]
    ProviderStartWaitTimedOut,
    #[error("Live transcript transition was not newly committed: {0:?}")]
    NotCommitted(Box<LiveLedgerCommitOutcome>),
    #[error(transparent)]
    Store(#[from] RuntimeStoreError),
    #[error(transparent)]
    Codec(#[from] serde_json::Error),
    #[error(transparent)]
    Transition(#[from] dsl::LiveTranscriptMachineTransitionError),
    #[error(transparent)]
    Observation(#[from] meerkat_contracts::wire::live_observation::LiveObservationEncodingError),
    #[error(transparent)]
    Completion(#[from] crate::live_ledger::completion::LiveCompletionEncodingError),
    #[error(transparent)]
    Gap(#[from] crate::live_ledger::transcript::LiveGapError),
    #[error(transparent)]
    Arithmetic(#[from] crate::live_resources::LiveResourceArithmeticError),
}

impl LiveTranscriptStoreOwner {
    pub async fn observe_provider_control(
        &self,
        channel: &LiveChannelId,
        observation: &meerkat_core::live_execution::observation::ContinuousLiveObservation,
    ) -> Result<super::LiveProviderControlOutcome, LiveTranscriptWriteError> {
        use super::{CommittedLiveProviderControl, LiveProviderControlOutcome};
        let (kind, event) = super::provider_control::event(observation)?;
        let digest = super::provider_control::digest(&self.session, channel, &event)?;
        for _ in 0..8 {
            let before = self
                .load()
                .await?
                .ok_or(LiveTranscriptWriteError::Retired)?;
            let sequence = next_sequence(Some(&before))?;
            let (prepared, transition) = PreparedLiveLedgerCommit::prepare_transcript_record(
                &self.session,
                Some(&before),
                dsl::LiveTranscriptInput::ObserveProviderControl {
                    channel: channel.to_string(),
                    sequence: sequence.get(),
                    kind,
                    digest: digest.clone(),
                    record_bytes: 1,
                },
                self.completion(channel, sequence, event.clone()),
            )?;
            match (prepared, transition.effects()) {
                (
                    None,
                    [
                        dsl::LiveTranscriptEffect::ProviderControlUnchanged {
                            channel: echoed,
                            digest: echoed_digest,
                            sequence: previous,
                        },
                    ],
                ) if echoed == channel.as_str() && echoed_digest == &digest => {
                    return Ok(LiveProviderControlOutcome::Accepted(
                        CommittedLiveProviderControl {
                            head: before.reference,
                            sequence: LiveObservationSeq::new(*previous)?,
                        },
                    ));
                }
                (
                    None,
                    [
                        dsl::LiveTranscriptEffect::ProviderControlRefused {
                            channel: echoed,
                            reason,
                        },
                    ],
                ) if echoed == channel.as_str() => {
                    return Ok(LiveProviderControlOutcome::Refused(*reason));
                }
                (
                    Some(prepared),
                    [
                        dsl::LiveTranscriptEffect::ProviderControlRecorded {
                            channel: echoed,
                            digest: echoed_digest,
                            sequence: echoed_sequence,
                        },
                    ],
                ) if echoed == channel.as_str()
                    && echoed_digest == &digest
                    && *echoed_sequence == sequence.get() =>
                {
                    match self.publish(prepared).await {
                        Ok(head) => {
                            return Ok(LiveProviderControlOutcome::Accepted(
                                CommittedLiveProviderControl { head, sequence },
                            ));
                        }
                        Err(LiveTranscriptWriteError::NotCommitted(outcome))
                            if matches!(*outcome, LiveLedgerCommitOutcome::Conflict { .. }) => {}
                        Err(error) => return Err(error),
                    }
                }
                _ => {
                    return Err(RuntimeStoreError::WriteFailed(
                        "unexpected provider control transition".into(),
                    )
                    .into());
                }
            }
        }
        Err(LiveTranscriptWriteError::ProviderControlHeadContention)
    }

    /// Provider start only; answer delivery and media readiness have other owners.
    pub async fn read_provider_start(
        &self,
        channel: &LiveChannelId,
    ) -> Result<Option<CommittedLiveProviderControl>, LiveTranscriptWriteError> {
        let Some(head) = self.load().await? else {
            return Ok(None);
        };
        let owner = self.restore(Some(&head))?;
        let Some(digest) = owner.state().provider_start_digests.get(channel.as_str()) else {
            return Ok(None);
        };
        let sequence = owner
            .state()
            .provider_control_sequences
            .get(digest)
            .copied()
            .ok_or_else(|| {
                RuntimeStoreError::ReadFailed("missing provider start receipt".into())
            })?;
        Ok(Some(CommittedLiveProviderControl {
            head: head.reference,
            sequence: LiveObservationSeq::new(sequence)?,
        }))
    }

    pub fn new(
        store: Arc<dyn RuntimeStore>,
        session: SessionId,
        fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Self {
        Self {
            store,
            session,
            fence,
        }
    }

    /// Reconcile exact provider observations against the durable account.
    /// Only physical observation closure (after ingress closure) releases its
    /// remaining control reserve. A provider final alone does not release it.
    pub async fn observe_voice_usage(
        &self,
        channel: &LiveChannelId,
        observation: &meerkat_core::live_execution::observation::LiveUsageSnapshot,
    ) -> Result<CommittedLiveVoiceUsage, LiveTranscriptWriteError> {
        for _ in 0..8 {
            let before = self
                .load()
                .await?
                .ok_or(LiveTranscriptWriteError::Retired)?;
            let owner = self.restore(Some(&before))?;
            let input = super::voice_usage::input(
                channel.as_str(),
                next_sequence(Some(&before))?.get(),
                observation,
            )?;
            let mut candidate = owner.prepare_authority();
            let transition =
                dsl::LiveTranscriptMachineMutator::apply(&mut candidate, input.clone())?;
            let sequence = meerkat_core::live_observation::LiveObservationSeq::new(
                *candidate
                    .state()
                    .voice_usage_sequences
                    .get(channel.as_str())
                    .ok_or(LiveTranscriptWriteError::Retired)?,
            )?;
            let snapshot = super::voice_usage::snapshot(candidate.state(), channel.as_str())?
                .ok_or(LiveTranscriptWriteError::Retired)?;
            if matches!(transition.effects(), [dsl::LiveTranscriptEffect::VoiceUsageUnchanged { channel: echoed, sequence: echoed_sequence }]
                if echoed == channel.as_str() && *echoed_sequence == sequence.get())
            {
                return Ok(CommittedLiveVoiceUsage {
                    head: before.reference,
                    sequence,
                    snapshot,
                });
            }
            if !matches!(transition.effects(), [dsl::LiveTranscriptEffect::VoiceUsageRecorded { channel: echoed, sequence: echoed_sequence }]
                if echoed == channel.as_str() && *echoed_sequence == sequence.get())
            {
                return Err(RuntimeStoreError::WriteFailed(
                    "unexpected Live usage transition effect".into(),
                )
                .into());
            }
            let record = self.completion(
                channel,
                sequence,
                LiveCompletionEvent::ChannelUsage {
                    snapshot: snapshot.clone(),
                },
            );
            let (prepared, _) = PreparedLiveLedgerCommit::from_transcript_record(
                &self.session,
                Some(&before),
                input,
                record,
            )?;
            match self.publish(prepared).await {
                Ok(head) => {
                    return Ok(CommittedLiveVoiceUsage {
                        head,
                        sequence,
                        snapshot,
                    });
                }
                Err(LiveTranscriptWriteError::NotCommitted(outcome))
                    if matches!(*outcome, LiveLedgerCommitOutcome::Conflict { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        Err(LiveTranscriptWriteError::UsageHeadContention)
    }

    pub async fn read_voice_usage(
        &self,
        channel: &LiveChannelId,
    ) -> Result<
        Option<meerkat_core::live_execution::observation::LiveUsageSnapshot>,
        LiveTranscriptWriteError,
    > {
        let Some(head) = self.load().await? else {
            return Ok(None);
        };
        let owner = self.restore(Some(&head))?;
        if !owner.state().voice_channels.contains(channel.as_str()) {
            return Ok(None);
        }
        Ok(super::voice_usage::snapshot(
            owner.state(),
            channel.as_str(),
        )?)
    }

    pub async fn activate_channel(
        self,
        channel: LiveChannelId,
        lifecycle: MachineLifecycleObservationVersion,
    ) -> Result<LiveTranscriptChannelIngress, LiveTranscriptWriteError> {
        self.activate(channel, lifecycle, false).await
    }

    /// Retain provider-control accounting beyond text ingress closure.
    pub async fn activate_voice_channel(
        self,
        channel: LiveChannelId,
        lifecycle: MachineLifecycleObservationVersion,
    ) -> Result<LiveTranscriptChannelIngress, LiveTranscriptWriteError> {
        self.activate(channel, lifecycle, true).await
    }

    async fn activate(
        self,
        channel: LiveChannelId,
        lifecycle: MachineLifecycleObservationVersion,
        voice_accounting: bool,
    ) -> Result<LiveTranscriptChannelIngress, LiveTranscriptWriteError> {
        if channel.as_str().is_empty()
            || channel.as_str().len() > crate::live_ledger::completion::LIVE_COMPLETION_ID_MAX_BYTES
        {
            return Err(meerkat_contracts::wire::live_observation::LiveObservationEncodingError::InvalidIdentity.into());
        }
        let ops = self
            .store
            .live_ledger_ops()
            .ok_or(LiveTranscriptWriteError::Unsupported)?;
        if !ops.ledger_write_profile().supports_lifecycle_fence() {
            return Err(LiveTranscriptWriteError::Unsupported);
        }
        let before = ops.load_live_head(&self.session).await?;
        let owner = self.restore(before.as_ref())?;
        let sequence = next_sequence(before.as_ref())?;
        let maximum = maximum_control_charge(&channel)?;
        let total = maximum.checked_mul(
            crate::live_resources::LiveCompletionObligation::ChannelControl.record_limit(),
        )?;
        let input = dsl::LiveTranscriptInput::ActivateChannel {
            voice_accounting,
            channel: channel.to_string(),
            sequence: sequence.get(),
            ingress_generation: owner.state().ingress_generation,
            credit_records: total.records,
            credit_bytes: total.encoded_bytes,
            maximum_record_charge: maximum.encoded_bytes,
        };
        let record = self.completion(
            &channel,
            sequence,
            LiveCompletionEvent::ChannelControl {
                outcome: LiveChannelControlOutcome::Activated,
                diagnostic: LiveCompletionText::new("")?,
            },
        );
        let (prepared, _) = PreparedLiveLedgerCommit::from_transcript_record(
            &self.session,
            before.as_ref(),
            input,
            record,
        )?;
        self.publish(prepared.with_lifecycle_fence(lifecycle))
            .await?;
        Ok(LiveTranscriptChannelIngress {
            owner: self,
            channel,
            receive_clock: Default::default(),
            last_delivered: 0,
            ingress_generation: owner.state().ingress_generation,
            unconfirmed_source: None,
        })
    }

    /// Explicit cold-process recovery; never invents a local receive count.
    /// Each channel is fenced with a retained-head witness in its own CAS.
    pub async fn recover_unknown_tails(
        &self,
    ) -> Result<Vec<LiveHeadReference>, LiveTranscriptWriteError> {
        let before = self.load().await?;
        let owner = self.restore(before.as_ref())?;
        let channels: Vec<_> = owner.state().accepting_channels.iter().cloned().collect();
        let mut committed = Vec::with_capacity(channels.len());
        for channel in channels {
            let before = self
                .load()
                .await?
                .ok_or(LiveTranscriptWriteError::Retired)?;
            let current = self.restore(Some(&before))?;
            if !current.state().accepting_channels.contains(&channel) {
                continue;
            }
            let sequence = next_sequence(Some(&before))?;
            let channel = LiveChannelId::new(channel);
            let record = self.completion(
                &channel,
                sequence,
                LiveCompletionEvent::ChannelDiscontinuity {
                    discontinuity: LiveDiscontinuity::UnknownExtentCrashDiscontinuity {
                        last_accepted_head: before.reference.clone(),
                        old_incarnation: channel.clone(),
                    },
                },
            );
            let (prepared, _) = PreparedLiveLedgerCommit::from_transcript_record(
                &self.session,
                Some(&before),
                dsl::LiveTranscriptInput::RecoverUnknownTail {
                    channel: channel.to_string(),
                    sequence: sequence.get(),
                    record_bytes: 0,
                },
                record,
            )?;
            committed.push(self.publish(prepared).await?);
        }
        Ok(committed)
    }

    async fn load(&self) -> Result<Option<LiveLedgerStoredHead>, LiveTranscriptWriteError> {
        Ok(self
            .store
            .live_ledger_ops()
            .ok_or(LiveTranscriptWriteError::Unsupported)?
            .load_live_head(&self.session)
            .await?)
    }

    fn restore(
        &self,
        head: Option<&LiveLedgerStoredHead>,
    ) -> Result<dsl::LiveTranscriptMachineAuthority, LiveTranscriptWriteError> {
        match head {
            None => Ok(dsl::LiveTranscriptMachineAuthority::new()),
            Some(head) => {
                head.validate_payload()?;
                if head.reference.session_id != self.session {
                    return Err(LiveTranscriptWriteError::Retired);
                }
                Ok(dsl::LiveTranscriptMachineAuthority::recover_from_state(
                    crate::generated::live_transcript_state::decode(
                        &head.payload.transcript_snapshot,
                    )?,
                )?)
            }
        }
    }

    fn completion(
        &self,
        channel: &LiveChannelId,
        sequence: LiveObservationSeq,
        event: LiveCompletionEvent,
    ) -> LiveLedgerRecord {
        LiveLedgerRecord::Completion(LiveCompletionRecord {
            format: LiveLedgerFormatV1::V1,
            session_id: self.session.clone(),
            channel_id: channel.clone(),
            sequence,
            event,
        })
    }

    async fn publish(
        &self,
        prepared: PreparedLiveLedgerCommit,
    ) -> Result<LiveHeadReference, LiveTranscriptWriteError> {
        match self
            .store
            .live_ledger_ops()
            .ok_or(LiveTranscriptWriteError::Unsupported)?
            .commit_live_ledger(prepared, Arc::clone(&self.fence))
            .await?
        {
            LiveLedgerCommitOutcome::Committed { head } => Ok(head),
            outcome => Err(LiveTranscriptWriteError::NotCommitted(Box::new(outcome))),
        }
    }
}

impl LiveTranscriptChannelIngress {
    /// Freeze the exact selected source before ordinary admission. The owning
    /// host supplies its currently resolved grant for this invocation; the
    /// channel does not cache a grant or reinterpret a replay as permission.
    pub async fn reserve_and_admit_client_source(
        &mut self,
        runtime: &crate::MeerkatMachine,
        delegation: meerkat_core::live_execution::request::LiveProviderReference,
        offset_ms: f64,
        grant: Option<&crate::live_grant::LiveExecutionGrant<()>>,
    ) -> Result<LiveClientDelegationOutcome, LiveClientDelegationError> {
        let source = self
            .reserve_client_source(delegation, offset_ms, grant)
            .await?;
        if let super::LiveSourceReservationOutcome::Retained(entry) = &source
            && let crate::live_source::LiveSourceEntryRecord::Reservation { record } =
                entry.as_ref()
            && matches!(
                record.disposition(),
                crate::live_source::LiveSourceDisposition::Reserved {}
            )
            && let Some(grant) = grant
        {
            let (authority, completion) = runtime
                .commit_live_input_admission(record.source().clone(), grant)
                .await?;
            return Ok(LiveClientDelegationOutcome::Admitted {
                authority,
                completion,
            });
        }
        Ok(LiveClientDelegationOutcome::Source(source))
    }

    pub async fn observe_provider_control(
        &self,
        event: &meerkat_core::live_execution::observation::ContinuousLiveObservation,
    ) -> Result<super::LiveProviderControlOutcome, LiveTranscriptWriteError> {
        self.owner
            .observe_provider_control(&self.channel, event)
            .await
    }

    pub async fn read_provider_start(
        &self,
    ) -> Result<Option<CommittedLiveProviderControl>, LiveTranscriptWriteError> {
        self.owner.read_provider_start(&self.channel).await
    }

    pub fn session_id(&self) -> &SessionId {
        &self.owner.session
    }

    pub async fn observation_closure_committed(&self) -> Result<bool, LiveTranscriptWriteError> {
        let head = self
            .owner
            .load()
            .await?
            .ok_or(LiveTranscriptWriteError::Retired)?;
        let owner = self.owner.restore(Some(&head))?;
        if !owner.state().voice_channels.contains(self.channel.as_str()) {
            return Err(LiveTranscriptWriteError::Retired);
        }
        Ok(!owner
            .state()
            .voice_observation_open
            .contains(self.channel.as_str()))
    }

    /// Fence new source selection and initial admissions while the sole
    /// observation reader finishes applying already-received provider facts.
    pub async fn begin_drain(&self) -> Result<LiveHeadReference, LiveTranscriptWriteError> {
        for _ in 0..8 {
            let before = self
                .owner
                .load()
                .await?
                .ok_or(LiveTranscriptWriteError::Retired)?;
            let owner = self.owner.restore(Some(&before))?;
            let mut transcript = owner.prepare_authority();
            let transition = dsl::LiveTranscriptMachineMutator::apply(
                &mut transcript,
                dsl::LiveTranscriptInput::FenceChannelSources {
                    channel: self.channel.to_string(),
                },
            )?;
            if !matches!(transition.effects(), [dsl::LiveTranscriptEffect::ChannelSourcesFenced { channel }]
                if channel == self.channel.as_str())
            {
                return Err(RuntimeStoreError::WriteFailed(
                    "unexpected channel source-fence effect".into(),
                )
                .into());
            }
            if transcript.state() == owner.state() {
                return Ok(before.reference);
            }
            let request = crate::live_ledger::authority::dsl::LiveRequestMachineAuthority::recover_from_state(
                crate::generated::live_request_state::decode(&before.payload.request_snapshot)?,
            ).map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            let prepared = PreparedLiveLedgerCommit::from_request_transition(
                &self.owner.session,
                Some(&before),
                &request.prepare_authority(),
            )?
            .with_transcript_transition(&transcript)?;
            match self.owner.publish(prepared).await {
                Ok(head) => return Ok(head),
                Err(LiveTranscriptWriteError::NotCommitted(outcome))
                    if matches!(*outcome, LiveLedgerCommitOutcome::Conflict { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        Err(LiveTranscriptWriteError::SourceFenceHeadContention)
    }

    pub async fn observe_voice_usage(
        &self,
        observation: &meerkat_core::live_execution::observation::LiveUsageSnapshot,
    ) -> Result<CommittedLiveVoiceUsage, LiveTranscriptWriteError> {
        self.owner
            .observe_voice_usage(&self.channel, observation)
            .await
    }
    pub async fn reserve_client_source(
        &mut self,
        delegation: meerkat_core::live_execution::request::LiveProviderReference,
        offset_ms: f64,
        grant: Option<&crate::live_grant::LiveExecutionGrant<()>>,
    ) -> Result<super::LiveSourceReservationOutcome, super::LiveSourceReservationError> {
        use meerkat_core::live_execution::request::{LiveSourceIdentity, LiveSourceKey};
        let source = LiveSourceKey::new(
            self.owner.session.clone(),
            self.channel.clone(),
            LiveSourceIdentity::ClientDelegation { delegation },
        )
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        self.reserve_source(
            source,
            crate::live_source::LiveSourceFingerprint::client_delegation(offset_ms)?,
            None,
            grant,
        )
        .await
    }

    pub async fn reserve_application_source(
        &mut self,
        request_id: meerkat_core::live_execution::request::LiveApplicationRequestId,
        interval: meerkat_core::live_execution::evidence::LiveObservationInterval,
        grant: Option<&crate::live_grant::LiveExecutionGrant<()>>,
    ) -> Result<super::LiveSourceReservationOutcome, super::LiveSourceReservationError> {
        use meerkat_core::live_execution::request::{LiveSourceIdentity, LiveSourceKey};
        let source = LiveSourceKey::new(
            self.owner.session.clone(),
            self.channel.clone(),
            LiveSourceIdentity::ApplicationRequest { request_id },
        )
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        self.reserve_source(
            source,
            crate::live_source::LiveSourceFingerprint::application_request(interval),
            Some(interval),
            grant,
        )
        .await
    }

    async fn reserve_source(
        &mut self,
        source: meerkat_core::live_execution::request::LiveSourceKey,
        fingerprint: crate::live_source::LiveSourceFingerprint,
        explicit: Option<meerkat_core::live_execution::evidence::LiveObservationInterval>,
        grant: Option<&crate::live_grant::LiveExecutionGrant<()>>,
    ) -> Result<super::LiveSourceReservationOutcome, super::LiveSourceReservationError> {
        use crate::live_ledger::authority::store::{
            LiveRequestStoreOwner, source_reservation::SourceSelection,
        };
        LiveRequestStoreOwner::new(Arc::clone(&self.owner.store), self.owner.session.clone())
            .reserve_selected_source(
                SourceSelection {
                    source,
                    fingerprint,
                    explicit,
                    received: self.received_ordinal(),
                    ingress_generation: self.ingress_generation,
                },
                grant,
                Arc::clone(&self.owner.fence),
                &mut self.unconfirmed_source,
            )
            .await
    }

    pub fn channel_id(&self) -> &LiveChannelId {
        &self.channel
    }

    pub fn received_ordinal(&self) -> u64 {
        self.receive_clock.received_ordinal()
    }

    pub fn receive_clock(&self) -> meerkat_core::live_observation::LiveObservationReceiveClock {
        self.receive_clock.clone()
    }

    /// Accounts the actual local receive before any await or encoding refusal.
    /// A lost/failed append is not resent. The next receive settles its exact
    /// still-uncommitted predecessor range before appending new content.
    pub async fn append(
        &mut self,
        observation: LiveTranscriptObservation,
    ) -> Result<LiveHeadReference, LiveTranscriptWriteError> {
        self.append_received_observation(Ok(observation)).await
    }

    /// A known received TEXT with invalid range data still consumes its local
    /// receive ordinal. It cannot disappear and turn a later prefix into Empty.
    pub async fn append_received_observation(
        &mut self,
        observation: Result<
            LiveTranscriptObservation,
            meerkat_core::live_observation::LiveObservationValueError,
        >,
    ) -> Result<LiveHeadReference, LiveTranscriptWriteError> {
        let receipt = self.receive_clock.record_received()?;
        self.append_with_receive_receipt(receipt, observation)
            .await
            .map(|committed| committed.head)
    }

    pub async fn append_with_receive_receipt(
        &mut self,
        receipt: meerkat_core::live_observation::LiveObservationReceiveReceipt,
        observation: Result<
            LiveTranscriptObservation,
            meerkat_core::live_observation::LiveObservationValueError,
        >,
    ) -> Result<CommittedLiveObservation, LiveTranscriptWriteError> {
        if !receipt.belongs_to(&self.receive_clock) || receipt.ordinal() <= self.last_delivered {
            return Err(LiveTranscriptWriteError::Retired);
        }
        let received = receipt.ordinal();
        self.last_delivered = received;
        let observation = observation?;
        let maximal = LiveObservationWireCodecV1::check_record_fit(LiveObservationRecord {
            sequence: LiveObservationSeq::new(u64::MAX)
                .map_err(|_| LiveTranscriptWriteError::SequenceExhausted)?,
            channel_id: self.channel.clone(),
            observation,
        })?;
        let (mut before, owner) = self.current().await?;
        let accepted = owner.state().receive_ordinals[self.channel.as_str()];
        if accepted >= received {
            return Err(LiveTranscriptWriteError::Retired);
        }
        if accepted < received - 1 {
            self.commit_gap(&before, accepted, received - 1, false)
                .await?;
            (before, _) = self.current().await?;
        }
        let sequence = next_sequence(Some(&before))?;
        let fit = LiveObservationWireCodecV1::check_record_fit(LiveObservationRecord {
            sequence,
            channel_id: self.channel.clone(),
            observation: maximal.record().observation.clone(),
        })?;
        let observation = StoredLiveObservation::from_fit(&fit);
        let record = LiveLedgerRecord::Observation(observation.clone());
        let (prepared, _) = PreparedLiveLedgerCommit::from_transcript_record(
            &self.owner.session,
            Some(&before),
            dsl::LiveTranscriptInput::AppendObservation {
                channel: self.channel.to_string(),
                sequence: sequence.get(),
                receive_ordinal: received,
                ingress_generation: self.ingress_generation,
            },
            record,
        )?;
        let head = self.owner.publish(prepared).await?;
        Ok(CommittedLiveObservation { head, observation })
    }

    /// The final reserved control slot can close a known missing tail in the
    /// same generated change; it never relabels known loss as an unknown crash.
    pub async fn close(&mut self) -> Result<LiveHeadReference, LiveTranscriptWriteError> {
        let before = self
            .owner
            .load()
            .await?
            .ok_or(LiveTranscriptWriteError::Retired)?;
        let owner = self.owner.restore(Some(&before))?;
        if !owner.state().channels.contains(self.channel.as_str()) {
            return Err(LiveTranscriptWriteError::Retired);
        }
        let accepted = owner.state().receive_ordinals[self.channel.as_str()];
        let received = self.received_ordinal();
        if accepted > received {
            return Err(LiveTranscriptWriteError::Retired);
        }
        if !owner
            .state()
            .accepting_channels
            .contains(self.channel.as_str())
        {
            return if accepted == received {
                Ok(before.reference)
            } else {
                Err(LiveTranscriptWriteError::Retired)
            };
        }
        if accepted < received {
            return self.commit_gap(&before, accepted, received, true).await;
        }
        let sequence = next_sequence(Some(&before))?;
        let record = self.owner.completion(
            &self.channel,
            sequence,
            LiveCompletionEvent::ChannelControl {
                outcome: LiveChannelControlOutcome::IngressClosed,
                diagnostic: LiveCompletionText::new("")?,
            },
        );
        let (prepared, _) = PreparedLiveLedgerCommit::from_transcript_record(
            &self.owner.session,
            Some(&before),
            dsl::LiveTranscriptInput::CloseChannel {
                channel: self.channel.to_string(),
                sequence: sequence.get(),
                record_bytes: 0,
            },
            record,
        )?;
        self.owner.publish(prepared).await
    }

    async fn current(
        &self,
    ) -> Result<(LiveLedgerStoredHead, dsl::LiveTranscriptMachineAuthority), LiveTranscriptWriteError>
    {
        let before = self
            .owner
            .load()
            .await?
            .ok_or(LiveTranscriptWriteError::Retired)?;
        let owner = self.owner.restore(Some(&before))?;
        if !owner
            .state()
            .accepting_channels
            .contains(self.channel.as_str())
        {
            return Err(LiveTranscriptWriteError::Retired);
        }
        Ok((before, owner))
    }

    async fn commit_gap(
        &self,
        before: &LiveLedgerStoredHead,
        after: u64,
        through: u64,
        close: bool,
    ) -> Result<LiveHeadReference, LiveTranscriptWriteError> {
        let sequence = next_sequence(Some(before))?;
        let record = self.owner.completion(
            &self.channel,
            sequence,
            LiveCompletionEvent::ChannelDiscontinuity {
                discontinuity: LiveDiscontinuity::KnownLocalGap {
                    channel_id: self.channel.clone(),
                    observed_bounds: KnownLiveReceiveGap::new(after, through)?,
                },
            },
        );
        let input = if close {
            dsl::LiveTranscriptInput::FenceKnownReceiveTail {
                channel: self.channel.to_string(),
                sequence: sequence.get(),
                after_received: after,
                through_received: through,
                record_bytes: 0,
            }
        } else {
            dsl::LiveTranscriptInput::RecordKnownGap {
                channel: self.channel.to_string(),
                sequence: sequence.get(),
                after_received: after,
                through_received: through,
                record_bytes: 0,
                ingress_generation: self.ingress_generation,
            }
        };
        let (prepared, _) = PreparedLiveLedgerCommit::from_transcript_record(
            &self.owner.session,
            Some(before),
            input,
            record,
        )?;
        self.owner.publish(prepared).await
    }
}

fn next_sequence(
    head: Option<&LiveLedgerStoredHead>,
) -> Result<LiveObservationSeq, LiveTranscriptWriteError> {
    let sequence = head
        .map_or(Some(1), |head| head.reference.event_count.checked_add(1))
        .ok_or(LiveTranscriptWriteError::SequenceExhausted)?;
    LiveObservationSeq::new(sequence).map_err(|_| LiveTranscriptWriteError::SequenceExhausted)
}
