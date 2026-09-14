//! Mechanical observation projection over a retained, store-verified prefix.
//! Callers authorize history access independently; cursors confer no authority.

use meerkat_contracts::wire::live_observation::{
    LIVE_OBSERVATION_PAGE_MAX_RECORDS, LIVE_OBSERVATION_REPLY_MAX_BYTES, LiveObservationCoverage,
    LiveObservationCursor, LiveObservationEncodingError, LiveObservationFilter,
    LiveObservationOwner, LiveObservationPage, LiveObservationPageQuery,
    LiveObservationReadFailure, LiveObservationSnapshot, LiveObservationWireCodecV1 as Codec,
};

use super::completion::LiveCompletionEvent;
use super::record::LiveLedgerRecord;
use super::transcript::{LiveDiscontinuity, LiveHeadReference, LiveLedgerFormatV1};
use crate::store::live_history::{LiveHistoryReadError, LiveHistoryReadRequest};
use crate::store::live_read::RuntimeLiveLedgerOps;
use crate::store::{RuntimeStore, RuntimeStoreError};
use std::sync::Arc;

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait LiveObservationHistoryReader: Send + Sync {
    async fn read(
        &self,
        owner: LiveObservationOwner,
        query: LiveObservationPageQuery,
    ) -> Result<LiveObservationPage, LiveObservationHistoryError>;
}

/// Independent retained-ledger read composition, not a SessionService history
/// adapter or an active Live channel handle.
pub struct RuntimeLiveObservationReader {
    store: Arc<dyn RuntimeStore>,
}

