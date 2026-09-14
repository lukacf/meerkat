//! Prepared head/event/source persistence handoff.
//!
//! The prepared carrier has no public constructor or deserializer. Generated
//! Live authority will produce it; stores only verify and persist its exact
//! successor. No handwritten lifecycle reducer lives at this boundary.

use std::sync::Arc;

use meerkat_core::SessionId;
use sha2::{Digest, Sha256};

use super::record::LiveLedgerRecord;
use super::source::{LiveSourceChargeDelta, PreparedLiveSourceMutation};
use super::transcript::{LiveHeadReference, LiveLedgerFormatV1, LiveLedgerPrefixDigest};
use crate::live_resources::LiveResourceCharge;
use crate::store::{RuntimeSessionAuthority, RuntimeStoreError};

/// Fixed head keys, counters, digest, and row/index custody use the same
/// conservative bookkeeping allowance as an event, plus the exact last-commit
/// digest. Snapshot bytes are extra.
pub const LIVE_HEAD_STORAGE_ALLOWANCE_BYTES: u64 =
    crate::live_resources::LIVE_RECORD_STORAGE_ALLOWANCE_BYTES + 32;

#[derive(Clone, PartialEq, Eq)]
pub struct LiveLedgerPayloadState {
    pub used: LiveResourceCharge,
    pub reserved: LiveResourceCharge,
    pub ingress_generation: u64,
    pub transcript_snapshot: Arc<Vec<u8>>,
    pub request_snapshot: Arc<Vec<u8>>,
}

impl std::fmt::Debug for LiveLedgerPayloadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveLedgerPayloadState")
            .field("used", &self.used)
            .field("reserved", &self.reserved)
            .field("ingress_generation", &self.ingress_generation)
            .field("transcript_snapshot_bytes", &self.transcript_snapshot.len())
            .field("request_snapshot_bytes", &self.request_snapshot.len())
            .finish()
    }
}

