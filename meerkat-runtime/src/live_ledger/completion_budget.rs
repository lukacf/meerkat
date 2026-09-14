//! Versioned completion capacity derived from the actual bounded codec.
//!
//! Every reserved record slot can hold the largest record in its obligation
//! class. This intentionally over-reserves rather than relying on a later
//! outcome, successful delivery, or deduplication to make capacity available.

use std::sync::OnceLock;

use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_execution::observation::{
    LiveUsageDispute, LiveUsageSnapshot, LiveVoiceDurationSeconds,
};
use meerkat_core::live_execution::request::LiveRequestCancellationReason;
use meerkat_core::live_observation::LiveObservationSeq;
use meerkat_core::{
    SessionId,
    lifecycle::{InputId, RunId},
    ops::OperationId,
};
use serde::{Deserialize, Serialize};

use super::completion::{
    LIVE_COMPLETION_ID_MAX_BYTES, LIVE_CONTEXT_CHUNK_MAX_BYTES, LIVE_RESULT_MAX_BYTES,
    LIVE_TERMINAL_DIAGNOSTIC_MAX_BYTES, LiveCallbackContinuationOutcome, LiveChannelControlOutcome,
    LiveCompletionEncodingError, LiveCompletionEvent, LiveCompletionRecord, LiveCompletionText,
    LivePhysicalEffectOutcome, LiveRequestCompletionFact, LiveRequestHold, LiveRequestRefusal,
};
use super::transcript::{
    KnownLiveReceiveGap, LiveDiscontinuity, LiveHeadReference, LiveLedgerFormatV1,
    LiveLedgerPrefixDigest,
};
use crate::live_delivery::{
    LiveContextChunkDeliveryState, LiveContinuationState, LiveResultDeliveryState,
};
use crate::live_resources::{
    LIVE_EVENT_STORAGE_ALLOWANCE_BYTES, LiveCompletionObligation, LiveResourceCharge,
};

pub use crate::live_ledger::authority::dsl::LiveCompletionCreditSchema as CompletionCreditSchema;

