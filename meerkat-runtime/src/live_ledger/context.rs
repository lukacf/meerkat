//! Immutable context plans and captured delivery facts. Neither decoding a
//! plan nor computing its frontiers issues disclosure or provider-send authority.

use std::collections::HashSet;
use std::num::NonZeroU16;

use meerkat_core::SessionId;
use meerkat_core::execution_scope::ExecutionAdmissionCommitRef;
use meerkat_core::live_execution::activation::LiveProfileRevision;
use meerkat_core::live_execution::profile::{LiveContextProjectionPolicy, LiveProfileId};
use meerkat_core::live_execution::request::{LiveProviderReference, LiveSourceKey};
use meerkat_core::live_execution::{LiveChannelId, LiveContextIntent};
use meerkat_core::live_observation::LiveTranscriptRange;
use meerkat_core::ops::OperationId;
use serde::{Deserialize, Serialize};

use super::attempt::LivePayloadRecordRef;
use crate::live_delivery::LiveContextChunkDeliveryState;
use crate::live_ledger::transcript::{LiveHeadReference, LiveLedgerFormatV1};
use crate::live_source::LiveActorContextReference;

/// Positions in one immutable source selection, not provider turn IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LiveContextSourceWindowParts")]
pub struct LiveContextSourceWindow {
    after: u64,
    through: u64,
}

