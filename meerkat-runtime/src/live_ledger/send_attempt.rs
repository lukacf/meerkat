//! Persisted physical-send facts. Decoding these records neither retries a
//! write nor restores a provider-send permit.

use std::collections::HashSet;
use std::num::NonZeroU64;

use meerkat_core::SessionId;
use meerkat_core::execution_scope::ExecutionAdmissionCommitRef;
use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_execution::request::LiveProviderReference;
use meerkat_core::ops::OperationId;
use serde::{Deserialize, Serialize};

use super::attempt::{
    LIVE_CONTINUATION_SELECTION_MAX_BATCHES, LiveFunctionBatchKey, LivePayloadRecordRef,
};
use super::transcript::LiveLedgerFormatV1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveSendAttemptTarget {
    FunctionOutput {
        batch: LiveFunctionBatchKey,
        call_id: LiveProviderReference,
    },
    Continuation {
        members: Vec<LiveFunctionBatchKey>,
    },
    ContextChunk {
        plan_id: OperationId,
        chunk_index: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveSendAbandonment {
    ChannelClosed,
    ChannelReplaced,
    RequestCancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveNoWriteEvidence {
    BeforeClaim {},
    NotEnqueued {
        claim: ExecutionAdmissionCommitRef,
        feedback: LivePayloadRecordRef,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveSendGenerationFenceRecord {
    pub fenced_generation: NonZeroU64,
    pub successor_generation: NonZeroU64,
    pub commit: ExecutionAdmissionCommitRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveSendAttemptDisposition {
    Authorized {},
    Claimed {
        claim: ExecutionAdmissionCommitRef,
    },
    NotEnqueued {
        claim: ExecutionAdmissionCommitRef,
        feedback: LivePayloadRecordRef,
    },
    WrittenConsumptionUnconfirmed {
        claim: ExecutionAdmissionCommitRef,
        feedback: LivePayloadRecordRef,
    },
    AmbiguousUnfenced {
        claim: ExecutionAdmissionCommitRef,
    },
    AmbiguousFenced {
        claim: ExecutionAdmissionCommitRef,
        fence: LiveSendGenerationFenceRecord,
    },
    NotSent {
        claim: ExecutionAdmissionCommitRef,
        no_write_feedback: LivePayloadRecordRef,
    },
    RejectedBeforeWrite {
        no_write: LiveNoWriteEvidence,
        rejection: LivePayloadRecordRef,
    },
    AbandonedBeforeClaim {
        reason: LiveSendAbandonment,
    },
    AbandonedNotEnqueued {
        claim: ExecutionAdmissionCommitRef,
        no_write_feedback: LivePayloadRecordRef,
        reason: LiveSendAbandonment,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LiveSendAttemptRecordParts")]
pub struct LiveSendAttemptRecord {
    format: LiveLedgerFormatV1,
    session_id: SessionId,
    channel_id: LiveChannelId,
    attempt_id: OperationId,
    send_generation: NonZeroU64,
    target: LiveSendAttemptTarget,
    payload: LivePayloadRecordRef,
    disposition: LiveSendAttemptDisposition,
    later_rejection: Option<LivePayloadRecordRef>,
}

impl LiveSendAttemptRecord {
    pub fn new(parts: LiveSendAttemptRecordParts) -> Result<Self, LiveSendAttemptRecordError> {
        if parts.channel_id.as_str().is_empty() || parts.channel_id.as_str().len() > 128 {
            return Err(LiveSendAttemptRecordError::InvalidTarget);
        }
        match &parts.target {
            LiveSendAttemptTarget::FunctionOutput { batch, call_id } => {
                if batch.channel_id() != &parts.channel_id || call_id.as_str().len() > 128 {
                    return Err(LiveSendAttemptRecordError::InvalidTarget);
                }
            }
            LiveSendAttemptTarget::Continuation { members } => {
                let distinct: HashSet<_> = members.iter().collect();
                if members.is_empty()
                    || members.len() > LIVE_CONTINUATION_SELECTION_MAX_BATCHES
                    || distinct.len() != members.len()
                    || members
                        .iter()
                        .any(|batch| batch.channel_id() != &parts.channel_id)
                {
                    return Err(LiveSendAttemptRecordError::InvalidTarget);
                }
            }
            LiveSendAttemptTarget::ContextChunk { chunk_index, .. } => {
                if *chunk_index >= 64 {
                    return Err(LiveSendAttemptRecordError::InvalidTarget);
                }
            }
        }
        let feedback = match &parts.disposition {
            LiveSendAttemptDisposition::NotEnqueued { feedback, .. }
            | LiveSendAttemptDisposition::WrittenConsumptionUnconfirmed { feedback, .. } => {
                Some(feedback)
            }
            LiveSendAttemptDisposition::NotSent {
                no_write_feedback, ..
            }
            | LiveSendAttemptDisposition::AbandonedNotEnqueued {
                no_write_feedback, ..
            } => Some(no_write_feedback),
            LiveSendAttemptDisposition::RejectedBeforeWrite {
                no_write: LiveNoWriteEvidence::NotEnqueued { feedback, .. },
                ..
            } => Some(feedback),
            _ => None,
        };
        if feedback.is_some_and(|feedback| feedback.sequence <= parts.payload.sequence) {
            return Err(LiveSendAttemptRecordError::FeedbackBeforePayload);
        }
        if let LiveSendAttemptDisposition::RejectedBeforeWrite { rejection, .. } =
            &parts.disposition
            && rejection.sequence <= parts.payload.sequence
        {
            return Err(LiveSendAttemptRecordError::FeedbackBeforePayload);
        }
        if let LiveSendAttemptDisposition::AmbiguousFenced { claim, fence } = &parts.disposition
            && (fence.fenced_generation != parts.send_generation
                || fence.successor_generation <= fence.fenced_generation
                || fence.commit.revision <= claim.revision)
        {
            return Err(LiveSendAttemptRecordError::InvalidFence);
        }
        if let Some(rejection) = &parts.later_rejection {
            let LiveSendAttemptDisposition::WrittenConsumptionUnconfirmed { feedback, .. } =
                &parts.disposition
            else {
                return Err(LiveSendAttemptRecordError::RejectionWithoutWrite);
            };
            // The receiver can persist an exact rejection before the send
            // worker's write feedback arrives. Both must follow the payload;
            // neither arrival order changes the physical write fact.
            if rejection.sequence <= parts.payload.sequence
                || rejection.sequence == feedback.sequence
            {
                return Err(LiveSendAttemptRecordError::RejectionWithoutWrite);
            }
        }
        let record = Self {
            format: parts.format,
            session_id: parts.session_id,
            channel_id: parts.channel_id,
            attempt_id: parts.attempt_id,
            send_generation: parts.send_generation,
            target: parts.target,
            payload: parts.payload,
            disposition: parts.disposition,
            later_rejection: parts.later_rejection,
        };
        meerkat_contracts::wire::live_observation::LiveObservationWireCodecV1::encode_ledger_record(
            &record,
        )?;
        Ok(record)
    }

    pub fn disposition(&self) -> &LiveSendAttemptDisposition {
        &self.disposition
    }

    pub fn attempt_id(&self) -> &OperationId {
        &self.attempt_id
    }

    pub fn payload(&self) -> &LivePayloadRecordRef {
        &self.payload
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveSendAttemptRecordParts {
    pub format: LiveLedgerFormatV1,
    pub session_id: SessionId,
    pub channel_id: LiveChannelId,
    pub attempt_id: OperationId,
    pub send_generation: NonZeroU64,
    pub target: LiveSendAttemptTarget,
    pub payload: LivePayloadRecordRef,
    pub disposition: LiveSendAttemptDisposition,
    pub later_rejection: Option<LivePayloadRecordRef>,
}

impl TryFrom<LiveSendAttemptRecordParts> for LiveSendAttemptRecord {
    type Error = LiveSendAttemptRecordError;

    fn try_from(parts: LiveSendAttemptRecordParts) -> Result<Self, Self::Error> {
        Self::new(parts)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LiveSendAttemptRecordError {
    #[error("Live send attempt has an invalid or foreign target")]
    InvalidTarget,
    #[error("Live physical-write feedback must follow its immutable payload")]
    FeedbackBeforePayload,
    #[error("Live send fence must name the claimed generation and a later committed successor")]
    InvalidFence,
    #[error("a later provider rejection must supplement, not replace, a written fact")]
    RejectionWithoutWrite,
    #[error(transparent)]
    Encoding(#[from] meerkat_contracts::wire::live_observation::LiveObservationEncodingError),
}