pub(crate) fn token_accounting_encoding_extrema()
-> impl Iterator<Item = meerkat_core::execution_scope::ScopedEffectTokenAccounting> {
    use meerkat_core::execution_scope::ScopedEffectTokenAccounting as Accounting;
    [Accounting::NotApplicable {}, Accounting::Unmeasured {}]
        .into_iter()
        .chain(
            meerkat_core::Provider::ALL_CONCRETE
                .iter()
                .copied()
                .chain([meerkat_core::Provider::Other])
                .flat_map(|reported_provider| {
                    [false, true]
                        .into_iter()
                        .map(move |identity_disputed| Accounting::Measured {
                            normalized_tokens: u64::MAX,
                            reported_provider,
                            reported_model_digest: [255; 32],
                            normalized_counter_digest: [255; 32],
                            identity_disputed,
                        })
                }),
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CompletionEnvelopeBudgetWire")]
pub struct CompletionEnvelopeBudgetV1 {
    schema: CompletionCreditSchema,
    obligation: LiveCompletionObligation,
    minimum_encoded_record_bytes: u64,
    maximum_encoded_record_bytes: u64,
    total: LiveResourceCharge,
}

impl CompletionEnvelopeBudgetV1 {
    pub fn for_obligation(
        obligation: LiveCompletionObligation,
    ) -> Result<Self, CompletionBudgetError> {
        static BOUNDS: OnceLock<[RecordBounds; 7]> = OnceLock::new();
        let bounds = match BOUNDS.get() {
            Some(bounds) => bounds,
            None => {
                let measured = measure_record_bounds()?;
                BOUNDS.get_or_init(|| measured)
            }
        };
        let RecordBounds {
            minimum: minimum_encoded_record_bytes,
            maximum: maximum_encoded_record_bytes,
        } = bounds[index(obligation)];
        let records = obligation.record_limit();
        let encoded_bytes = maximum_encoded_record_bytes
            .checked_add(LIVE_EVENT_STORAGE_ALLOWANCE_BYTES)
            .and_then(|bytes| bytes.checked_mul(records))
            .ok_or(CompletionBudgetError::Overflow)?;
        Ok(Self {
            schema: CompletionCreditSchema::V1,
            obligation,
            minimum_encoded_record_bytes,
            maximum_encoded_record_bytes,
            total: LiveResourceCharge {
                records,
                encoded_bytes,
            },
        })
    }

    pub const fn obligation(self) -> LiveCompletionObligation {
        self.obligation
    }

    pub const fn maximum_encoded_record_bytes(self) -> u64 {
        self.maximum_encoded_record_bytes
    }

    pub const fn minimum_encoded_record_bytes(self) -> u64 {
        self.minimum_encoded_record_bytes
    }

    pub const fn total(self) -> LiveResourceCharge {
        self.total
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletionEnvelopeBudgetWire {
    schema: CompletionCreditSchema,
    obligation: LiveCompletionObligation,
    minimum_encoded_record_bytes: u64,
    maximum_encoded_record_bytes: u64,
    total: LiveResourceCharge,
}

impl TryFrom<CompletionEnvelopeBudgetWire> for CompletionEnvelopeBudgetV1 {
    type Error = CompletionBudgetError;

    fn try_from(value: CompletionEnvelopeBudgetWire) -> Result<Self, Self::Error> {
        let expected = Self::for_obligation(value.obligation)?;
        if value.schema != expected.schema
            || value.minimum_encoded_record_bytes != expected.minimum_encoded_record_bytes
            || value.maximum_encoded_record_bytes != expected.maximum_encoded_record_bytes
            || value.total != expected.total
        {
            return Err(CompletionBudgetError::IncompatibleStoredBudget);
        }
        Ok(expected)
    }
}

/// Persisted credit content. The generated owner alone changes `spent` in the
/// ledger transaction; decoding this image issues neither a reservation nor
/// an effect-start permit. Remaining capacity is derived, never a second fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CompletionCreditReservationWire")]
pub struct CompletionCreditReservation {
    budget: CompletionEnvelopeBudgetV1,
    spent: LiveResourceCharge,
}

impl CompletionCreditReservation {
    pub fn from_record(
        budget: CompletionEnvelopeBudgetV1,
        spent: LiveResourceCharge,
    ) -> Result<Self, CompletionBudgetError> {
        let remaining = budget
            .total()
            .checked_sub(spent)
            .map_err(|_| CompletionBudgetError::CreditOverspent)?;
        let minimum_charge = budget
            .minimum_encoded_record_bytes
            .checked_add(LIVE_EVENT_STORAGE_ALLOWANCE_BYTES)
            .ok_or(CompletionBudgetError::Overflow)?;
        let maximum_charge = budget
            .maximum_encoded_record_bytes
            .checked_add(LIVE_EVENT_STORAGE_ALLOWANCE_BYTES)
            .ok_or(CompletionBudgetError::Overflow)?;
        let minimum_spent = minimum_charge
            .checked_mul(spent.records)
            .ok_or(CompletionBudgetError::Overflow)?;
        let maximum_spent = maximum_charge
            .checked_mul(spent.records)
            .ok_or(CompletionBudgetError::Overflow)?;
        let remaining_owed = maximum_charge
            .checked_mul(remaining.records)
            .ok_or(CompletionBudgetError::Overflow)?;
        if !(minimum_spent..=maximum_spent).contains(&spent.encoded_bytes)
            || remaining.encoded_bytes < remaining_owed
        {
            return Err(CompletionBudgetError::CreditOverspent);
        }
        Ok(Self { budget, spent })
    }

    pub const fn budget(self) -> CompletionEnvelopeBudgetV1 {
        self.budget
    }

    pub const fn spent(self) -> LiveResourceCharge {
        self.spent
    }

    pub fn remaining(self) -> LiveResourceCharge {
        // Construction and deserialization enforce componentwise bounds.
        let total = self.budget.total();
        LiveResourceCharge {
            records: total.records - self.spent.records,
            encoded_bytes: total.encoded_bytes - self.spent.encoded_bytes,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletionCreditReservationWire {
    budget: CompletionEnvelopeBudgetV1,
    spent: LiveResourceCharge,
}

impl TryFrom<CompletionCreditReservationWire> for CompletionCreditReservation {
    type Error = CompletionBudgetError;

    fn try_from(value: CompletionCreditReservationWire) -> Result<Self, Self::Error> {
        Self::from_record(value.budget, value.spent)
    }
}

const fn index(obligation: LiveCompletionObligation) -> usize {
    match obligation {
        LiveCompletionObligation::ChannelControl => 0,
        LiveCompletionObligation::RequestChain => 1,
        LiveCompletionObligation::EffectStart => 2,
        LiveCompletionObligation::FunctionOutput => 3,
        LiveCompletionObligation::Continuation => 4,
        LiveCompletionObligation::ContextChunk => 5,
        LiveCompletionObligation::CallbackContinuation => 6,
    }
}

#[derive(Clone, Copy)]
struct RecordBounds {
    minimum: u64,
    maximum: u64,
}

fn measure_record_bounds() -> Result<[RecordBounds; 7], CompletionBudgetError> {
    let mut bounds = [RecordBounds {
        minimum: u64::MAX,
        maximum: 0,
    }; 7];
    for event in maximal_completion_events()? {
        let record = maximal_completion_record(event.clone())?;
        let encoded = record
            .encode()
            .map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)?;
        let slot = &mut bounds[index(encoded.obligation())];
        slot.maximum = slot.maximum.max(encoded.bytes().len() as u64);
        let minimum = minimal_completion_record(event)?.encode()?;
        slot.minimum = slot.minimum.min(minimum.bytes().len() as u64);
    }
    if bounds
        .iter()
        .any(|bounds| bounds.maximum == 0 || bounds.minimum > bounds.maximum)
    {
        return Err(CompletionBudgetError::MissingEnvelopeClass);
    }
    Ok(bounds)
}

pub fn minimal_completion_record(
    mut event: LiveCompletionEvent,
) -> Result<LiveCompletionRecord, CompletionBudgetError> {
    match &mut event {
        LiveCompletionEvent::ChannelControl { diagnostic, .. } => {
            *diagnostic = LiveCompletionText::new("")?;
        }
        LiveCompletionEvent::EffectTerminal {
            diagnostic,
            token_accounting,
            ..
        } => {
            *diagnostic = LiveCompletionText::new("")?;
            if let meerkat_core::execution_scope::ScopedEffectTokenAccounting::Measured {
                normalized_tokens,
                reported_model_digest,
                normalized_counter_digest,
                ..
            } = token_accounting
            {
                *normalized_tokens = 0;
                *reported_model_digest = [0; 32];
                *normalized_counter_digest = [0; 32];
            }
        }
        LiveCompletionEvent::RequestOutcome { outcome, .. } => match outcome {
            LiveRequestCompletionFact::Completed { result, .. } => {
                *result = LiveCompletionText::new("")?;
            }
            LiveRequestCompletionFact::Failed { detail, .. } => {
                *detail = LiveCompletionText::new("")?;
            }
            LiveRequestCompletionFact::OrdinaryTerminal { receipt_digest, .. }
            | LiveRequestCompletionFact::OrdinaryRunlessTerminal { receipt_digest, .. } => {
                *receipt_digest = LiveCompletionText::new("")?;
            }
            LiveRequestCompletionFact::Refused { .. }
            | LiveRequestCompletionFact::CancelledWithoutRun { .. }
            | LiveRequestCompletionFact::Cancelled { .. }
            | LiveRequestCompletionFact::Held { .. }
            | LiveRequestCompletionFact::AdmissionUnconfirmed {} => {}
        },
        LiveCompletionEvent::FunctionOutput { output, .. } => {
            *output = LiveCompletionText::new("")?;
        }
        LiveCompletionEvent::ContextChunk { content, .. } => {
            *content = LiveCompletionText::new("")?;
        }
        LiveCompletionEvent::ChannelUsage { snapshot } => {
            let zero = LiveVoiceDurationSeconds::new(0.0)
                .map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)?;
            match snapshot {
                LiveUsageSnapshot::Periodic { cumulative_seconds }
                | LiveUsageSnapshot::SessionClosed { cumulative_seconds } => {
                    *cumulative_seconds = zero;
                }
                LiveUsageSnapshot::CloseUnconfirmed {
                    last_observed_seconds,
                } => *last_observed_seconds = Some(zero),
                LiveUsageSnapshot::Disputed {
                    last_valid_seconds, ..
                } => *last_valid_seconds = Some(zero),
            }
        }
        LiveCompletionEvent::ChannelProviderStarted { provider_session } => {
            *provider_session = LiveCompletionText::new("a")?;
        }
        LiveCompletionEvent::ChannelProviderDiagnostic { diagnostic } => {
            *diagnostic = meerkat_core::live_execution::backend::LiveProviderDiagnostic::new(
                diagnostic.category(),
                meerkat_core::live_execution::backend::LiveBackendOwnership::Unowned {},
                std::num::NonZeroU64::MIN,
            )
            .map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)?;
        }
        LiveCompletionEvent::ChannelDiscontinuity { discontinuity } => match discontinuity {
            LiveDiscontinuity::KnownLocalGap {
                channel_id,
                observed_bounds,
            } => {
                *channel_id = LiveChannelId::new("a");
                *observed_bounds = KnownLiveReceiveGap::new(0, 1)
                    .map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)?;
            }
            LiveDiscontinuity::UnknownExtentCrashDiscontinuity {
                last_accepted_head,
                old_incarnation,
            } => {
                *old_incarnation = LiveChannelId::new("a");
                last_accepted_head.generation = 0;
                last_accepted_head.revision = 0;
                last_accepted_head.event_count = 0;
                last_accepted_head.prefix_digest = LiveLedgerPrefixDigest::from_sha256([0; 32]);
            }
        },
        LiveCompletionEvent::CallbackSuspended { batch_digest, .. } => {
            *batch_digest = serde_json::from_value(serde_json::json!(vec![0; 32]))
                .map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)?;
        }
        LiveCompletionEvent::FunctionOutputSettlement { .. }
        | LiveCompletionEvent::ContinuationSettlement { .. }
        | LiveCompletionEvent::ContextSettlement { .. }
        | LiveCompletionEvent::CallbackContinuationSettlement { .. } => {}
    }
    Ok(LiveCompletionRecord {
        format: LiveLedgerFormatV1::V1,
        session_id: SessionId(uuid::Uuid::nil()),
        channel_id: LiveChannelId::new("a"),
        sequence: LiveObservationSeq::new(1)
            .map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)?,
        event,
    })
}

/// The codec emits at most six bytes for one decoded UTF-8 byte (U+0000).
/// UUID spellings have fixed width; every numeric sequence uses full u64 width.
pub fn maximal_completion_record(
    event: LiveCompletionEvent,
) -> Result<LiveCompletionRecord, CompletionBudgetError> {
    Ok(LiveCompletionRecord {
        format: LiveLedgerFormatV1::V1,
        session_id: SessionId(uuid::Uuid::nil()),
        channel_id: LiveChannelId::new("\0".repeat(LIVE_COMPLETION_ID_MAX_BYTES)),
        sequence: LiveObservationSeq::new(u64::MAX)
            .map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)?,
        event,
    })
}

