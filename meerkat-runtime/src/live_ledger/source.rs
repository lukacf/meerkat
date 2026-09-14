//! Exact source-row storage content. A row digest is comparison material, not
//! source reservation or permission to create an ordinary input.

use meerkat_core::live_execution::request::LiveSourceKey;
use sha2::{Digest, Sha256};

use crate::live_resources::LiveResourceCharge;
use crate::live_source::LiveSourceEntryRecord;
use crate::store::RuntimeStoreError;

pub const LIVE_SOURCE_ROW_MAX_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("live source channel must contain 1-128 UTF-8 bytes")]
pub(crate) struct LiveSourceStorageKeyError;

pub(crate) fn validate_source_storage_key(
    source: &LiveSourceKey,
) -> Result<(), LiveSourceStorageKeyError> {
    let bytes = source.channel_id().as_str().len();
    if !(1..=128).contains(&bytes) {
        return Err(LiveSourceStorageKeyError);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct LiveSourceRowDigest([u8; 32]);

impl LiveSourceRowDigest {
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Clone)]
pub struct LiveSourceRow {
    source: LiveSourceKey,
    bytes: Vec<u8>,
    digest: LiveSourceRowDigest,
}

impl std::fmt::Debug for LiveSourceRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSourceRow")
            .field("session_id", self.source.session_id())
            .field("encoded_bytes", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

impl LiveSourceRow {
    pub fn encode(record: &LiveSourceEntryRecord) -> Result<Self, RuntimeStoreError> {
        validate_source_storage_key(record.source())
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        let bytes = meerkat_contracts::wire::live_observation::LiveObservationWireCodecV1::encode_ledger_record(record)
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        if bytes.len() > LIVE_SOURCE_ROW_MAX_BYTES {
            return Err(RuntimeStoreError::WriteFailed(
                "live source row exceeds its encoded bound".into(),
            ));
        }
        let source = record.source().clone();
        let digest = source_row_digest(&source, &bytes)?;
        Ok(Self {
            source,
            bytes,
            digest,
        })
    }

    pub fn restore(
        source: LiveSourceKey,
        bytes: Vec<u8>,
        expected_digest: &[u8],
    ) -> Result<Self, RuntimeStoreError> {
        validate_source_storage_key(&source)
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        if bytes.len() > LIVE_SOURCE_ROW_MAX_BYTES || expected_digest.len() != 32 {
            return Err(RuntimeStoreError::ReadFailed(
                "live source row exceeds its encoded bound".into(),
            ));
        }
        let digest = source_row_digest(&source, &bytes)?;
        if digest.as_bytes().as_slice() != expected_digest {
            return Err(RuntimeStoreError::ReadFailed(
                "live source row digest mismatch".into(),
            ));
        }
        let record: LiveSourceEntryRecord = serde_json::from_slice(&bytes).map_err(|error| {
            RuntimeStoreError::ReadFailed(format!("invalid live source row: {error}"))
        })?;
        let canonical = Self::encode(&record)?;
        if canonical.source != source || canonical.bytes != bytes {
            return Err(RuntimeStoreError::ReadFailed(
                "live source key or canonical encoding mismatch".into(),
            ));
        }
        Ok(Self {
            source,
            bytes,
            digest,
        })
    }

    pub fn source(&self) -> &LiveSourceKey {
        &self.source
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub const fn digest(&self) -> LiveSourceRowDigest {
        self.digest
    }

    pub fn record(&self) -> Result<LiveSourceEntryRecord, RuntimeStoreError> {
        serde_json::from_slice(&self.bytes).map_err(|error| {
            RuntimeStoreError::ReadFailed(format!("invalid live source row: {error}"))
        })
    }

    /// Both the identity column and its primary-key index copy are variable
    /// sized, in addition to the record and fixed row/index allowance.
    pub fn charge(&self) -> Result<LiveResourceCharge, RuntimeStoreError> {
        let identity = encoded_source_identity(&self.source)?;
        LiveResourceCharge::for_encoded_record(&self.bytes)
            .and_then(|base| {
                base.checked_add(
                    LiveResourceCharge {
                        records: 0,
                        encoded_bytes: identity.len() as u64,
                    }
                    .checked_mul(2)?,
                )
            })
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))
    }
}

pub(crate) fn encoded_source_identity(
    source: &LiveSourceKey,
) -> Result<Vec<u8>, RuntimeStoreError> {
    validate_source_storage_key(source)
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    meerkat_contracts::wire::live_observation::LiveObservationWireCodecV1::encode_ledger_record(
        source.source(),
    )
    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))
}

pub(crate) struct PreparedLiveSourceMutation {
    pub expected: Option<LiveSourceRowDigest>,
    pub replacement: LiveSourceRow,
}

#[derive(Default)]
pub(crate) struct LiveSourceChargeDelta {
    pub previous: LiveResourceCharge,
    pub replacement: LiveResourceCharge,
}

pub(crate) enum LiveSourceMutationCheck {
    Match(LiveSourceChargeDelta),
    Conflict {
        current: Option<LiveSourceRowDigest>,
    },
}

impl PreparedLiveSourceMutation {
    pub fn check(
        &self,
        current: Option<&LiveSourceRow>,
    ) -> Result<LiveSourceMutationCheck, RuntimeStoreError> {
        let observed = current.map(LiveSourceRow::digest);
        if self.expected != observed {
            return Ok(LiveSourceMutationCheck::Conflict { current: observed });
        }
        if let Some(current) = current
            && (current.source() != self.replacement.source()
                || !current
                    .record()?
                    .preserves_frozen_content(&self.replacement.record()?))
        {
            return Err(RuntimeStoreError::WriteFailed(
                "live source replacement changes frozen content".into(),
            ));
        }
        Ok(LiveSourceMutationCheck::Match(LiveSourceChargeDelta {
            previous: current.map_or(Ok(LiveResourceCharge::default()), LiveSourceRow::charge)?,
            replacement: self.replacement.charge()?,
        }))
    }
}

impl LiveSourceChargeDelta {
    pub fn checked_add(self, other: Self) -> Result<Self, RuntimeStoreError> {
        Ok(Self {
            previous: self
                .previous
                .checked_add(other.previous)
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?,
            replacement: self
                .replacement
                .checked_add(other.replacement)
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?,
        })
    }
}

fn source_row_digest(
    source: &LiveSourceKey,
    bytes: &[u8],
) -> Result<LiveSourceRowDigest, RuntimeStoreError> {
    let key = meerkat_contracts::wire::live_observation::LiveObservationWireCodecV1::encode_ledger_record(source)
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    let mut hash = Sha256::new();
    hash.update(b"meerkat.live-source-row.v1\0");
    for part in [key.as_slice(), bytes] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    Ok(LiveSourceRowDigest(hash.finalize().into()))
}
