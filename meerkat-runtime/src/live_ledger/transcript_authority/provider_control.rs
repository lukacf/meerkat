use meerkat_core::{
    SessionId,
    live_execution::{LiveChannelId, observation::ContinuousLiveObservation},
    live_observation::LiveObservationSeq,
};
use sha2::{Digest, Sha256};

use super::{LiveTranscriptWriteError, dsl};
use crate::{
    live_ledger::{
        completion::{LiveCompletionEvent, LiveCompletionText},
        transcript::LiveHeadReference,
    },
    store::RuntimeStoreError,
};

/// The head anchors the read/publication; the sequence is the retained fact.
/// A replay may be observed at a later head without creating another event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedLiveProviderControl {
    pub(super) head: LiveHeadReference,
    pub(super) sequence: LiveObservationSeq,
}

impl CommittedLiveProviderControl {
    pub fn head(&self) -> &LiveHeadReference {
        &self.head
    }

    pub fn sequence(&self) -> LiveObservationSeq {
        self.sequence
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveProviderControlOutcome {
    Accepted(CommittedLiveProviderControl),
    Refused(dsl::LiveProviderControlRefusal),
}

pub(in crate::live_ledger) fn digest(
    session: &SessionId,
    channel: &LiveChannelId,
    event: &LiveCompletionEvent,
) -> Result<String, RuntimeStoreError> {
    let encoded =
        serde_json::to_vec(&("meerkat.live-provider-control.v1", session, channel, event))
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

pub(super) fn event(
    observation: &ContinuousLiveObservation,
) -> Result<(dsl::LiveProviderControlKind, LiveCompletionEvent), LiveTranscriptWriteError> {
    Ok(match observation {
        ContinuousLiveObservation::ProviderStarted { provider_session } => (
            dsl::LiveProviderControlKind::Started,
            LiveCompletionEvent::ChannelProviderStarted {
                provider_session: LiveCompletionText::new(provider_session.as_str())?,
            },
        ),
        ContinuousLiveObservation::Diagnostic(diagnostic) => (
            dsl::LiveProviderControlKind::Diagnostic,
            LiveCompletionEvent::ChannelProviderDiagnostic {
                diagnostic: diagnostic.clone(),
            },
        ),
        _ => {
            return Err(RuntimeStoreError::WriteFailed(
                "not a provider control observation".into(),
            )
            .into());
        }
    })
}
