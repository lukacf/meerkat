//! Exact wire-fit contracts for continuous Live observation history.
//!
//! A fit receipt proves only encoding bounds. The runtime's generated owner
//! separately authorizes observation acceptance and history access.

use std::fmt;
use std::io::{self, Write};
use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use meerkat_core::SessionId;
use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_observation::{LiveObservationSeq, LiveTranscriptObservation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::supervisor_bridge::BridgeReply;

pub const LIVE_OBSERVATION_REPLY_MAX_BYTES: usize = 128 * 1024;
pub const LIVE_OBSERVATION_TEXT_MAX_BYTES: usize = 64 * 1024;
pub const LIVE_OBSERVATION_PAGE_MAX_RECORDS: usize = 256;
const ID_MAX_BYTES: usize = 128;
const CURSOR_MAX_BYTES: usize = 4096;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveObservationEncodingProfile {
    V1,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveObservationRecord {
    pub sequence: LiveObservationSeq,
    pub channel_id: LiveChannelId,
    pub observation: LiveTranscriptObservation,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveObservationOwner {
    Session {
        session_id: SessionId,
    },
    Member {
        session_id: SessionId,
        mob_id: String,
        agent_identity: String,
    },
}

impl LiveObservationOwner {
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        match self {
            Self::Session { session_id } | Self::Member { session_id, .. } => session_id,
        }
    }

    fn validate(&self) -> Result<(), LiveObservationEncodingError> {
        if let Self::Member {
            mob_id,
            agent_identity,
            ..
        } = self
        {
            validate_id(mob_id)?;
            validate_id(agent_identity)?;
        }
        Ok(())
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveObservationFilter {
    AllChannels {},
    Channel { channel_id: LiveChannelId },
}

impl LiveObservationFilter {
    fn validate(&self) -> Result<(), LiveObservationEncodingError> {
        if let Self::Channel { channel_id } = self {
            validate_id(channel_id.as_str())?;
        }
        Ok(())
    }

    fn matches(&self, record: &LiveObservationRecord) -> bool {
        match self {
            Self::AllChannels {} => true,
            Self::Channel { channel_id } => channel_id == &record.channel_id,
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveObservationCoverage {
    CompleteAcceptedPrefix,
    KnownLocalGap,
    UnknownExtentCrashDiscontinuity,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveObservationSnapshot {
    pub generation: u64,
    pub revision: u64,
    pub end_sequence: u64,
    pub prefix_digest: String,
    pub coverage: LiveObservationCoverage,
}

impl LiveObservationSnapshot {
    fn validate(&self) -> Result<(), LiveObservationEncodingError> {
        let Some(digest) = self.prefix_digest.strip_prefix("sha256:") else {
            return Err(LiveObservationEncodingError::InvalidPrefixDigest);
        };
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(LiveObservationEncodingError::InvalidPrefixDigest);
        }
        Ok(())
    }
}

/// Opaque query token, never a read authorization or bearer credential.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LiveObservationCursor(String);

impl fmt::Debug for LiveObservationCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LiveObservationCursor([REDACTED])")
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorPayload {
    profile: LiveObservationEncodingProfile,
    session_id: SessionId,
    filter: LiveObservationFilter,
    snapshot: LiveObservationSnapshot,
    after_sequence: u64,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveObservationPage {
    pub encoding_profile: LiveObservationEncodingProfile,
    pub owner: LiveObservationOwner,
    pub filter: LiveObservationFilter,
    pub snapshot: LiveObservationSnapshot,
    pub after_sequence: u64,
    pub records: Vec<Arc<LiveObservationRecord>>,
    pub has_more: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<LiveObservationCursor>,
}

/// Persistable encoding evidence, checked again when a record is loaded.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveObservationWireReceipt {
    pub profile: LiveObservationEncodingProfile,
    pub record_digest: String,
    pub record_encoded_bytes: usize,
    pub maximal_single_reply_bytes: usize,
}

/// An immutable record paired with its measured encoding proof.
#[derive(Debug, Clone)]
pub struct LiveObservationWireFit {
    record: Arc<LiveObservationRecord>,
    receipt: LiveObservationWireReceipt,
}

impl LiveObservationWireFit {
    #[must_use]
    pub fn record(&self) -> &LiveObservationRecord {
        &self.record
    }

    #[must_use]
    pub fn receipt(&self) -> &LiveObservationWireReceipt {
        &self.receipt
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LiveObservationEncodingError {
    #[error("live observation identity is empty or exceeds its wire bound")]
    InvalidIdentity,
    #[error("live observation prefix digest is not canonical SHA-256")]
    InvalidPrefixDigest,
    #[error("live observation text exceeds the decoded byte ceiling")]
    TextTooLarge,
    #[error("live observation reply exceeds its encoded byte budget")]
    EncodedReplyTooLarge,
    #[error("live observation wire receipt does not match the immutable record")]
    ReceiptMismatch,
    #[error("live observation records do not form the requested ordered window")]
    InvalidWindow,
    #[error("live observation page limit must be between 1 and 256")]
    InvalidPageLimit,
    #[error("live observation cursor is malformed or exceeds its bound")]
    InvalidCursor,
    #[error("live observation cursor belongs to another snapshot or filter")]
    CursorMismatch,
    #[error("cannot allocate bounded live observation encoding")]
    AllocationFailed,
    #[error("cannot encode live observation wire value: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// The single serializer for record fit, actual replies and cursor payloads.
pub struct LiveObservationWireCodecV1;

impl LiveObservationWireCodecV1 {
    /// Serialize an independent Live ledger record with the same bounded
    /// encoding used for observation admission, page replies and cursors.
    /// Domain validation and durable acceptance remain the ledger owner's job.
    pub fn encode_ledger_record(
        value: &impl Serialize,
    ) -> Result<Vec<u8>, LiveObservationEncodingError> {
        encode_bounded(value, LIVE_OBSERVATION_REPLY_MAX_BYTES)
    }

    pub fn check_record_fit(
        record: LiveObservationRecord,
    ) -> Result<LiveObservationWireFit, LiveObservationEncodingError> {
        validate_id(record.channel_id.as_str())?;
        if record.observation.text().len() > LIVE_OBSERVATION_TEXT_MAX_BYTES {
            return Err(LiveObservationEncodingError::TextTooLarge);
        }
        let record_bytes = encode_bounded(&record, LIVE_OBSERVATION_REPLY_MAX_BYTES)?;
        let record = Arc::new(record);
        let maximal = maximal_metadata_page(Arc::clone(&record))?;
        let maximal_single_reply_bytes = Self::encode_reply(&maximal)?.len();
        Ok(LiveObservationWireFit {
            record,
            receipt: LiveObservationWireReceipt {
                profile: LiveObservationEncodingProfile::V1,
                record_digest: format!("sha256:{:x}", Sha256::digest(&record_bytes)),
                record_encoded_bytes: record_bytes.len(),
                maximal_single_reply_bytes,
            },
        })
    }

    pub fn restore_record_fit(
        record: LiveObservationRecord,
        receipt: &LiveObservationWireReceipt,
    ) -> Result<LiveObservationWireFit, LiveObservationEncodingError> {
        let fit = Self::check_record_fit(record)?;
        if &fit.receipt != receipt {
            return Err(LiveObservationEncodingError::ReceiptMismatch);
        }
        Ok(fit)
    }

    pub fn encode_reply(
        page: &LiveObservationPage,
    ) -> Result<Vec<u8>, LiveObservationEncodingError> {
        page.owner.validate()?;
        page.filter.validate()?;
        page.snapshot.validate()?;
        if page.records.len() > LIVE_OBSERVATION_PAGE_MAX_RECORDS {
            return Err(LiveObservationEncodingError::InvalidPageLimit);
        }
        validate_window(
            page.after_sequence,
            &page.filter,
            &page.snapshot,
            page.records.iter().map(AsRef::as_ref),
        )?;
        if page.has_more != page.next_cursor.is_some()
            || (page.has_more
                && page
                    .records
                    .last()
                    .is_none_or(|record| record.sequence.get() >= page.snapshot.end_sequence))
        {
            return Err(LiveObservationEncodingError::InvalidWindow);
        }
        if let Some(cursor) = &page.next_cursor
            && (cursor.0.len() > CURSOR_MAX_BYTES
                || !cursor
                    .0
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
        {
            return Err(LiveObservationEncodingError::InvalidCursor);
        }
        encode_bounded(
            &BridgeReply::MemberLiveObservationPage(page.clone()),
            LIVE_OBSERVATION_REPLY_MAX_BYTES,
        )
    }

    /// Pack a prefix of an already-authorized, snapshot-consistent window.
    ///
    /// `more_after_window` is supplied by the store's bounded snapshot read;
    /// this codec cannot infer unseen rows or authorize their disclosure.
    pub fn page(
        owner: LiveObservationOwner,
        filter: LiveObservationFilter,
        snapshot: LiveObservationSnapshot,
        after_sequence: u64,
        window: &[LiveObservationWireFit],
        limit: usize,
        more_after_window: bool,
    ) -> Result<LiveObservationPage, LiveObservationEncodingError> {
        if !(1..=LIVE_OBSERVATION_PAGE_MAX_RECORDS).contains(&limit)
            || window.len() > LIVE_OBSERVATION_PAGE_MAX_RECORDS
        {
            return Err(LiveObservationEncodingError::InvalidPageLimit);
        }
        owner.validate()?;
        filter.validate()?;
        snapshot.validate()?;
        if more_after_window
            && window
                .last()
                .is_none_or(|fit| fit.record.sequence.get() >= snapshot.end_sequence)
        {
            return Err(LiveObservationEncodingError::InvalidWindow);
        }
        validate_window(
            after_sequence,
            &filter,
            &snapshot,
            window.iter().map(LiveObservationWireFit::record),
        )?;
        let base = LiveObservationPage {
            encoding_profile: LiveObservationEncodingProfile::V1,
            owner,
            filter,
            snapshot,
            after_sequence,
            records: Vec::new(),
            has_more: false,
            next_cursor: None,
        };
        let candidate =
            |count: usize| -> Result<LiveObservationPage, LiveObservationEncodingError> {
                let mut page = base.clone();
                page.records = window
                    .iter()
                    .take(count)
                    .map(|fit| Arc::clone(&fit.record))
                    .collect();
                page.has_more = count < window.len() || more_after_window;
                page.next_cursor = if page.has_more {
                    let last = page
                        .records
                        .last()
                        .ok_or(LiveObservationEncodingError::InvalidWindow)?;
                    Some(encode_cursor(&page, last.sequence.get())?)
                } else {
                    None
                };
                Ok(page)
            };
        let upper = window.len().min(limit);
        let full = candidate(upper)?;
        match Self::encode_reply(&full) {
            Ok(_) => return Ok(full),
            Err(LiveObservationEncodingError::EncodedReplyTooLarge) => {}
            Err(error) => return Err(error),
        }
        if upper <= 1 {
            return Err(LiveObservationEncodingError::ReceiptMismatch);
        }
        let first = candidate(1)?;
        Self::encode_reply(&first).map_err(|error| match error {
            LiveObservationEncodingError::EncodedReplyTooLarge => {
                LiveObservationEncodingError::ReceiptMismatch
            }
            other => other,
        })?;
        let mut lower = 1;
        let mut upper = upper - 1;
        while lower < upper {
            let middle = lower + (upper - lower).div_ceil(2);
            match Self::encode_reply(&candidate(middle)?) {
                Ok(_) => lower = middle,
                Err(LiveObservationEncodingError::EncodedReplyTooLarge) => upper = middle - 1,
                Err(error) => return Err(error),
            }
        }
        let page = candidate(lower)?;
        Self::encode_reply(&page)?;
        Ok(page)
    }

    pub fn cursor_after_sequence(
        cursor: &LiveObservationCursor,
        owner: &LiveObservationOwner,
        filter: &LiveObservationFilter,
        snapshot: &LiveObservationSnapshot,
    ) -> Result<u64, LiveObservationEncodingError> {
        owner.validate()?;
        filter.validate()?;
        snapshot.validate()?;
        if cursor.0.len() > CURSOR_MAX_BYTES {
            return Err(LiveObservationEncodingError::InvalidCursor);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(&cursor.0)
            .map_err(|_| LiveObservationEncodingError::InvalidCursor)?;
        let payload: CursorPayload = serde_json::from_slice(&bytes)
            .map_err(|_| LiveObservationEncodingError::InvalidCursor)?;
        if &payload.session_id != owner.session_id()
            || &payload.filter != filter
            || &payload.snapshot != snapshot
            || payload.after_sequence > snapshot.end_sequence
        {
            return Err(LiveObservationEncodingError::CursorMismatch);
        }
        Ok(payload.after_sequence)
    }
}

fn validate_id(value: &str) -> Result<(), LiveObservationEncodingError> {
    if value.is_empty() || value.len() > ID_MAX_BYTES {
        return Err(LiveObservationEncodingError::InvalidIdentity);
    }
    Ok(())
}

fn encode_cursor(
    page: &LiveObservationPage,
    after_sequence: u64,
) -> Result<LiveObservationCursor, LiveObservationEncodingError> {
    let payload = CursorPayload {
        profile: page.encoding_profile,
        session_id: page.owner.session_id().clone(),
        filter: page.filter.clone(),
        snapshot: page.snapshot.clone(),
        after_sequence,
    };
    let bytes = encode_bounded(&payload, CURSOR_MAX_BYTES / 4 * 3)?;
    let encoded = URL_SAFE_NO_PAD.encode(bytes);
    if encoded.len() > CURSOR_MAX_BYTES {
        return Err(LiveObservationEncodingError::InvalidCursor);
    }
    Ok(LiveObservationCursor(encoded))
}

fn maximal_metadata_page(
    record: Arc<LiveObservationRecord>,
) -> Result<LiveObservationPage, LiveObservationEncodingError> {
    let sequence = record.sequence.get();
    let channel_id = record.channel_id.clone();
    let mut page = LiveObservationPage {
        encoding_profile: LiveObservationEncodingProfile::V1,
        owner: LiveObservationOwner::Member {
            session_id: SessionId(uuid::Uuid::nil()),
            mob_id: "\0".repeat(ID_MAX_BYTES),
            agent_identity: "\0".repeat(ID_MAX_BYTES),
        },
        filter: LiveObservationFilter::Channel { channel_id },
        snapshot: LiveObservationSnapshot {
            generation: u64::MAX,
            revision: u64::MAX,
            end_sequence: u64::MAX,
            prefix_digest: format!("sha256:{}", "f".repeat(64)),
            coverage: LiveObservationCoverage::UnknownExtentCrashDiscontinuity,
        },
        after_sequence: sequence - 1,
        records: vec![record],
        has_more: sequence < u64::MAX,
        next_cursor: None,
    };
    if page.has_more {
        page.next_cursor = Some(encode_cursor(&page, sequence)?);
    }
    Ok(page)
}

fn validate_window<'a>(
    after_sequence: u64,
    filter: &LiveObservationFilter,
    snapshot: &LiveObservationSnapshot,
    records: impl IntoIterator<Item = &'a LiveObservationRecord>,
) -> Result<(), LiveObservationEncodingError> {
    if after_sequence > snapshot.end_sequence {
        return Err(LiveObservationEncodingError::InvalidWindow);
    }
    let mut previous = after_sequence;
    for record in records {
        validate_id(record.channel_id.as_str())?;
        if record.observation.text().len() > LIVE_OBSERVATION_TEXT_MAX_BYTES {
            return Err(LiveObservationEncodingError::TextTooLarge);
        }
        if record.sequence.get() <= previous
            || record.sequence.get() > snapshot.end_sequence
            || !filter.matches(record)
        {
            return Err(LiveObservationEncodingError::InvalidWindow);
        }
        previous = record.sequence.get();
    }
    Ok(())
}

fn encode_bounded(
    value: &impl Serialize,
    limit: usize,
) -> Result<Vec<u8>, LiveObservationEncodingError> {
    let mut writer = JsonBudgetWriter {
        bytes: Vec::new(),
        limit,
        exceeded: false,
        allocation_failed: false,
    };
    let result = serde_json::to_writer(&mut writer, value);
    if writer.exceeded {
        return Err(LiveObservationEncodingError::EncodedReplyTooLarge);
    }
    if writer.allocation_failed {
        return Err(LiveObservationEncodingError::AllocationFailed);
    }
    result?;
    Ok(writer.bytes)
}

struct JsonBudgetWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
    allocation_failed: bool,
}

impl Write for JsonBudgetWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::other(
                "live observation encoding budget exceeded",
            ));
        }
        let required = self.bytes.len() + bytes.len();
        if required > self.bytes.capacity() {
            let capacity = required
                .max(self.bytes.capacity().saturating_mul(2))
                .min(self.limit);
            if self
                .bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .is_err()
            {
                self.allocation_failed = true;
                return Err(io::Error::other("live observation allocation failed"));
            }
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