impl LiveLedgerPayloadState {
    fn snapshot_bytes(&self) -> Result<u64, RuntimeStoreError> {
        (self.transcript_snapshot.len() as u64)
            .checked_add(self.request_snapshot.len() as u64)
            .ok_or_else(|| {
                RuntimeStoreError::WriteFailed("live snapshot byte count overflow".into())
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveLedgerStoredHead {
    pub reference: LiveHeadReference,
    pub payload: LiveLedgerPayloadState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiveLedgerCommitDigest(pub [u8; 32]);

#[derive(Debug, Clone)]
pub(crate) struct StoredLiveLedgerCommit {
    pub head: LiveLedgerStoredHead,
    pub operation: LiveLedgerCommitDigest,
}

impl LiveLedgerStoredHead {
    pub(crate) fn validate_payload(&self) -> Result<(), RuntimeStoreError> {
        let invalid =
            || RuntimeStoreError::ReadFailed("invalid live head accounting or format".into());
        let total = self
            .payload
            .used
            .checked_add(self.payload.reserved)
            .map_err(|_| invalid())?;
        let minimum_bytes = self
            .payload
            .snapshot_bytes()?
            .checked_add(LIVE_HEAD_STORAGE_ALLOWANCE_BYTES)
            .ok_or_else(invalid)?;
        if self.reference.format != LiveLedgerFormatV1::V1
            || self.reference.generation == 0
            || self.reference.revision == 0
            || self.payload.ingress_generation == 0
            || total.records > crate::live_resources::LIVE_LEDGER_MAX_CHARGE.records
            || total.encoded_bytes > crate::live_resources::LIVE_LEDGER_MAX_CHARGE.encoded_bytes
            || self.payload.used.records < self.reference.event_count
            || self.payload.used.encoded_bytes < minimum_bytes
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Canonical changes can reach a store only through a prepared owner handoff,
/// not through fields supplied by a surface or deserialized source record.
///
/// ```compile_fail
/// use meerkat_runtime::live_ledger::write::PreparedLiveLedgerCommit;
/// let forged = serde_json::from_str::<PreparedLiveLedgerCommit>("{}");
/// ```
pub struct PreparedLiveLedgerCommit {
    expected: Option<LiveHeadReference>,
    expected_actor: Option<RuntimeSessionAuthority>,
    successor: LiveLedgerStoredHead,
    records: Vec<LiveLedgerRecord>,
    sources: Vec<PreparedLiveSourceMutation>,
    quota: LiveResourceCharge,
}

impl PreparedLiveLedgerCommit {
    pub fn session_id(&self) -> &SessionId {
        &self.successor.reference.session_id
    }
    pub fn expected(&self) -> Option<&LiveHeadReference> {
        self.expected.as_ref()
    }
    pub fn expected_actor(&self) -> Option<&RuntimeSessionAuthority> {
        self.expected_actor.as_ref()
    }
    pub fn successor(&self) -> &LiveLedgerStoredHead {
        &self.successor
    }
    pub fn records(&self) -> &[LiveLedgerRecord] {
        &self.records
    }

    pub(crate) fn prefix_witnesses<'a>(
        &'a self,
        encoded: &'a [Vec<u8>],
    ) -> impl Iterator<Item = super::record::LiveEventPrefixWitness> + 'a {
        let prefix = self.expected.as_ref().map_or_else(
            || {
                LiveLedgerPrefixDigest::empty(
                    self.session_id(),
                    self.successor.reference.generation,
                )
            },
            |head| head.prefix_digest,
        );
        self.records
            .iter()
            .zip(encoded)
            .scan(prefix, |prefix, (record, bytes)| {
                *prefix = prefix.appended(record.sequence(), bytes);
                Some(super::record::LiveEventPrefixWitness {
                    commit_revision: self.successor.reference.revision,
                    prefix: *prefix,
                })
            })
    }
    pub(crate) fn sources(&self) -> &[PreparedLiveSourceMutation] {
        &self.sources
    }

    pub(crate) fn validate_new_source_context(
        &self,
        source: &PreparedLiveSourceMutation,
        current: Option<&super::source::LiveSourceRow>,
    ) -> Result<(), RuntimeStoreError> {
        if current.is_none()
            && let crate::live_source::LiveSourceEntryRecord::Reservation { record } =
                source.replacement.record()?
            && (Some(&record.context().live_head) != self.expected.as_ref()
                || self
                    .expected_actor
                    .as_ref()
                    .is_none_or(|actor| !record.context().actor.matches_authority(actor)))
        {
            return Err(RuntimeStoreError::WriteFailed(
                "new live source requires its exact composite actor/head fence".into(),
            ));
        }
        Ok(())
    }

    /// Bind the whole operation, not just the resulting prefix. Length framing
    /// is versioned and snapshots/record bytes feed the hash without copying.
    pub(crate) fn operation_digest(
        &self,
        encoded: &[Vec<u8>],
    ) -> Result<LiveLedgerCommitDigest, RuntimeStoreError> {
        let Self {
            expected,
            expected_actor,
            successor,
            records,
            sources,
            quota,
        } = self;
        let LiveLedgerStoredHead { reference, payload } = successor;
        let LiveLedgerPayloadState {
            used,
            reserved,
            ingress_generation,
            transcript_snapshot,
            request_snapshot,
        } = payload;
        if encoded.len() != records.len() {
            return Err(RuntimeStoreError::WriteFailed(
                "live operation record count differs".into(),
            ));
        }
        let mut hash = Sha256::new();
        hash.update(b"meerkat.live-ledger-commit.v1\0");
        hash_serialized_part(&mut hash, expected)?;
        match expected_actor {
            None => hash.update([0]),
            Some(RuntimeSessionAuthority::WholeBlob(actor)) => {
                hash.update([1]);
                hash_serialized_part(
                    &mut hash,
                    &(
                        actor.authority_version(),
                        actor.session_id(),
                        actor.store_revision(),
                        actor.blob_sha256(),
                    ),
                )?;
            }
            Some(RuntimeSessionAuthority::HeadCanonical(actor)) => {
                hash.update([2]);
                hash_serialized_part(
                    &mut hash,
                    &(
                        actor.authority_version(),
                        actor.session_id(),
                        actor.store_revision(),
                        actor.boundary_head(),
                        actor.committed_head_token(),
                    ),
                )?;
            }
        }
        hash_serialized_part(
            &mut hash,
            &(reference, used, reserved, ingress_generation, quota),
        )?;
        hash_part(&mut hash, transcript_snapshot);
        hash_part(&mut hash, request_snapshot);
        hash.update((encoded.len() as u64).to_be_bytes());
        for record in encoded {
            hash_part(&mut hash, record);
        }
        hash.update((sources.len() as u64).to_be_bytes());
        for source in sources {
            hash_serialized_part(&mut hash, &(source.expected, source.replacement.source()))?;
            hash_part(&mut hash, source.replacement.bytes());
        }
        Ok(LiveLedgerCommitDigest(hash.finalize().into()))
    }

    pub(crate) fn encoded_records(&self) -> Result<Vec<Vec<u8>>, RuntimeStoreError> {
        let encoded: Vec<Vec<u8>> = self.records.iter().map(|record| {
            let bytes = record.encode().map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            if bytes.len() > crate::store::live_read::LIVE_COMPOSITE_MAX_RECORD_BYTES
                || record.channel_id().as_str().is_empty()
                || record.channel_id().as_str().len() > 128
                || matches!(record, LiveLedgerRecord::Completion(record) if &record.session_id != self.session_id())
            {
                return Err(RuntimeStoreError::WriteFailed("invalid live record owner or bound".into()));
            }
            Ok(bytes)
        })        .collect::<Result<_, _>>()?;
        // Validate even on replay: a subset of the originally committed batch
        // must not count as the same operation merely because its rows match.
        let invalid =
            || RuntimeStoreError::WriteFailed("invalid prepared live record prefix".into());
        let after = &self.successor.reference;
        let (revision, mut sequence, mut prefix) = match &self.expected {
            Some(before) => {
                if before.session_id != after.session_id || before.generation != after.generation {
                    return Err(invalid());
                }
                (before.revision, before.event_count, before.prefix_digest)
            }
            None => (
                0,
                0,
                LiveLedgerPrefixDigest::empty(self.session_id(), after.generation),
            ),
        };
        if revision.checked_add(1) != Some(after.revision) {
            return Err(invalid());
        }
        for (record, bytes) in self.records.iter().zip(&encoded) {
            sequence = sequence.checked_add(1).ok_or_else(invalid)?;
            if record.sequence().get() != sequence {
                return Err(invalid());
            }
            prefix = prefix.appended(record.sequence(), bytes);
        }
        if sequence != after.event_count || prefix != after.prefix_digest {
            return Err(invalid());
        }
        self.successor.validate_payload()?;
        let mut keys = std::collections::HashSet::new();
        for source in &self.sources {
            super::source::validate_source_storage_key(source.replacement.source())
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            if source.replacement.source().session_id() != self.session_id()
                || !keys.insert(source.replacement.source())
            {
                return Err(RuntimeStoreError::WriteFailed(
                    "duplicate or foreign source mutation".into(),
                ));
            }
        }
        Ok(encoded)
    }

    /// Mechanical validation against the state observed by this transaction.
    /// Counter, prefix, byte and revision checks never select a lifecycle step.
    pub(crate) fn validate(
        &self,
        before: Option<&LiveLedgerStoredHead>,
        encoded: &[Vec<u8>],
        sources: LiveSourceChargeDelta,
    ) -> Result<(), RuntimeStoreError> {
        let invalid =
            || RuntimeStoreError::WriteFailed("invalid prepared live head/event transition".into());
        let after = &self.successor;
        if self.expected.as_ref() != before.map(|head| &head.reference)
            || after.reference.format != LiveLedgerFormatV1::V1
            || after.reference.generation == 0
            || after.payload.ingress_generation == 0
            || self.records.len() != encoded.len()
            || self
                .expected_actor
                .as_ref()
                .is_some_and(|actor| actor.session_id() != self.session_id())
            || self.quota.records > crate::live_resources::LIVE_LEDGER_MAX_CHARGE.records
            || self.quota.encoded_bytes
                > crate::live_resources::LIVE_LEDGER_MAX_CHARGE.encoded_bytes
            || [
                after.reference.generation,
                after.reference.revision,
                after.reference.event_count,
                after.payload.ingress_generation,
            ]
            .into_iter()
            .any(|value| i64::try_from(value).is_err())
        {
            return Err(invalid());
        }
        let (revision, count, mut prefix, used, old_snapshot_bytes) = match before {
            None => (
                0,
                0,
                LiveLedgerPrefixDigest::empty(self.session_id(), after.reference.generation),
                LiveResourceCharge {
                    records: 0,
                    encoded_bytes: LIVE_HEAD_STORAGE_ALLOWANCE_BYTES,
                },
                0,
            ),
            Some(before) => {
                if before.reference.session_id != *self.session_id()
                    || before.reference.generation != after.reference.generation
                    || before.payload.ingress_generation > after.payload.ingress_generation
                {
                    return Err(invalid());
                }
                (
                    before.reference.revision,
                    before.reference.event_count,
                    before.reference.prefix_digest,
                    before.payload.used,
                    before.payload.snapshot_bytes()?,
                )
            }
        };
        if revision.checked_add(1) != Some(after.reference.revision) {
            return Err(invalid());
        }
        let mut sequence = count;
        let mut appended = LiveResourceCharge::default();
        for (record, bytes) in self.records.iter().zip(encoded) {
            sequence = sequence.checked_add(1).ok_or_else(invalid)?;
            if record.sequence().get() != sequence
                || matches!(record, LiveLedgerRecord::Completion(record) if &record.session_id != self.session_id())
            {
                return Err(invalid());
            }
            prefix = prefix.appended(record.sequence(), bytes);
            appended = appended
                .checked_add(LiveResourceCharge::for_event_record(bytes).map_err(|_| invalid())?)
                .map_err(|_| invalid())?;
        }
        let new_snapshot_bytes = after.payload.snapshot_bytes()?;
        let expected_bytes = used
            .encoded_bytes
            .checked_sub(old_snapshot_bytes)
            .and_then(|bytes| bytes.checked_sub(sources.previous.encoded_bytes))
            .and_then(|bytes| bytes.checked_add(new_snapshot_bytes))
            .and_then(|bytes| bytes.checked_add(appended.encoded_bytes))
            .and_then(|bytes| bytes.checked_add(sources.replacement.encoded_bytes))
            .ok_or_else(invalid)?;
        if after.reference.event_count != sequence
            || after.reference.prefix_digest != prefix
            || used
                .records
                .checked_sub(sources.previous.records)
                .and_then(|records| records.checked_add(sources.replacement.records))
                .and_then(|records| records.checked_add(appended.records))
                != Some(after.payload.used.records)
            || after.payload.used.encoded_bytes != expected_bytes
        {
            return Err(invalid());
        }
        let total = after
            .payload
            .used
            .checked_add(after.payload.reserved)
            .map_err(|_| invalid())?;
        if total.records > self.quota.records || total.encoded_bytes > self.quota.encoded_bytes {
            return Err(RuntimeStoreError::WriteFailed(
                "prepared live state exceeds used-plus-reserved quota".into(),
            ));
        }
        Ok(())
    }
}

fn hash_part(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

fn hash_serialized_part(
    hash: &mut Sha256,
    value: &impl serde::Serialize,
) -> Result<(), RuntimeStoreError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    hash_part(hash, &bytes);
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiveLedgerCommitOutcome {
    Committed {
        head: LiveHeadReference,
    },
    AlreadyCommitted {
        head: LiveHeadReference,
    },
    Conflict {
        current: Option<LiveHeadReference>,
    },
    ActorConflict {
        current: Option<RuntimeSessionAuthority>,
    },
    SourceConflict {
        source: meerkat_core::live_execution::request::LiveSourceKey,
        current: Option<super::source::LiveSourceRowDigest>,
    },
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
