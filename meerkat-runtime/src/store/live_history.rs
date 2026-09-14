//! Bounded immutable-prefix reads. Cursor material is not authorization.
//! Backends verify retained commit boundaries under the same read snapshot.

use meerkat_core::live_execution::LiveChannelId;

use super::RuntimeStoreError;
use super::live_read::{LIVE_COMPOSITE_MAX_RECORD_BYTES, LiveCompositeReadRequest};
use crate::live_ledger::record::{LiveEventPrefixWitness, LiveLedgerRecord};
use crate::live_ledger::transcript::{LiveHeadReference, LiveLedgerPrefixDigest};

#[derive(Debug, Clone)]
pub struct LiveHistoryReadRequest {
    head: LiveHeadReference,
    selection: LiveCompositeReadRequest,
}

impl LiveHistoryReadRequest {
    pub fn new(
        head: LiveHeadReference,
        channel: Option<LiveChannelId>,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Self, LiveHistoryReadError> {
        if head.revision == 0 || head.generation == 0 || after_sequence > head.event_count {
            return Err(LiveHistoryReadError::InvalidSnapshot);
        }
        let selection =
            LiveCompositeReadRequest::new(head.session_id.clone(), channel, after_sequence, limit)?;
        Ok(Self { head, selection })
    }
    pub fn head(&self) -> &LiveHeadReference {
        &self.head
    }
    pub fn selection(&self) -> &LiveCompositeReadRequest {
        &self.selection
    }
}

/// Read content only. The backend's declared read method, not construction of
/// this value, establishes that the requested prefix was committed.
#[derive(Debug)]
pub struct LiveHistoryWindow {
    head: LiveHeadReference,
    records: Vec<LiveLedgerRecord>,
    has_more: bool,
}

impl LiveHistoryWindow {
    pub fn new(
        request: &LiveHistoryReadRequest,
        records: Vec<LiveLedgerRecord>,
        has_more: bool,
    ) -> Result<Self, LiveHistoryReadError> {
        let selection = request.selection();
        if records.len() > selection.limit() || (records.is_empty() && has_more) {
            return Err(LiveHistoryReadError::InvalidWindow);
        }
        let mut previous = selection.after_sequence();
        let mut bytes = 0_usize;
        for record in &records {
            if record.sequence().get() <= previous
                || record.sequence().get() > request.head.event_count
                || selection
                    .channel_id()
                    .is_some_and(|channel| channel != record.channel_id())
                || (selection.channel_id().is_none()
                    && previous.checked_add(1) != Some(record.sequence().get()))
                || matches!(record, LiveLedgerRecord::Completion(row) if &row.session_id != selection.session_id())
            {
                return Err(LiveHistoryReadError::InvalidWindow);
            }
            let encoded = record
                .encode()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
            bytes = bytes
                .checked_add(encoded.len())
                .ok_or(LiveHistoryReadError::InvalidWindow)?;
            if bytes > LIVE_COMPOSITE_MAX_RECORD_BYTES {
                return Err(LiveHistoryReadError::InvalidWindow);
            }
            previous = record.sequence().get();
        }
        if (has_more && previous >= request.head.event_count)
            || (!has_more
                && selection.channel_id().is_none()
                && previous != request.head.event_count)
        {
            return Err(LiveHistoryReadError::InvalidWindow);
        }
        Ok(Self {
            head: request.head.clone(),
            records,
            has_more,
        })
    }
    pub fn head(&self) -> &LiveHeadReference {
        &self.head
    }
    pub fn records(&self) -> &[LiveLedgerRecord] {
        &self.records
    }
    pub const fn has_more(&self) -> bool {
        self.has_more
    }
}

pub(crate) fn validate_retained_prefix(
    requested: &LiveHeadReference,
    current: &LiveHeadReference,
    last: Option<LiveEventPrefixWitness>,
    next: Option<LiveEventPrefixWitness>,
) -> Result<(), LiveHistoryReadError> {
    if requested.session_id != current.session_id
        || requested.generation > current.generation
        || requested.revision == 0
        || requested.revision > current.revision
        || requested.event_count > current.event_count
    {
        return Err(LiveHistoryReadError::InvalidSnapshot);
    }
    if requested.generation < current.generation {
        return Err(LiveHistoryReadError::SnapshotExpired);
    }
    if (requested.event_count > 0) != last.is_some()
        || (requested.event_count < current.event_count) != next.is_some()
        || last
            .into_iter()
            .chain(next)
            .any(|w| w.commit_revision == 0 || w.commit_revision > current.revision)
        || matches!((last,next),(Some(a),Some(b)) if a.commit_revision > b.commit_revision)
    {
        return Err(RuntimeStoreError::ReadFailed(
            "invalid retained Live event commit witnesses".into(),
        )
        .into());
    }
    let (minimum_revision, prefix) = last.map_or_else(
        || {
            (
                1,
                LiveLedgerPrefixDigest::empty(&requested.session_id, requested.generation),
            )
        },
        |witness| (witness.commit_revision, witness.prefix),
    );
    // Revisions are consecutive. If the next event was committed in the same
    // revision, the requested end lies inside an atomic batch, not a head.
    if requested.revision < minimum_revision
        || requested.prefix_digest != prefix
        || next.is_some_and(|witness| requested.revision >= witness.commit_revision)
    {
        return Err(LiveHistoryReadError::InvalidSnapshot);
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum LiveHistoryReadError {
    #[error("Live snapshot is not retained")]
    SnapshotExpired,
    #[error("Live snapshot does not name a committed prefix")]
    InvalidSnapshot,
    #[error("invalid bounded Live history window")]
    InvalidWindow,
    #[error(transparent)]
    Store(#[from] RuntimeStoreError),
}