/// All bounded event variants, including every plain-enum discriminator. The
/// schema inventory test pins this table to the enum's full variant inventory.
pub fn maximal_completion_events() -> Result<Vec<LiveCompletionEvent>, CompletionBudgetError> {
    let operation = || OperationId(uuid::Uuid::nil());
    let input = || InputId::from_uuid(uuid::Uuid::nil());
    let run = || RunId::from_uuid(uuid::Uuid::nil());
    let result =
        LiveCompletionText::<LIVE_RESULT_MAX_BYTES>::new("\0".repeat(LIVE_RESULT_MAX_BYTES))?;
    let diagnostic = LiveCompletionText::<LIVE_TERMINAL_DIAGNOSTIC_MAX_BYTES>::new(
        "\0".repeat(LIVE_TERMINAL_DIAGNOSTIC_MAX_BYTES),
    )?;
    let chunk = LiveCompletionText::<LIVE_CONTEXT_CHUNK_MAX_BYTES>::new(
        "\0".repeat(LIVE_CONTEXT_CHUNK_MAX_BYTES),
    )?;
    let mut events = Vec::new();
    events.push(LiveCompletionEvent::ChannelProviderStarted {
        provider_session: LiveCompletionText::new("\0".repeat(LIVE_COMPLETION_ID_MAX_BYTES))?,
    });
    // A validated diagnostic is at most 1024 encoded bytes, below the existing
    // escaped 1024-byte ChannelControl text. That variant bounds this class.
    use meerkat_core::live_execution::backend::{
        LiveBackendCandidates, LiveBackendOwnership, LiveBackendResponseKey,
        LiveProviderDiagnostic, LiveProviderDiagnosticCategory,
    };
    use meerkat_core::live_execution::request::LiveProviderReference;
    for category in [
        LiveProviderDiagnosticCategory::BackendAdvisoryError,
        LiveProviderDiagnosticCategory::ProtocolInconsistency,
        LiveProviderDiagnosticCategory::AccountingUnmeasured,
        LiveProviderDiagnosticCategory::AccountingDisputed,
        LiveProviderDiagnosticCategory::UnsupportedProviderEvent,
        LiveProviderDiagnosticCategory::UncorrelatedContextAcknowledgment,
    ] {
        events.push(LiveCompletionEvent::ChannelProviderDiagnostic {
            diagnostic: LiveProviderDiagnostic::new(
                category,
                LiveBackendOwnership::Unowned {},
                std::num::NonZeroU64::MAX,
            )
            .map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)?,
        });
    }
    let reference = |value: String| {
        LiveProviderReference::new(value).map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)
    };
    let response = reference("\0".repeat(128))?;
    let category = LiveProviderDiagnosticCategory::UncorrelatedContextAcknowledgment;
    let sized = LiveProviderDiagnostic::new(
        category,
        LiveBackendOwnership::Owned {
            response: LiveBackendResponseKey {
                response: response.clone(),
                delegation: Some(reference("a".into())?),
            },
        },
        std::num::NonZeroU64::MAX,
    )
    .map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)?;
    let size = serde_json::to_vec(&sized)
        .map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)?
        .len();
    let remaining = meerkat_core::live_execution::backend::LIVE_PUBLIC_DIAGNOSTIC_MAX_BYTES
        .checked_sub(size)
        .ok_or(CompletionBudgetError::MaximumEnvelopeInvalid)?;
    for attribution in [
        LiveBackendOwnership::Owned {
            response: LiveBackendResponseKey {
                response: response.clone(),
                delegation: None,
            },
        },
        LiveBackendOwnership::Owned {
            response: LiveBackendResponseKey {
                response,
                delegation: Some(reference("a".repeat(remaining + 1))?),
            },
        },
        LiveBackendOwnership::Ambiguous {
            candidates: LiveBackendCandidates::new(vec![
                LiveBackendResponseKey {
                    response: reference("a".into())?,
                    delegation: None,
                },
                LiveBackendResponseKey {
                    response: reference("b".into())?,
                    delegation: Some(reference("d".into())?),
                },
            ])
            .map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)?,
        },
    ] {
        events.push(LiveCompletionEvent::ChannelProviderDiagnostic {
            diagnostic: LiveProviderDiagnostic::new(
                category,
                attribution,
                std::num::NonZeroU64::MAX,
            )
            .map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)?,
        });
    }
    for outcome in LiveChannelControlOutcome::ALL {
        events.push(LiveCompletionEvent::ChannelControl {
            outcome: *outcome,
            diagnostic: diagnostic.clone(),
        });
    }
    for duration in [f64::MAX, f64::MIN_POSITIVE] {
        let duration = LiveVoiceDurationSeconds::new(duration)
            .map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)?;
        for snapshot in [
            LiveUsageSnapshot::Periodic {
                cumulative_seconds: duration,
            },
            LiveUsageSnapshot::SessionClosed {
                cumulative_seconds: duration,
            },
            LiveUsageSnapshot::CloseUnconfirmed {
                last_observed_seconds: Some(duration),
            },
            LiveUsageSnapshot::CloseUnconfirmed {
                last_observed_seconds: None,
            },
            LiveUsageSnapshot::Disputed {
                last_valid_seconds: Some(duration),
                reason: LiveUsageDispute::Regression,
            },
            LiveUsageSnapshot::Disputed {
                last_valid_seconds: Some(duration),
                reason: LiveUsageDispute::InvalidDuration,
            },
            LiveUsageSnapshot::Disputed {
                last_valid_seconds: Some(duration),
                reason: LiveUsageDispute::ConflictingFinal,
            },
            LiveUsageSnapshot::Disputed {
                last_valid_seconds: Some(duration),
                reason: LiveUsageDispute::MissingFinal,
            },
            LiveUsageSnapshot::Disputed {
                last_valid_seconds: None,
                reason: LiveUsageDispute::MissingFinal,
            },
        ] {
            events.push(LiveCompletionEvent::ChannelUsage { snapshot });
        }
    }
    for discontinuity in [
        LiveDiscontinuity::KnownLocalGap {
            channel_id: LiveChannelId::new("\0".repeat(LIVE_COMPLETION_ID_MAX_BYTES)),
            observed_bounds: KnownLiveReceiveGap::new(u64::MAX - 1, u64::MAX)
                .map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)?,
        },
        LiveDiscontinuity::UnknownExtentCrashDiscontinuity {
            last_accepted_head: LiveHeadReference {
                format: LiveLedgerFormatV1::V1,
                session_id: SessionId(uuid::Uuid::nil()),
                generation: u64::MAX,
                revision: u64::MAX,
                event_count: u64::MAX,
                prefix_digest: LiveLedgerPrefixDigest::from_sha256([255; 32]),
            },
            old_incarnation: LiveChannelId::new("\0".repeat(LIVE_COMPLETION_ID_MAX_BYTES)),
        },
    ] {
        events.push(LiveCompletionEvent::ChannelDiscontinuity { discontinuity });
    }
    let mut terminal = vec![
        LiveRequestCompletionFact::OrdinaryRunlessTerminal {
            input_id: input(),
            receipt_digest: LiveCompletionText::new("\0".repeat(64))?,
        },
        LiveRequestCompletionFact::OrdinaryTerminal {
            input_id: input(),
            run_id: run(),
            receipt_digest: LiveCompletionText::new("\0".repeat(64))?,
        },
        LiveRequestCompletionFact::Completed {
            input_id: input(),
            run_id: run(),
            result: result.clone(),
        },
        LiveRequestCompletionFact::Failed {
            input_id: input(),
            run_id: run(),
            detail: diagnostic.clone(),
        },
        LiveRequestCompletionFact::AdmissionUnconfirmed {},
    ];
    for reason in [
        LiveRequestCancellationReason::OperatorRequested,
        LiveRequestCancellationReason::GrantRevoked,
        LiveRequestCancellationReason::SessionArchived,
        LiveRequestCancellationReason::ExplicitSupersession,
    ] {
        terminal.push(LiveRequestCompletionFact::CancelledWithoutRun { reason });
        terminal.push(LiveRequestCompletionFact::Cancelled {
            input_id: input(),
            run_id: run(),
            reason,
        });
    }
    terminal.extend(
        LiveRequestRefusal::ALL
            .iter()
            .map(|reason| LiveRequestCompletionFact::Refused { reason: *reason }),
    );
    terminal.extend(
        LiveRequestHold::ALL
            .iter()
            .map(|reason| LiveRequestCompletionFact::Held {
                input_id: input(),
                reason: *reason,
            }),
    );
    events.extend(
        terminal
            .into_iter()
            .map(|outcome| LiveCompletionEvent::RequestOutcome {
                request_id: operation(),
                outcome,
            }),
    );
    for outcome in LivePhysicalEffectOutcome::ALL {
        for token_accounting in token_accounting_encoding_extrema() {
            events.push(LiveCompletionEvent::EffectTerminal {
                claim_id: operation(),
                request_id: operation(),
                outcome: *outcome,
                token_accounting,
                diagnostic: diagnostic.clone(),
            });
        }
    }
    events.push(LiveCompletionEvent::CallbackSuspended {
        claim_id: operation(),
        request_id: operation(),
        input_id: input(),
        run_id: run(),
        batch_digest: serde_json::from_value(serde_json::json!(vec![255; 32]))
            .map_err(|_| CompletionBudgetError::MaximumEnvelopeInvalid)?,
    });
    events.push(LiveCompletionEvent::FunctionOutput {
        attempt_id: operation(),
        request_id: operation(),
        output: result,
    });
    events.extend(LiveResultDeliveryState::ALL.iter().map(|state| {
        LiveCompletionEvent::FunctionOutputSettlement {
            attempt_id: operation(),
            state: *state,
        }
    }));
    events.extend(LiveContinuationState::ALL.iter().map(|state| {
        LiveCompletionEvent::ContinuationSettlement {
            attempt_id: operation(),
            state: *state,
        }
    }));
    events.push(LiveCompletionEvent::ContextChunk {
        attempt_id: operation(),
        content: chunk,
    });
    events.extend(LiveContextChunkDeliveryState::ALL.iter().map(|state| {
        LiveCompletionEvent::ContextSettlement {
            attempt_id: operation(),
            state: *state,
        }
    }));
    for outcome in [
        LiveCallbackContinuationOutcome::Admitted { input_id: input() },
        LiveCallbackContinuationOutcome::CancelledWithoutInput {},
        LiveCallbackContinuationOutcome::ResourceCapacityRefused {},
        LiveCallbackContinuationOutcome::ScopeRestorationFailed {},
        LiveCallbackContinuationOutcome::AdmissionUnconfirmed {},
    ] {
        events.push(LiveCompletionEvent::CallbackContinuationSettlement {
            request_id: operation(),
            suspended_run_id: run(),
            outcome,
        });
    }
    Ok(events)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CompletionBudgetError {
    #[error("a maximum completion envelope violates its encoding contract")]
    MaximumEnvelopeInvalid,
    #[error("a completion obligation has no maximum envelope")]
    MissingEnvelopeClass,
    #[error("completion reservation arithmetic overflowed")]
    Overflow,
    #[error("persisted completion budget does not match its versioned schema")]
    IncompatibleStoredBudget,
    #[error(
        "persisted completion spending violates per-record charge bounds or remaining owed capacity"
    )]
    CreditOverspent,
}

impl From<LiveCompletionEncodingError> for CompletionBudgetError {
    fn from(_: LiveCompletionEncodingError) -> Self {
        Self::MaximumEnvelopeInvalid
    }
}
