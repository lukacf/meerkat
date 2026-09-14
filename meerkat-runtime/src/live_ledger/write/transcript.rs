use super::*;
use crate::live_ledger::completion::{LiveChannelControlOutcome, LiveCompletionEvent};
use crate::live_ledger::transcript::LiveDiscontinuity;
use crate::live_ledger::transcript_authority::dsl;

fn invalid(message: impl std::fmt::Display) -> RuntimeStoreError {
    RuntimeStoreError::WriteFailed(message.to_string())
}

pub(in crate::live_ledger) fn reserved_charge(
    state: &dsl::LiveTranscriptMachineState,
) -> Result<LiveResourceCharge, RuntimeStoreError> {
    // The session fence outlives individual channels. Reserve the maximum
    // encoded flag/generation growth without duplicating either owner field.
    let fence = LiveResourceCharge {
        records: 0,
        encoded_bytes: if state.ingress_open {
            let maximum = serde_json::to_vec(&(false, u64::MAX))
                .map_err(invalid)?
                .len();
            let current = serde_json::to_vec(&(state.ingress_open, state.ingress_generation))
                .map_err(invalid)?
                .len();
            u64::try_from(
                maximum
                    .checked_sub(current)
                    .ok_or_else(|| invalid("invalid ingress fence encoding bound"))?,
            )
            .map_err(invalid)?
        } else {
            0
        },
    };
    state
        .accepting_channels
        .union(&state.voice_observation_open)
        .try_fold(fence, |total, channel| {
            let value = |map: &std::collections::BTreeMap<String, u64>| {
                map.get(channel)
                    .copied()
                    .ok_or_else(|| invalid("missing transcript control accounting"))
            };
            let remaining = LiveResourceCharge {
                records: value(&state.control_credit_records)?,
                encoded_bytes: value(&state.control_credit_bytes)?,
            }
            .checked_sub(LiveResourceCharge {
                records: value(&state.control_spent_records)?,
                encoded_bytes: value(&state.control_spent_bytes)?,
            })
            .map_err(invalid)?;
            total.checked_add(remaining).map_err(invalid)
        })
}

pub(in crate::live_ledger) fn maximum_control_charge(
    channel: &meerkat_core::live_execution::LiveChannelId,
) -> Result<LiveResourceCharge, RuntimeStoreError> {
    let budget = crate::live_ledger::completion_budget::CompletionEnvelopeBudgetV1::for_obligation(
        crate::live_resources::LiveCompletionObligation::ChannelControl,
    )
    .map_err(invalid)?;
    // A control step can add one gap-map entry and change numeric fields.
    // Charging 20 bytes for every schema field bounds every u64 digit change,
    // even fields that the step leaves untouched. Removal only reduces size.
    let gap = serde_json::to_vec(&std::collections::BTreeMap::from([(
        u64::MAX,
        channel.as_str(),
    )]))
    .map_err(invalid)?;
    let scalar_growth = u64::try_from(
        dsl::LiveTranscriptMachineState::schema_static()
            .state
            .fields
            .len(),
    )
    .map_err(invalid)?
    .checked_mul(20)
    .ok_or_else(|| invalid("transcript snapshot bound overflow"))?;
    let digest = "f".repeat(64);
    let control_receipts = serde_json::to_vec(&(
        std::collections::BTreeMap::from([(channel.as_str(), digest.as_str())]),
        std::collections::BTreeMap::from([(digest.as_str(), channel.as_str())]),
        std::collections::BTreeMap::from([(digest.as_str(), u64::MAX)]),
    ))
    .map_err(invalid)?;
    let encoded_bytes = budget
        .maximum_encoded_record_bytes()
        .checked_add(crate::live_resources::LIVE_EVENT_STORAGE_ALLOWANCE_BYTES)
        .and_then(|bytes| bytes.checked_add(gap.len() as u64))
        .and_then(|bytes| bytes.checked_add(scalar_growth))
        .and_then(|bytes| bytes.checked_add(64 + 4 * gap.len() as u64))
        .and_then(|bytes| bytes.checked_add(control_receipts.len() as u64))
        .ok_or_else(|| invalid("transcript control bound overflow"))?;
    Ok(LiveResourceCharge {
        records: 1,
        encoded_bytes,
    })
}