impl RuntimeLiveObservationReader {
    pub fn new(store: Arc<dyn RuntimeStore>) -> Self {
        Self { store }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl LiveObservationHistoryReader for RuntimeLiveObservationReader {
    async fn read(
        &self,
        owner: LiveObservationOwner,
        query: LiveObservationPageQuery,
    ) -> Result<LiveObservationPage, LiveObservationHistoryError> {
        let store = self.store.live_ledger_ops().ok_or_else(|| {
            LiveHistoryReadError::Store(RuntimeStoreError::Unsupported(
                "retained Live observation pages".into(),
            ))
        })?;
        let (filter, cursor, limit) = query.into_parts();
        read_observation_history(store, owner, filter, cursor, limit).await
    }
}

pub struct LiveObservationHistoryQuery {
    pub owner: LiveObservationOwner,
    pub filter: LiveObservationFilter,
    pub head: LiveHeadReference,
    pub coverage: LiveObservationCoverage,
    pub cursor: Option<LiveObservationCursor>,
    pub limit: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum LiveObservationHistoryError {
    #[error("Live history owner differs from the requested session")]
    OwnerMismatch,
    #[error("the session has no retained Live ledger")]
    NoRetainedHistory,
    #[error(transparent)]
    Read(#[from] LiveHistoryReadError),
    #[error(transparent)]
    Encoding(#[from] LiveObservationEncodingError),
}

impl LiveObservationHistoryError {
    pub fn failure(&self) -> LiveObservationReadFailure {
        match self {
            Self::OwnerMismatch
            | Self::Read(LiveHistoryReadError::InvalidSnapshot)
            | Self::Encoding(
                LiveObservationEncodingError::CursorMismatch
                | LiveObservationEncodingError::InvalidPrefixDigest,
            ) => LiveObservationReadFailure::CursorMismatch,
            Self::Read(LiveHistoryReadError::SnapshotExpired) => {
                LiveObservationReadFailure::CursorExpired
            }
            Self::NoRetainedHistory => LiveObservationReadFailure::Unavailable,
            Self::Read(LiveHistoryReadError::Store(RuntimeStoreError::Unsupported(_))) => {
                LiveObservationReadFailure::Unsupported
            }
            Self::Encoding(
                LiveObservationEncodingError::InvalidIdentity
                | LiveObservationEncodingError::InvalidPageLimit
                | LiveObservationEncodingError::InvalidCursor,
            ) => LiveObservationReadFailure::InvalidQuery,
            Self::Read(_) | Self::Encoding(_) => LiveObservationReadFailure::Integrity,
        }
    }
}

/// Capture or resume an authorized history read without an active channel.
/// No head, generation, coverage, or storage capability is minted on absence.
pub async fn read_observation_history(
    store: &dyn RuntimeLiveLedgerOps,
    owner: LiveObservationOwner,
    filter: LiveObservationFilter,
    cursor: Option<LiveObservationCursor>,
    limit: usize,
) -> Result<LiveObservationPage, LiveObservationHistoryError> {
    Codec::validate_query(&owner, &filter, limit)?;
    let head = if let Some(cursor) = &cursor {
        let snapshot = Codec::cursor_snapshot(cursor, &owner, &filter)?;
        LiveHeadReference {
            format: LiveLedgerFormatV1::V1,
            session_id: owner.session_id().clone(),
            generation: snapshot.generation,
            revision: snapshot.revision,
            event_count: snapshot.end_sequence,
            prefix_digest: snapshot.prefix_digest.parse()?,
        }
    } else {
        store
            .load_live_head(owner.session_id())
            .await
            .map_err(LiveHistoryReadError::from)?
            .ok_or(LiveObservationHistoryError::NoRetainedHistory)?
            .reference
    };
    let coverage = read_coverage(store, &head, &filter).await?;
    read_observation_page(
        store,
        LiveObservationHistoryQuery {
            owner,
            filter,
            head,
            coverage,
            cursor,
            limit,
        },
    )
    .await
}

// A stateless projection of retained discontinuity records, never inference
// from watermarks or cursor claims. Each backend read and allocation is bounded.
async fn read_coverage(
    store: &dyn RuntimeLiveLedgerOps,
    head: &LiveHeadReference,
    filter: &LiveObservationFilter,
) -> Result<LiveObservationCoverage, LiveObservationHistoryError> {
    let channel = match filter {
        LiveObservationFilter::AllChannels {} => None,
        LiveObservationFilter::Channel { channel_id } => Some(channel_id.clone()),
    };
    let mut coverage = LiveObservationCoverage::CompleteAcceptedPrefix;
    let mut scanned = 0;
    loop {
        let request = LiveHistoryReadRequest::new(
            head.clone(),
            channel.clone(),
            scanned,
            LIVE_OBSERVATION_PAGE_MAX_RECORDS,
        )?;
        let window = store.read_live_history(&request).await?;
        if window.head() != head {
            return Err(LiveHistoryReadError::InvalidSnapshot.into());
        }
        for record in window.records() {
            scanned = record.sequence().get();
            if let LiveLedgerRecord::Completion(completion) = record
                && let LiveCompletionEvent::ChannelDiscontinuity { discontinuity } =
                    &completion.event
            {
                match discontinuity {
                    LiveDiscontinuity::UnknownExtentCrashDiscontinuity { .. } => {
                        coverage = LiveObservationCoverage::UnknownExtentCrashDiscontinuity;
                    }
                    LiveDiscontinuity::KnownLocalGap { .. }
                        if coverage == LiveObservationCoverage::CompleteAcceptedPrefix =>
                    {
                        coverage = LiveObservationCoverage::KnownLocalGap;
                    }
                    LiveDiscontinuity::KnownLocalGap { .. } => {}
                }
            }
        }
        if !window.has_more() {
            return Ok(coverage);
        }
    }
}

pub async fn read_observation_page(
    store: &dyn RuntimeLiveLedgerOps,
    query: LiveObservationHistoryQuery,
) -> Result<LiveObservationPage, LiveObservationHistoryError> {
    if query.owner.session_id() != &query.head.session_id {
        return Err(LiveObservationHistoryError::OwnerMismatch);
    }
    let snapshot = LiveObservationSnapshot {
        generation: query.head.generation,
        revision: query.head.revision,
        end_sequence: query.head.event_count,
        prefix_digest: query.head.prefix_digest.to_string(),
        coverage: query.coverage,
    };
    let after = query.cursor.as_ref().map_or(Ok(0), |cursor| {
        Codec::cursor_after_sequence(cursor, &query.owner, &query.filter, &snapshot)
    })?;
    // Validate even empty histories before asking the backend for rows.
    Codec::page(
        query.owner.clone(),
        query.filter.clone(),
        snapshot.clone(),
        after,
        &[],
        query.limit,
        false,
    )?;
    let channel = match &query.filter {
        LiveObservationFilter::AllChannels {} => None,
        LiveObservationFilter::Channel { channel_id } => Some(channel_id.clone()),
    };
    let mut scanned = after;
    let mut observations = Vec::new();
    let mut encoded_bytes = 0_usize;
    let more = 'scan: loop {
        let request = LiveHistoryReadRequest::new(
            query.head.clone(),
            channel.clone(),
            scanned,
            LIVE_OBSERVATION_PAGE_MAX_RECORDS,
        )?;
        let window = store.read_live_history(&request).await?;
        if window.head() != &query.head {
            return Err(LiveHistoryReadError::InvalidSnapshot.into());
        }
        for record in window.records() {
            scanned = record.sequence().get();
            if let LiveLedgerRecord::Observation(stored) = record {
                let fit =
                    Codec::restore_record_fit(stored.record().clone(), stored.wire_receipt())?;
                let bytes = fit.receipt().record_encoded_bytes;
                if observations.len() == query.limit
                    || bytes > LIVE_OBSERVATION_REPLY_MAX_BYTES - encoded_bytes
                {
                    break 'scan true;
                }
                encoded_bytes += bytes;
                observations.push(fit);
            }
        }
        if !window.has_more() {
            break false;
        }
        // Controls do not become observation records or empty continuation
        // pages. Continue bounded reads until a visible lookahead or EOF.
    };
    Ok(Codec::page(
        query.owner,
        query.filter,
        snapshot,
        after,
        &observations,
        query.limit,
        more,
    )?)
}