impl LiveContextSourceWindow {
    pub fn new(after: u64, through: u64) -> Result<Self, LiveContextPlanError> {
        if through < after {
            return Err(LiveContextPlanError::InvalidSourceWindow);
        }
        Ok(Self { after, through })
    }
    pub const fn after(self) -> u64 {
        self.after
    }
    pub const fn through(self) -> u64 {
        self.through
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveContextSourceWindowParts {
    after: u64,
    through: u64,
}

impl TryFrom<LiveContextSourceWindowParts> for LiveContextSourceWindow {
    type Error = LiveContextPlanError;
    fn try_from(value: LiveContextSourceWindowParts) -> Result<Self, Self::Error> {
        Self::new(value.after, value.through)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveContextPlanOrigin {
    VoiceProfile {
        profile_id: LiveProfileId,
        revision: LiveProfileRevision,
    },
    ActorHistory {
        actor: LiveActorContextReference,
        canonical_message_count: u64,
    },
    ContinuousObservations {
        head: LiveHeadReference,
    },
    TaskResult {
        source: LiveSourceKey,
        payload: LivePayloadRecordRef,
    },
    ApplicationContext {
        source: LiveSourceKey,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveContextOmissionReason {
    ExcludedByPolicy,
    NonTextual,
    OlderThanWindow,
    OverBudget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveContextOmission {
    pub source: LiveContextSourceWindow,
    pub reason: LiveContextOmissionReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveContextChunkAttempt {
    pub attempt_id: OperationId,
    pub event_id: Option<LiveProviderReference>,
    pub claim_commit: ExecutionAdmissionCommitRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveContextCorrelatedInjection {
    pub event_id: LiveProviderReference,
    pub estimated_range: LiveTranscriptRange,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveContextChunkRecord {
    pub source_ordinal: u64,
    pub source_chunk_index: u16,
    pub source_chunk_count: NonZeroU16,
    pub payload: LivePayloadRecordRef,
    pub content_digest: [u8; 32],
    pub decoded_utf8_bytes: u16,
    pub attempt: Option<LiveContextChunkAttempt>,
    pub write: LiveContextChunkDeliveryState,
    pub correlated_injection: Option<LiveContextCorrelatedInjection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveContextPlanFrontiers {
    pub considered_source_through: u64,
    pub attempted_chunk_count: usize,
    pub correlated_injection_chunk_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "LiveContextPlanParts")]
pub struct LiveContextPlanRecord {
    format: LiveLedgerFormatV1,
    plan_id: OperationId,
    session_id: SessionId,
    channel_id: LiveChannelId,
    origin: LiveContextPlanOrigin,
    intent: LiveContextIntent,
    policy: LiveContextProjectionPolicy,
    considered: LiveContextSourceWindow,
    omissions: Vec<LiveContextOmission>,
    chunks: Vec<LiveContextChunkRecord>,
    uncorrelated_injections: Vec<LiveTranscriptRange>,
}

impl LiveContextPlanRecord {
    pub fn new(parts: LiveContextPlanParts) -> Result<Self, LiveContextPlanError> {
        if parts.channel_id.as_str().is_empty()
            || parts.channel_id.as_str().len() > 128
            || parts.chunks.len() > 64
            || parts.omissions.len() > 64
            || parts.uncorrelated_injections.len() > 64
        {
            return Err(LiveContextPlanError::PlanBound);
        }
        if parts.intent == LiveContextIntent::Instructions
            && !matches!(parts.origin, LiveContextPlanOrigin::VoiceProfile { .. })
        {
            return Err(LiveContextPlanError::InstructionsProvenance);
        }
        let source_end = match &parts.origin {
            LiveContextPlanOrigin::ActorHistory {
                actor,
                canonical_message_count,
            } => {
                if actor.session_id != parts.session_id
                    || actor.token.is_empty()
                    || actor.token.len() > 256
                {
                    return Err(LiveContextPlanError::SourceOwnerMismatch);
                }
                *canonical_message_count
            }
            LiveContextPlanOrigin::ContinuousObservations { head } => {
                if head.session_id != parts.session_id || head.generation == 0 || head.revision == 0
                {
                    return Err(LiveContextPlanError::SourceOwnerMismatch);
                }
                head.event_count
            }
            LiveContextPlanOrigin::TaskResult { source, .. }
            | LiveContextPlanOrigin::ApplicationContext { source } => {
                if source.session_id() != &parts.session_id {
                    return Err(LiveContextPlanError::SourceOwnerMismatch);
                }
                1
            }
            LiveContextPlanOrigin::VoiceProfile { .. } => 1,
        };
        if parts.considered.through() > source_end {
            return Err(LiveContextPlanError::InvalidSourceWindow);
        }
        let mut covered = Vec::new();
        for omission in &parts.omissions {
            if omission.source.after() == omission.source.through()
                || (parts.policy == LiveContextProjectionPolicy::Reject
                    && matches!(
                        omission.reason,
                        LiveContextOmissionReason::OlderThanWindow
                            | LiveContextOmissionReason::OverBudget
                    ))
            {
                return Err(LiveContextPlanError::InvalidOmission);
            }
            covered.push(omission.source);
        }
        let mut attempts = HashSet::new();
        let mut events = HashSet::new();
        let mut previous_source = parts.considered.after();
        let mut source_chunk_count = 0;
        let mut source_chunks_seen = 0;
        let mut unattempted_seen = false;
        for chunk in &parts.chunks {
            if chunk.source_ordinal <= parts.considered.after()
                || chunk.source_ordinal > parts.considered.through()
                || chunk.source_ordinal < previous_source
                || !(1..=400).contains(&chunk.decoded_utf8_bytes)
            {
                return Err(LiveContextPlanError::InvalidChunk);
            }
            if chunk.source_ordinal != previous_source {
                if source_chunks_seen != source_chunk_count {
                    return Err(LiveContextPlanError::IncompleteSourceChunks);
                }
                covered.push(LiveContextSourceWindow::new(
                    chunk.source_ordinal - 1,
                    chunk.source_ordinal,
                )?);
                source_chunks_seen = 0;
                source_chunk_count = chunk.source_chunk_count.get();
            }
            if chunk.source_chunk_index != source_chunks_seen
                || chunk.source_chunk_count.get() != source_chunk_count
                || source_chunk_count > 64
                || source_chunks_seen >= source_chunk_count
            {
                return Err(LiveContextPlanError::IncompleteSourceChunks);
            }
            source_chunks_seen += 1;
            previous_source = chunk.source_ordinal;
            match &chunk.attempt {
                Some(attempt) => {
                    if unattempted_seen
                        || !attempts.insert(&attempt.attempt_id)
                        || attempt
                            .event_id
                            .as_ref()
                            .is_some_and(|id| id.as_str().len() > 128 || !events.insert(id))
                    {
                        return Err(LiveContextPlanError::InvalidAttemptPrefix);
                    }
                }
                None => unattempted_seen = true,
            }
            match chunk.write {
                LiveContextChunkDeliveryState::NotClaimed => {
                    if chunk.attempt.is_some() || chunk.correlated_injection.is_some() {
                        return Err(LiveContextPlanError::InvalidAttemptPrefix);
                    }
                }
                LiveContextChunkDeliveryState::AbandonedBeforeWrite => {
                    if chunk.correlated_injection.is_some() {
                        return Err(LiveContextPlanError::InvalidInjection);
                    }
                }
                LiveContextChunkDeliveryState::Claimed
                | LiveContextChunkDeliveryState::NotEnqueued
                | LiveContextChunkDeliveryState::WrittenInjectionUnconfirmed
                | LiveContextChunkDeliveryState::CorrelatedInjectionObserved
                | LiveContextChunkDeliveryState::AmbiguousUnfenced
                | LiveContextChunkDeliveryState::AmbiguousFenced => {
                    if chunk.attempt.is_none() {
                        return Err(LiveContextPlanError::InvalidAttemptPrefix);
                    }
                }
            }
            if let Some(injection) = &chunk.correlated_injection {
                if !matches!(
                    chunk.write,
                    LiveContextChunkDeliveryState::WrittenInjectionUnconfirmed
                        | LiveContextChunkDeliveryState::CorrelatedInjectionObserved
                ) || chunk
                    .attempt
                    .as_ref()
                    .and_then(|attempt| attempt.event_id.as_ref())
                    != Some(&injection.event_id)
                {
                    return Err(LiveContextPlanError::InvalidInjection);
                }
            } else if chunk.write == LiveContextChunkDeliveryState::CorrelatedInjectionObserved {
                return Err(LiveContextPlanError::InvalidInjection);
            }
        }
        if source_chunks_seen != source_chunk_count {
            return Err(LiveContextPlanError::IncompleteSourceChunks);
        }
        covered.sort_by_key(|range| range.after());
        let mut through = parts.considered.after();
        for range in covered {
            if range.after() != through || range.through() > parts.considered.through() {
                return Err(LiveContextPlanError::IncompleteCoverage);
            }
            through = range.through();
        }
        if through != parts.considered.through() {
            return Err(LiveContextPlanError::IncompleteCoverage);
        }
        let plan = Self {
            format: parts.format,
            plan_id: parts.plan_id,
            session_id: parts.session_id,
            channel_id: parts.channel_id,
            origin: parts.origin,
            intent: parts.intent,
            policy: parts.policy,
            considered: parts.considered,
            omissions: parts.omissions,
            chunks: parts.chunks,
            uncorrelated_injections: parts.uncorrelated_injections,
        };
        let bytes = meerkat_contracts::wire::live_observation::LiveObservationWireCodecV1::encode_ledger_record(&plan)
            .map_err(|error| LiveContextPlanError::Encoding(error.to_string()))?;
        if bytes.len() > 64 * 1024 {
            return Err(LiveContextPlanError::PlanBound);
        }
        Ok(plan)
    }

    pub fn frontiers(&self) -> LiveContextPlanFrontiers {
        LiveContextPlanFrontiers {
            considered_source_through: self.considered.through(),
            attempted_chunk_count: self
                .chunks
                .iter()
                .take_while(|chunk| chunk.attempt.is_some())
                .count(),
            correlated_injection_chunk_count: self
                .chunks
                .iter()
                .take_while(|chunk| chunk.correlated_injection.is_some())
                .count(),
        }
    }
    pub fn chunks(&self) -> &[LiveContextChunkRecord] {
        &self.chunks
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveContextPlanParts {
    pub format: LiveLedgerFormatV1,
    pub plan_id: OperationId,
    pub session_id: SessionId,
    pub channel_id: LiveChannelId,
    pub origin: LiveContextPlanOrigin,
    pub intent: LiveContextIntent,
    pub policy: LiveContextProjectionPolicy,
    pub considered: LiveContextSourceWindow,
    pub omissions: Vec<LiveContextOmission>,
    pub chunks: Vec<LiveContextChunkRecord>,
    pub uncorrelated_injections: Vec<LiveTranscriptRange>,
}

impl TryFrom<LiveContextPlanParts> for LiveContextPlanRecord {
    type Error = LiveContextPlanError;
    fn try_from(value: LiveContextPlanParts) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LiveContextPlanError {
    #[error("context source does not bind the selected session")]
    SourceOwnerMismatch,
    #[error("invalid context source window")]
    InvalidSourceWindow,
    #[error("context plan exceeds its count or encoded-byte bound")]
    PlanBound,
    #[error("only the voice-profile slot may supply instructions")]
    InstructionsProvenance,
    #[error("invalid or policy-forbidden context omission")]
    InvalidOmission,
    #[error("invalid, out-of-order or oversized context chunk")]
    InvalidChunk,
    #[error("a selected source must retain every ordered chunk")]
    IncompleteSourceChunks,
    #[error("context attempts must be a distinct ordered prefix")]
    InvalidAttemptPrefix,
    #[error("injection needs exact event correlation and a physical write")]
    InvalidInjection,
    #[error("every considered source must be selected or explicitly omitted exactly once")]
    IncompleteCoverage,
    #[error("cannot encode context plan: {0}")]
    Encoding(String),
}