impl PreparedLiveLedgerCommit {
    pub(in crate::live_ledger) fn initial_head(
        session_id: &SessionId,
    ) -> Result<LiveLedgerStoredHead, RuntimeStoreError> {
        let request = crate::generated::live_request_state::encode(
            crate::live_ledger::authority::dsl::LiveRequestMachineAuthority::new().state(),
        )
        .map_err(invalid)?;
        let transcript_owner = dsl::LiveTranscriptMachineAuthority::new();
        let transcript = crate::generated::live_transcript_state::encode(transcript_owner.state())
            .map_err(invalid)?;
        let encoded_bytes = LIVE_HEAD_STORAGE_ALLOWANCE_BYTES
            .checked_add(request.len() as u64)
            .and_then(|bytes| bytes.checked_add(transcript.len() as u64))
            .ok_or_else(|| invalid("initial Live snapshot overflow"))?;
        Ok(LiveLedgerStoredHead {
            reference: LiveHeadReference {
                format: LiveLedgerFormatV1::V1,
                session_id: session_id.clone(),
                generation: 1,
                revision: 0,
                event_count: 0,
                prefix_digest: LiveLedgerPrefixDigest::empty(session_id, 1),
            },
            payload: LiveLedgerPayloadState {
                used: LiveResourceCharge {
                    records: 0,
                    encoded_bytes,
                },
                reserved: reserved_charge(transcript_owner.state())?,
                ingress_generation: 1,
                transcript_snapshot: Arc::new(transcript),
                request_snapshot: Arc::new(request),
            },
        })
    }

    pub(in crate::live_ledger) fn from_transcript_record(
        session_id: &SessionId,
        before: Option<&LiveLedgerStoredHead>,
        input: dsl::LiveTranscriptInput,
        record: LiveLedgerRecord,
    ) -> Result<(Self, dsl::LiveTranscriptMachineTransition), RuntimeStoreError> {
        let (commit, transition) =
            Self::prepare_transcript_record(session_id, before, input, record)?;
        Ok((
            commit.ok_or_else(|| invalid("transcript transition did not authorize an append"))?,
            transition,
        ))
    }

    pub(in crate::live_ledger) fn prepare_transcript_record(
        session_id: &SessionId,
        before: Option<&LiveLedgerStoredHead>,
        mut input: dsl::LiveTranscriptInput,
        record: LiveLedgerRecord,
    ) -> Result<(Option<Self>, dsl::LiveTranscriptMachineTransition), RuntimeStoreError> {
        let mut successor = match before {
            Some(head) => {
                head.validate_payload()?;
                head.clone()
            }
            None => Self::initial_head(session_id)?,
        };
        if successor.reference.session_id != *session_id
            || successor.reference.event_count.checked_add(1) != Some(record.sequence().get())
        {
            return Err(invalid(
                "transcript record is not the next exact session event",
            ));
        }
        let encoded = record.encode().map_err(invalid)?;
        validate_record_binding(session_id, before, &input, &record)?;
        let predecessor = dsl::LiveTranscriptMachineAuthority::recover_from_state(
            crate::generated::live_transcript_state::decode(&successor.payload.transcript_snapshot)
                .map_err(invalid)?,
        )
        .map_err(invalid)?;
        let charge = LiveResourceCharge::for_event_record(&encoded).map_err(invalid)?;
        if let Some(bytes) = control_bytes(&mut input) {
            *bytes = charge.encoded_bytes;
        }
        // Spent-byte digits themselves affect the snapshot. Re-evaluate the
        // generated candidate until the exact measured debit is stable.
        for _ in 0..=20 {
            let mut candidate = predecessor.prepare_authority();
            let transition =
                dsl::LiveTranscriptMachineMutator::apply(&mut candidate, input.clone())
                    .map_err(invalid)?;
            if matches!(
                transition.effects(),
                [dsl::LiveTranscriptEffect::ProviderControlUnchanged { .. }
                    | dsl::LiveTranscriptEffect::ProviderControlRefused { .. }]
            ) {
                if candidate.state() != predecessor.state() {
                    return Err(invalid(
                        "non-appending provider control mutated owner state",
                    ));
                }
                return Ok((None, transition));
            }
            let snapshot = crate::generated::live_transcript_state::encode(candidate.state())
                .map_err(invalid)?;
            let snapshot_growth = snapshot
                .len()
                .saturating_sub(successor.payload.transcript_snapshot.len())
                as u64;
            if let Some(bytes) = control_bytes(&mut input) {
                let actual = charge
                    .encoded_bytes
                    .checked_add(snapshot_growth)
                    .ok_or_else(|| invalid("transcript control debit overflow"))?;
                if *bytes != actual {
                    if actual < *bytes {
                        return Err(invalid("non-monotonic transcript control debit"));
                    }
                    *bytes = actual;
                    continue;
                }
            }
            successor.reference.revision = successor
                .reference
                .revision
                .checked_add(1)
                .ok_or_else(|| invalid("Live revision overflow"))?;
            successor.reference.event_count = record.sequence().get();
            successor.reference.prefix_digest = successor
                .reference
                .prefix_digest
                .appended(record.sequence(), &encoded);
            successor.payload.used.encoded_bytes = successor
                .payload
                .used
                .encoded_bytes
                .checked_sub(successor.payload.transcript_snapshot.len() as u64)
                .and_then(|bytes| bytes.checked_add(snapshot.len() as u64))
                .ok_or_else(|| invalid("transcript snapshot charge overflow"))?;
            successor.payload.used = successor
                .payload
                .used
                .checked_add(charge)
                .map_err(invalid)?;
            successor.payload.reserved = successor
                .payload
                .reserved
                .checked_sub(reserved_charge(predecessor.state())?)
                .map_err(invalid)?
                .checked_add(reserved_charge(candidate.state())?)
                .map_err(invalid)?;
            successor.payload.transcript_snapshot = Arc::new(snapshot);
            let commit = Self {
                purpose: LiveLedgerWritePurpose::ComponentMutation,
                expected: before.map(|head| head.reference.clone()),
                expected_actor: None,
                expected_lifecycle: None,
                successor,
                records: vec![record],
                sources: Vec::new(),
                input_admission: None,
                input_stage: None,
                input_read_fences: Vec::new(),
                quota: crate::live_resources::LIVE_LEDGER_MAX_CHARGE,
            };
            commit.validate(before, &[encoded], LiveSourceChargeDelta::default())?;
            return Ok((Some(commit), transition));
        }
        Err(invalid("transcript control debit did not converge"))
    }
}

