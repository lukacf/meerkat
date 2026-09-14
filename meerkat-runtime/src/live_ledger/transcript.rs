use meerkat_contracts::wire::live_observation::{
    LiveObservationEncodingError, LiveObservationRecord, LiveObservationWireCodecV1,
    LiveObservationWireFit, LiveObservationWireReceipt,
};
use meerkat_core::SessionId;
use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_observation::LiveObservationSeq;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum LiveLedgerFormatV1 {
    #[serde(rename = "live_ledger_v1")]
    V1,
}

/// Immutable comparison material, not a store-issued current-head witness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LiveHeadReference {
    pub format: LiveLedgerFormatV1,
    pub session_id: SessionId,
    pub generation: u64,
    pub revision: u64,
    pub event_count: u64,
    pub prefix_digest: LiveLedgerPrefixDigest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct LiveLedgerPrefixDigest([u8; 32]);

impl LiveLedgerPrefixDigest {
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub const fn from_sha256(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn empty(session_id: &SessionId, generation: u64) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"meerkat.live-ledger-prefix.v1\0");
        digest.update(session_id.to_string().as_bytes());
        digest.update(generation.to_be_bytes());
        Self(digest.finalize().into())
    }

    /// Digest calculation is mechanical, never permission to append.
    pub fn appended(self, sequence: LiveObservationSeq, encoded_record: &[u8]) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"meerkat.live-ledger-record.v1\0");
        digest.update(self.0);
        digest.update(sequence.get().to_be_bytes());
        digest.update((encoded_record.len() as u64).to_be_bytes());
        digest.update(encoded_record);
        Self(digest.finalize().into())
    }
}

/// The exact locally witnessed missing receive interval. No conversion from
/// a durable watermark exists: persistence alone cannot know lost receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(try_from = "KnownLiveReceiveGapWire")]
pub struct KnownLiveReceiveGap {
    after_received_ordinal: u64,
    through_received_ordinal: u64,
}

impl KnownLiveReceiveGap {
    pub fn new(
        after_received_ordinal: u64,
        through_received_ordinal: u64,
    ) -> Result<Self, LiveGapError> {
        if through_received_ordinal <= after_received_ordinal {
            return Err(LiveGapError);
        }
        Ok(Self {
            after_received_ordinal,
            through_received_ordinal,
        })
    }

    pub const fn after_received_ordinal(self) -> u64 {
        self.after_received_ordinal
    }

    pub const fn through_received_ordinal(self) -> u64 {
        self.through_received_ordinal
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct KnownLiveReceiveGapWire {
    #[schemars(range(max = u64::MAX - 1))]
    after_received_ordinal: u64,
    #[schemars(range(min = 1))]
    through_received_ordinal: u64,
}

impl TryFrom<KnownLiveReceiveGapWire> for KnownLiveReceiveGap {
    type Error = LiveGapError;

    fn try_from(value: KnownLiveReceiveGapWire) -> Result<Self, Self::Error> {
        Self::new(value.after_received_ordinal, value.through_received_ordinal)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveDiscontinuity {
    KnownLocalGap {
        #[schemars(extend("minLength" = 1, "maxLength" = super::completion::LIVE_COMPLETION_ID_MAX_BYTES, "x-max-utf8-bytes" = super::completion::LIVE_COMPLETION_ID_MAX_BYTES))]
        channel_id: LiveChannelId,
        observed_bounds: KnownLiveReceiveGap,
    },
    UnknownExtentCrashDiscontinuity {
        last_accepted_head: LiveHeadReference,
        #[schemars(extend("minLength" = 1, "maxLength" = super::completion::LIVE_COMPLETION_ID_MAX_BYTES, "x-max-utf8-bytes" = super::completion::LIVE_COMPLETION_ID_MAX_BYTES))]
        old_incarnation: LiveChannelId,
    },
}

/// Versioned persisted observation and its exact wire-fit receipt. The
/// receipt is rechecked on decode, not trusted because it was serialized.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "StoredLiveObservationWire")]
pub struct StoredLiveObservation {
    format: LiveLedgerFormatV1,
    record: LiveObservationRecord,
    wire_receipt: LiveObservationWireReceipt,
}

impl StoredLiveObservation {
    pub fn from_fit(fit: &LiveObservationWireFit) -> Self {
        Self {
            format: LiveLedgerFormatV1::V1,
            record: fit.record().clone(),
            wire_receipt: fit.receipt().clone(),
        }
    }

    pub fn record(&self) -> &LiveObservationRecord {
        &self.record
    }

    pub fn wire_receipt(&self) -> &LiveObservationWireReceipt {
        &self.wire_receipt
    }

    pub fn encoded_charge(
        &self,
    ) -> Result<
        crate::live_resources::LiveResourceCharge,
        super::completion::LiveCompletionEncodingError,
    > {
        let bytes = LiveObservationWireCodecV1::encode_ledger_record(self)?;
        Ok(crate::live_resources::LiveResourceCharge::for_event_record(
            &bytes,
        )?)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLiveObservationWire {
    format: LiveLedgerFormatV1,
    record: LiveObservationRecord,
    wire_receipt: LiveObservationWireReceipt,
}

impl TryFrom<StoredLiveObservationWire> for StoredLiveObservation {
    type Error = LiveObservationEncodingError;

    fn try_from(value: StoredLiveObservationWire) -> Result<Self, Self::Error> {
        let fit =
            LiveObservationWireCodecV1::restore_record_fit(value.record, &value.wire_receipt)?;
        let mut stored = Self::from_fit(&fit);
        stored.format = value.format;
        Ok(stored)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("known live receive gap requires a nonempty ordered interval")]
pub struct LiveGapError;
