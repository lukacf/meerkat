//! Closed continuation/attempt read images. Validation checks content joins;
//! only generated authority and an atomic store claim may authorize a send.

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;

use meerkat_core::SessionId;
use meerkat_core::execution_scope::ExecutionAdmissionCommitRef;
use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_execution::request::LiveProviderReference;
use meerkat_core::ops::OperationId;
use serde::{Deserialize, Serialize};

use crate::live_delivery::{
    LiveContinuationBatchEligibility, LiveContinuationState, LiveResultDeliveryState,
};
use crate::live_ledger::transcript::LiveLedgerFormatV1;

pub const LIVE_CONTINUATION_SELECTION_MAX_BATCHES: usize = 32;
pub const LIVE_BATCH_MAX_OUTPUTS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "LiveFunctionBatchKeyParts")]
pub struct LiveFunctionBatchKey {
    channel_id: LiveChannelId,
    delegation: LiveProviderReference,
    response: LiveProviderReference,
}

impl LiveFunctionBatchKey {
    pub fn new(
        channel_id: LiveChannelId,
        delegation: LiveProviderReference,
        response: LiveProviderReference,
    ) -> Result<Self, LiveAttemptImageError> {
        if [channel_id.as_str(), delegation.as_str(), response.as_str()]
            .into_iter()
            .any(|value| value.is_empty() || value.len() > 128)
        {
            return Err(LiveAttemptImageError::InvalidIdentity);
        }
        Ok(Self {
            channel_id,
            delegation,
            response,
        })
    }