fn control_bytes(input: &mut dsl::LiveTranscriptInput) -> Option<&mut u64> {
    match input {
        dsl::LiveTranscriptInput::RecordKnownGap { record_bytes, .. }
        | dsl::LiveTranscriptInput::FenceKnownReceiveTail { record_bytes, .. }
        | dsl::LiveTranscriptInput::RecoverUnknownTail { record_bytes, .. }
        | dsl::LiveTranscriptInput::ObserveProviderControl { record_bytes, .. }
        | dsl::LiveTranscriptInput::CloseChannel { record_bytes, .. } => Some(record_bytes),
        dsl::LiveTranscriptInput::ObserveVoiceUsage {
            record_bytes, kind, ..
        } if *kind != dsl::LiveVoiceUsageKind::Periodic => Some(record_bytes),
        _ => None,
    }
}

fn validate_record_binding(
    session: &SessionId,
    before: Option<&LiveLedgerStoredHead>,
    input: &dsl::LiveTranscriptInput,
    record: &LiveLedgerRecord,
) -> Result<(), RuntimeStoreError> {
    use dsl::LiveTranscriptInput as Input;
    let channel = record.channel_id().as_str();
    let sequence = record.sequence().get();
    if let LiveLedgerRecord::Completion(completion) = record
        && completion.session_id != *session
    {
        return Err(invalid("foreign transcript completion"));
    }
    let matches = match (input, record) {
        (
            Input::ObserveProviderControl {
                channel: expected,
                sequence: seq,
                kind,
                digest,
                ..
            },
            LiveLedgerRecord::Completion(completion),
        ) => {
            let expected_kind = match &completion.event {
                LiveCompletionEvent::ChannelProviderStarted { provider_session }
                    if !provider_session.as_str().is_empty() =>
                {
                    dsl::LiveProviderControlKind::Started
                }
                LiveCompletionEvent::ChannelProviderDiagnostic { .. } => {
                    dsl::LiveProviderControlKind::Diagnostic
                }
                _ => return Err(invalid("invalid provider control record")),
            };
            expected == channel
                && *seq == sequence
                && *kind == expected_kind
                && *digest
                    == crate::live_ledger::transcript_authority::provider_control::digest(
                        session,
                        record.channel_id(),
                        &completion.event,
                    )?
        }
        (
            Input::ObserveVoiceUsage {
                channel: expected,
                sequence: seq,
                kind,
                seconds_bits,
                digest,
                ..
            },
            LiveLedgerRecord::Completion(completion),
        ) => {
            let head =
                before.ok_or_else(|| invalid("voice usage requires an activated channel"))?;
            let owner = dsl::LiveTranscriptMachineAuthority::recover_from_state(
                crate::generated::live_transcript_state::decode(&head.payload.transcript_snapshot)
                    .map_err(invalid)?,
            )
            .map_err(invalid)?;
            let mut candidate = owner.prepare_authority();
            let mut probe = input.clone();
            if let Input::ObserveVoiceUsage { record_bytes, .. } = &mut probe {
                *record_bytes = 1;
            }
            let transition =
                dsl::LiveTranscriptMachineMutator::apply(&mut candidate, probe).map_err(invalid)?;
            expected == channel
                && *seq == sequence
                && *digest
                    == crate::live_ledger::transcript_authority::voice_usage::digest(
                        *kind,
                        *seconds_bits,
                    )?
                && matches!(transition.effects(), [dsl::LiveTranscriptEffect::VoiceUsageRecorded { channel: echoed, sequence: echoed_sequence }]
                    if echoed == channel && *echoed_sequence == sequence)
                && matches!(&completion.event, LiveCompletionEvent::ChannelUsage { snapshot }
                    if Some(snapshot.clone()) == crate::live_ledger::transcript_authority::voice_usage::snapshot(candidate.state(), channel)?)
        }
        (
            Input::ActivateChannel {
                channel: expected,
                sequence: seq,
                credit_records,
                credit_bytes,
                maximum_record_charge,
                ..
            },
            LiveLedgerRecord::Completion(completion),
        ) => {
            let maximum = maximum_control_charge(record.channel_id())?;
            let required = maximum
                .checked_mul(
                    crate::live_resources::LiveCompletionObligation::ChannelControl.record_limit(),
                )
                .map_err(invalid)?;
            expected == channel
                && *seq == sequence
                && *credit_records == required.records
                && *credit_bytes == required.encoded_bytes
                && *maximum_record_charge == maximum.encoded_bytes
                && matches!(&completion.event, LiveCompletionEvent::ChannelControl {
                    outcome: LiveChannelControlOutcome::Activated, diagnostic
                } if diagnostic.as_str().is_empty())
        }
        (
            Input::AppendObservation {
                channel: expected,
                sequence: seq,
                ..
            },
            LiveLedgerRecord::Observation(_),
        ) => expected == channel && *seq == sequence,
        (
            Input::RecordKnownGap {
                channel: expected,
                sequence: seq,
                after_received,
                through_received,
                ..
            }
            | Input::FenceKnownReceiveTail {
                channel: expected,
                sequence: seq,
                after_received,
                through_received,
                ..
            },
            LiveLedgerRecord::Completion(completion),
        ) => {
            expected == channel
                && *seq == sequence
                && matches!(&completion.event,
                LiveCompletionEvent::ChannelDiscontinuity {
                    discontinuity: LiveDiscontinuity::KnownLocalGap { channel_id, observed_bounds }
                } if channel_id.as_str() == channel
                    && observed_bounds.after_received_ordinal() == *after_received
                    && observed_bounds.through_received_ordinal() == *through_received)
        }
        (
            Input::RecoverUnknownTail {
                channel: expected,
                sequence: seq,
                ..
            },
            LiveLedgerRecord::Completion(completion),
        ) => {
            expected == channel
                && *seq == sequence
                && matches!(&completion.event,
                LiveCompletionEvent::ChannelDiscontinuity {
                    discontinuity: LiveDiscontinuity::UnknownExtentCrashDiscontinuity { old_incarnation, last_accepted_head }
                } if old_incarnation.as_str() == channel
                    && before.is_some_and(|head| head.reference == *last_accepted_head))
        }
        (
            Input::CloseChannel {
                channel: expected,
                sequence: seq,
                ..
            },
            LiveLedgerRecord::Completion(completion),
        ) => {
            expected == channel
                && *seq == sequence
                && matches!(&completion.event,
                LiveCompletionEvent::ChannelControl {
                    outcome: LiveChannelControlOutcome::IngressClosed, diagnostic
                } if diagnostic.as_str().is_empty())
        }
        _ => false,
    };
    if !matches {
        return Err(invalid("transcript command and exact record disagree"));
    }
    Ok(())
}
