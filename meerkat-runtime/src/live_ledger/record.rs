//! Disjoint existing ledger encodings, with no additional wrapper bytes.
//! Each alternative rejects unknown fields, so an unknown or mixed shape
//! cannot silently become a different record kind.

use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_observation::LiveObservationSeq;
use serde::{Deserialize, Serialize};

use super::completion::{LiveCompletionEncodingError, LiveCompletionRecord};
use super::transcript::StoredLiveObservation;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LiveLedgerRecord {
    Observation(StoredLiveObservation),
    Completion(LiveCompletionRecord),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiveEventPrefixWitness {
    pub commit_revision: u64,
    pub prefix: super::transcript::LiveLedgerPrefixDigest,
}

impl LiveEventPrefixWitness {
    pub fn validate_record(
        self,
        previous: Option<Self>,
        session: &meerkat_core::SessionId,
        generation: u64,
        sequence: LiveObservationSeq,
        bytes: &[u8],
    ) -> Result<(), crate::store::RuntimeStoreError> {
        if (sequence.get() > 1) != previous.is_some()
            || self.commit_revision == 0
            || previous.is_some_and(|prior| prior.commit_revision > self.commit_revision)
        {
            return Err(crate::store::RuntimeStoreError::ReadFailed(
                "invalid live event revision chain".into(),
            ));
        }
        let prefix = previous
            .map_or_else(
                || super::transcript::LiveLedgerPrefixDigest::empty(session, generation),
                |prior| prior.prefix,
            )
            .appended(sequence, bytes);
        if prefix != self.prefix {
            return Err(crate::store::RuntimeStoreError::ReadFailed(
                "live event bytes differ from the retained prefix witness".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StoredLiveLedgerEvent {
    pub record: LiveLedgerRecord,
    pub witness: LiveEventPrefixWitness,
}

impl LiveLedgerRecord {
    pub fn sequence(&self) -> LiveObservationSeq {
        match self {
            Self::Observation(record) => record.record().sequence,
            Self::Completion(record) => record.sequence,
        }
    }

    pub fn channel_id(&self) -> &LiveChannelId {
        match self {
            Self::Observation(record) => &record.record().channel_id,
            Self::Completion(record) => &record.channel_id,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, LiveCompletionEncodingError> {
        if let Self::Completion(record) = self {
            return Ok(record.encode()?.bytes().to_vec());
        }
        Ok(meerkat_contracts::wire::live_observation::LiveObservationWireCodecV1::encode_ledger_record(self)?)
    }
}