    pub fn channel_id(&self) -> &LiveChannelId {
        &self.channel_id
    }
    pub fn delegation(&self) -> &LiveProviderReference {
        &self.delegation
    }
    pub fn response(&self) -> &LiveProviderReference {
        &self.response
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveFunctionBatchKeyParts {
    channel_id: LiveChannelId,
    delegation: LiveProviderReference,
    response: LiveProviderReference,
}

impl TryFrom<LiveFunctionBatchKeyParts> for LiveFunctionBatchKey {
    type Error = LiveAttemptImageError;
    fn try_from(value: LiveFunctionBatchKeyParts) -> Result<Self, Self::Error> {
        Self::new(value.channel_id, value.delegation, value.response)
    }
}

/// References an already stored payload. Snapshots never duplicate its body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LivePayloadRecordRef {
    pub sequence: NonZeroU64,
    pub digest: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveFunctionOutputRecord {
    pub call_id: LiveProviderReference,
    pub attempt_id: OperationId,
    pub payload: LivePayloadRecordRef,
    pub physical_write: LiveResultDeliveryState,
    /// A later exact rejection supplements, never overwrites, physical write.
    pub later_rejection: Option<LivePayloadRecordRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveContinuationBatchRecord {
    pub key: LiveFunctionBatchKey,
    pub outputs: Vec<LiveFunctionOutputRecord>,
    pub eligibility: LiveContinuationBatchEligibility,
    pub claiming_attempt: Option<OperationId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveContinuationClaimRecord {
    pub attempt_id: OperationId,
    pub send_generation: NonZeroU64,
    pub members: Vec<LiveFunctionBatchKey>,
    pub claim_commit: ExecutionAdmissionCommitRef,
    pub delivery: LiveContinuationState,
}

/// This refusal has no fabricated continuation attempt or provider write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveContinuationNotAttemptedRecord {
    pub members: Vec<LiveFunctionBatchKey>,
    pub missing_required_outputs: Vec<OperationId>,
    pub fenced_send_generation: NonZeroU64,
}

/// A bounded captured join of selected batches and their claims. It is not a
/// lifetime in-memory tombstone registry or a substitute for keyed store reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LiveContinuationSnapshotParts")]
pub struct LiveContinuationSnapshot {
    format: LiveLedgerFormatV1,
    session_id: SessionId,
    channel_id: LiveChannelId,
    send_generation: NonZeroU64,
    batches: Vec<LiveContinuationBatchRecord>,
    claims: Vec<LiveContinuationClaimRecord>,
    not_attempted: Vec<LiveContinuationNotAttemptedRecord>,
}

impl LiveContinuationSnapshot {
    pub fn new(parts: LiveContinuationSnapshotParts) -> Result<Self, LiveAttemptImageError> {
        if parts.channel_id.as_str().is_empty()
            || parts.channel_id.as_str().len() > 128
            || parts.batches.len() > LIVE_CONTINUATION_SELECTION_MAX_BATCHES
            || parts.claims.len() > LIVE_CONTINUATION_SELECTION_MAX_BATCHES
            || parts.not_attempted.len() > LIVE_CONTINUATION_SELECTION_MAX_BATCHES
        {
            return Err(LiveAttemptImageError::SelectionBound);
        }
        let mut batches = HashMap::new();
        let mut output_attempts = HashSet::new();
        for batch in &parts.batches {
            if batch.key.channel_id() != &parts.channel_id
                || batches.insert(&batch.key, batch).is_some()
                || batch.outputs.len() > LIVE_BATCH_MAX_OUTPUTS
            {
                return Err(LiveAttemptImageError::InvalidBatch);
            }
            let mut calls = HashSet::new();
            for output in &batch.outputs {
                if output.call_id.as_str().len() > 128
                    || !calls.insert(&output.call_id)
                    || !output_attempts.insert(&output.attempt_id)
                    || (output.later_rejection.is_some()
                        && output.physical_write
                            != LiveResultDeliveryState::WrittenConsumptionUnconfirmed)
                {
                    return Err(LiveAttemptImageError::InvalidOutputSet);
                }
            }
            let all_written = !batch.outputs.is_empty()
                && batch.outputs.iter().all(|output| {
                    output.physical_write == LiveResultDeliveryState::WrittenConsumptionUnconfirmed
                });
            match batch.eligibility {
                LiveContinuationBatchEligibility::EligibleUnclaimed => {
                    if !all_written
                        || batch.claiming_attempt.is_some()
                        || batch
                            .outputs
                            .iter()
                            .any(|output| output.later_rejection.is_some())
                    {
                        return Err(LiveAttemptImageError::UnwrittenRequiredOutput);
                    }
                }
                LiveContinuationBatchEligibility::ClaimedByAttempt
                | LiveContinuationBatchEligibility::Spent => {
                    if !all_written || batch.claiming_attempt.is_none() {
                        return Err(LiveAttemptImageError::MissingClaim);
                    }
                }
                LiveContinuationBatchEligibility::PendingOutputs
                | LiveContinuationBatchEligibility::Abandoned => {
                    if batch.claiming_attempt.is_some() {
                        return Err(LiveAttemptImageError::UnexpectedClaim);
                    }
                }
            }
        }
        let mut attempts = HashSet::new();
        let mut claimed_members = HashMap::new();
        let mut active = 0;
        for claim in &parts.claims {
            if !attempts.insert(&claim.attempt_id)
                || output_attempts.contains(&claim.attempt_id)
                || claim.send_generation > parts.send_generation
                || claim.members.is_empty()
                || claim.members.len() > LIVE_CONTINUATION_SELECTION_MAX_BATCHES
            {
                return Err(LiveAttemptImageError::InvalidClaim);
            }
            let active_claim = match claim.delivery {
                LiveContinuationState::Claimed
                | LiveContinuationState::NotEnqueued
                | LiveContinuationState::AmbiguousUnfenced => true,
                LiveContinuationState::WrittenConsumptionUnconfirmed
                | LiveContinuationState::AmbiguousFenced
                | LiveContinuationState::AbandonedByClose => false,
                LiveContinuationState::NotClaimed
                | LiveContinuationState::NotAttemptedMissingRequiredOutputs => {
                    return Err(LiveAttemptImageError::InvalidClaim);
                }
            };
            if active_claim {
                active += 1;
                if active > 1 || claim.send_generation != parts.send_generation {
                    return Err(LiveAttemptImageError::OverlappingActiveAttempt);
                }
            }
            for member in &claim.members {
                let batch = batches
                    .get(member)
                    .ok_or(LiveAttemptImageError::MissingBatch)?;
                if claimed_members.insert(member, &claim.attempt_id).is_some() {
                    return Err(LiveAttemptImageError::OverlappingMembership);
                }
                if batch.claiming_attempt.as_ref() != Some(&claim.attempt_id)
                    || (active_claim
                        && batch.eligibility != LiveContinuationBatchEligibility::ClaimedByAttempt)
                    || (!active_claim
                        && batch.eligibility != LiveContinuationBatchEligibility::Spent)
                {
                    return Err(LiveAttemptImageError::ClaimOwnerMismatch);
                }
            }
        }
        for batch in &parts.batches {
            if batch.claiming_attempt.as_ref() != claimed_members.get(&batch.key).copied() {
                return Err(LiveAttemptImageError::MissingClaim);
            }
        }
        let mut abandoned_members = HashSet::new();
        for refusal in &parts.not_attempted {
            if refusal.members.is_empty()
                || refusal.members.len() > LIVE_CONTINUATION_SELECTION_MAX_BATCHES
                || refusal.fenced_send_generation > parts.send_generation
            {
                return Err(LiveAttemptImageError::InvalidAbandonment);
            }
            let mut missing = HashSet::new();
            for member in &refusal.members {
                let batch = batches
                    .get(member)
                    .ok_or(LiveAttemptImageError::MissingBatch)?;
                if !abandoned_members.insert(member)
                    || batch.eligibility != LiveContinuationBatchEligibility::Abandoned
                {
                    return Err(LiveAttemptImageError::InvalidAbandonment);
                }
                for output in &batch.outputs {
                    if output.physical_write
                        != LiveResultDeliveryState::WrittenConsumptionUnconfirmed
                        || output.later_rejection.is_some()
                    {
                        missing.insert(&output.attempt_id);
                    }
                }
            }
            let declared: HashSet<_> = refusal.missing_required_outputs.iter().collect();
            if missing.is_empty()
                || declared.len() != refusal.missing_required_outputs.len()
                || declared != missing
            {
                return Err(LiveAttemptImageError::MissingOutputMismatch);
            }
        }
        let snapshot = Self {
            format: parts.format,
            session_id: parts.session_id,
            channel_id: parts.channel_id,
            send_generation: parts.send_generation,
            batches: parts.batches,
            claims: parts.claims,
            not_attempted: parts.not_attempted,
        };
        meerkat_contracts::wire::live_observation::LiveObservationWireCodecV1::encode_ledger_record(&snapshot)
            .map_err(|error| match error {
                meerkat_contracts::wire::live_observation::LiveObservationEncodingError::EncodedReplyTooLarge => LiveAttemptImageError::SelectionBound,
                other => LiveAttemptImageError::Encoding(other.to_string()),
            })?;
        Ok(snapshot)
    }

    pub fn batches(&self) -> &[LiveContinuationBatchRecord] {
        &self.batches
    }
    pub fn claims(&self) -> &[LiveContinuationClaimRecord] {
        &self.claims
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveContinuationSnapshotParts {
    pub format: LiveLedgerFormatV1,
    pub session_id: SessionId,
    pub channel_id: LiveChannelId,
    pub send_generation: NonZeroU64,
    pub batches: Vec<LiveContinuationBatchRecord>,
    pub claims: Vec<LiveContinuationClaimRecord>,
    pub not_attempted: Vec<LiveContinuationNotAttemptedRecord>,
}

impl TryFrom<LiveContinuationSnapshotParts> for LiveContinuationSnapshot {
    type Error = LiveAttemptImageError;
    fn try_from(parts: LiveContinuationSnapshotParts) -> Result<Self, Self::Error> {
        Self::new(parts)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LiveAttemptImageError {
    #[error("cannot encode Live continuation selection: {0}")]
    Encoding(String),
    #[error("invalid bounded Live batch identity")]
    InvalidIdentity,
    #[error("Live continuation selection exceeds its bounded window")]
    SelectionBound,
    #[error("duplicate, foreign-channel or oversized Live batch")]
    InvalidBatch,
    #[error("invalid, repeated or contradictory required output")]
    InvalidOutputSet,
    #[error("continuation eligibility requires every output to be written and unrejected")]
    UnwrittenRequiredOutput,
    #[error("claimed or spent batch is missing its exact claim")]
    MissingClaim,
    #[error("unclaimed batch cannot contain a claim owner")]
    UnexpectedClaim,
    #[error("invalid empty, repeated, future or unclaimed continuation attempt")]
    InvalidClaim,
    #[error("a channel has more than one active continuation or a stale active generation")]
    OverlappingActiveAttempt,
    #[error("continuation record refers to a batch outside its captured join")]
    MissingBatch,
    #[error("a batch cannot be claimed by another subset, superset or overlapping set")]
    OverlappingMembership,
    #[error("batch claim owner or delivery class differs from its exact attempt")]
    ClaimOwnerMismatch,
    #[error("invalid or repeated aggregate continuation abandonment")]
    InvalidAbandonment,
    #[error("missing-output declaration must match the exact nonempty required-output set")]
    MissingOutputMismatch,
}
