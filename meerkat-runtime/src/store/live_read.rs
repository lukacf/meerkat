//! Store-owned atomic reads and prepared independent Live component writes.
//!
//! Capture parts are backend content, not read authority. Only an invocation
//! of the declared atomic backend seam seals them into a composite read.

use std::sync::Arc;

use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::{Session, SessionId};
use sha2::{Digest, Sha256};

use super::{
    CommittedWholeBlobSnapshot, HeadCanonicalStoreAuthority, RuntimeSessionAuthority,
    RuntimeStoreError, WholeBlobStoreAuthority,
};
use crate::live_ledger::record::LiveLedgerRecord;
use crate::live_ledger::transcript::LiveHeadReference;

pub const LIVE_COMPOSITE_MAX_RECORDS: usize = 256;
pub const LIVE_COMPOSITE_MAX_RECORD_BYTES: usize = 128 * 1024;

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone)]
pub(crate) struct LiveCompositeTestPause {
    pub reached: std::sync::mpsc::Sender<()>,
    pub resume: Arc<std::sync::Mutex<std::sync::mpsc::Receiver<()>>>,
}

#[cfg(any(test, feature = "test-support"))]
impl LiveCompositeTestPause {
    pub fn wait(&self) -> Result<(), RuntimeStoreError> {
        self.reached
            .send(())
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        self.resume
            .lock()
            .map_err(|_| RuntimeStoreError::ReadFailed("live read fixture lock poisoned".into()))?
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|error| {
                RuntimeStoreError::ReadFailed(format!("live read fixture resume failed: {error}"))
            })?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveCompositeReadRequest {
    session_id: SessionId,
    channel_id: Option<LiveChannelId>,
    after_sequence: u64,
    limit: usize,
}

impl LiveCompositeReadRequest {
    pub fn new(
        session_id: SessionId,
        channel_id: Option<LiveChannelId>,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Self, RuntimeStoreError> {
        if !(1..=LIVE_COMPOSITE_MAX_RECORDS).contains(&limit)
            || channel_id
                .as_ref()
                .is_some_and(|id| id.as_str().is_empty() || id.as_str().len() > 128)
        {
            return Err(RuntimeStoreError::ReadFailed(
                "invalid bounded live composite window".into(),
            ));
        }
        Ok(Self {
            session_id,
            channel_id,
            after_sequence,
            limit,
        })
    }
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    pub fn channel_id(&self) -> Option<&LiveChannelId> {
        self.channel_id.as_ref()
    }
    pub const fn after_sequence(&self) -> u64 {
        self.after_sequence
    }
    pub const fn limit(&self) -> usize {
        self.limit
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveCompositeReadProfile {
    Unsupported,
    AtomicSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveLedgerWriteProfile {
    Unsupported,
    /// Head/event atomicity only; does not declare source/input admission.
    AtomicHeadEvents,
    /// Source-row CAS is included, but ordinary input admission remains separate.
    AtomicHeadEventsSources,
}

/// A physical backend supplies both domains from ONE snapshot/lock. This
/// capability does not advertise atomic Live admission or mutation support.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait RuntimeLiveLedgerOps: Send + Sync {
    fn composite_read_profile(&self) -> LiveCompositeReadProfile;

    fn ledger_write_profile(&self) -> LiveLedgerWriteProfile {
        LiveLedgerWriteProfile::Unsupported
    }

    async fn read_live_history(
        &self,
        _request: &super::live_history::LiveHistoryReadRequest,
    ) -> Result<super::live_history::LiveHistoryWindow, super::live_history::LiveHistoryReadError>
    {
        Err(RuntimeStoreError::Unsupported("retained Live prefix reads".into()).into())
    }

    async fn lookup_live_source(
        &self,
        _source: &meerkat_core::live_execution::request::LiveSourceKey,
    ) -> Result<Option<crate::live_ledger::source::LiveSourceRow>, RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "atomic live source lookup".into(),
        ))
    }

    async fn load_live_head(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<crate::live_ledger::write::LiveLedgerStoredHead>, RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "live head payload read".into(),
        ))
    }

    async fn capture_live_composite(
        &self,
        request: &LiveCompositeReadRequest,
    ) -> Result<Option<LiveCompositeCapture>, RuntimeStoreError>;

    /// Prepared component persistence is distinct from ordinary input admission.
    /// Unknown backends fail closed instead of attempting independent writes.
    async fn commit_live_ledger(
        &self,
        _prepared: crate::live_ledger::write::PreparedLiveLedgerCommit,
        _write_fence: Arc<dyn super::RuntimeStoreWriteFence>,
    ) -> Result<crate::live_ledger::write::LiveLedgerCommitOutcome, RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "atomic live head/event commit".into(),
        ))
    }
}

/// Raw atomic-backend output. Constructing capture content never issues a
/// composite read handle; the declared backend call below does that.
pub struct LiveCompositeCapture {
    session: Arc<Session>,
    actor: RuntimeSessionAuthority,
    live_head: Option<LiveHeadReference>,
    records: Vec<LiveLedgerRecord>,
    has_more: bool,
}

impl LiveCompositeCapture {
    pub fn whole_blob(
        bytes: Arc<Vec<u8>>,
        actor: WholeBlobStoreAuthority,
        live_head: Option<LiveHeadReference>,
        records: Vec<LiveLedgerRecord>,
        has_more: bool,
    ) -> Result<Self, RuntimeStoreError> {
        let snapshot = CommittedWholeBlobSnapshot::new(bytes, actor)?;
        Ok(Self {
            session: snapshot.session_arc(),
            actor: RuntimeSessionAuthority::WholeBlob(snapshot.authority().clone()),
            live_head,
            records,
            has_more,
        })
    }

    /// `materialized` must be the committed runtime boundary head, not the
    /// actor's physical provisional tail, verified in this same transaction.
    pub fn head_canonical(
        materialized: meerkat_core::VerifiedSessionHeadMaterialization,
        actor: HeadCanonicalStoreAuthority,
        live_head: Option<LiveHeadReference>,
        records: Vec<LiveLedgerRecord>,
        has_more: bool,
    ) -> Result<Self, RuntimeStoreError> {
        if materialized.session().id() != actor.session_id()
            || meerkat_core::session_head_cas_token(materialized.head())
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                != actor.committed_head_token()
        {
            return Err(RuntimeStoreError::ReadFailed(
                "live composite actor session mismatch".into(),
            ));
        }
        Ok(Self {
            session: Arc::clone(materialized.session()),
            actor: RuntimeSessionAuthority::HeadCanonical(actor),
            live_head,
            records,
            has_more,
        })
    }
}

/// Not deserializable and not constructible by pairing two public reads.
///
/// ```compile_fail
/// use meerkat_runtime::store::live_read::LiveCompositeReadAuthority;
/// let forged = serde_json::from_str::<LiveCompositeReadAuthority>("{}");
/// ```
pub struct LiveCompositeReadAuthority {
    actor: RuntimeSessionAuthority,
    live_head: Option<LiveHeadReference>,
    selection: LiveCompositeReadRequest,
    record_window_digest: [u8; 32],
}

impl std::fmt::Debug for LiveCompositeReadAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveCompositeReadAuthority")
            .field("session_id", &self.actor.session_id())
            .field("actor_revision", &self.actor.store_revision())
            .field("live_head", &self.live_head)
            .finish_non_exhaustive()
    }
}

impl LiveCompositeReadAuthority {
    pub fn actor(&self) -> &RuntimeSessionAuthority {
        &self.actor
    }
    pub fn live_head(&self) -> Option<&LiveHeadReference> {
        self.live_head.as_ref()
    }
    pub fn selection(&self) -> &LiveCompositeReadRequest {
        &self.selection
    }
    pub const fn record_window_digest(&self) -> &[u8; 32] {
        &self.record_window_digest
    }
}

pub struct LiveCompositeRead {
    session: Arc<Session>,
    authority: LiveCompositeReadAuthority,
    records: Vec<LiveLedgerRecord>,
    has_more: bool,
}

impl LiveCompositeRead {
    pub fn session(&self) -> &Session {
        &self.session
    }
    pub fn authority(&self) -> &LiveCompositeReadAuthority {
        &self.authority
    }
    pub fn records(&self) -> &[LiveLedgerRecord] {
        &self.records
    }
    pub const fn has_more(&self) -> bool {
        self.has_more
    }
}

pub(crate) fn bounded_live_window(
    request: &LiveCompositeReadRequest,
    records: impl Iterator<Item = Result<LiveLedgerRecord, RuntimeStoreError>>,
) -> Result<(Vec<LiveLedgerRecord>, bool), RuntimeStoreError> {
    let mut selected = Vec::new();
    let mut bytes = 0_usize;
    for record in records {
        let record = record?;
        let encoded = record
            .encode()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let next_bytes = bytes
            .checked_add(encoded.len())
            .ok_or_else(|| RuntimeStoreError::ReadFailed("live read byte count overflow".into()))?;
        if selected.len() == request.limit() || next_bytes > LIVE_COMPOSITE_MAX_RECORD_BYTES {
            if selected.is_empty() {
                return Err(RuntimeStoreError::ReadFailed(
                    "stored live record exceeds its accepted read bound".into(),
                ));
            }
            return Ok((selected, true));
        }
        bytes = next_bytes;
        selected.push(record);
    }
    Ok((selected, false))
}

pub async fn read_live_composite(
    store: &dyn RuntimeLiveLedgerOps,
    request: LiveCompositeReadRequest,
) -> Result<Option<LiveCompositeRead>, RuntimeStoreError> {
    if store.composite_read_profile() != LiveCompositeReadProfile::AtomicSnapshot {
        return Err(RuntimeStoreError::Unsupported(
            "atomic live composite reads".into(),
        ));
    }
    let Some(capture) = store.capture_live_composite(&request).await? else {
        return Ok(None);
    };
    if capture.session.id() != request.session_id()
        || capture.actor.session_id() != request.session_id()
        || capture
            .live_head
            .as_ref()
            .is_some_and(|head| &head.session_id != request.session_id())
        || capture.records.len() > request.limit()
    {
        return Err(RuntimeStoreError::ReadFailed(
            "live composite capture has mismatched owner or bounds".into(),
        ));
    }
    let end = capture
        .live_head
        .as_ref()
        .map_or(0, |head| head.event_count);
    if request.after_sequence() > end || (capture.has_more && capture.records.is_empty()) {
        return Err(RuntimeStoreError::ReadFailed(
            "live composite capture cannot advance its window".into(),
        ));
    }
    let mut digest = Sha256::new();
    digest.update(b"meerkat.live-composite-window.v1\0");
    let mut bytes = 0_usize;
    let mut previous = request.after_sequence();
    for record in &capture.records {
        if record.sequence().get() <= previous
            || record.sequence().get() > end
            || (request.channel_id().is_none()
                && previous.checked_add(1) != Some(record.sequence().get()))
            || request
                .channel_id()
                .is_some_and(|channel| channel != record.channel_id())
            || matches!(record, LiveLedgerRecord::Completion(record) if &record.session_id != request.session_id())
        {
            return Err(RuntimeStoreError::ReadFailed(
                "live composite capture has an invalid ordered record window".into(),
            ));
        }
        let encoded = record
            .encode()
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        bytes = bytes.checked_add(encoded.len()).ok_or_else(|| {
            RuntimeStoreError::ReadFailed("live composite byte charge overflow".into())
        })?;
        if bytes > LIVE_COMPOSITE_MAX_RECORD_BYTES {
            return Err(RuntimeStoreError::ReadFailed(
                "live composite capture exceeds its byte bound".into(),
            ));
        }
        digest.update((encoded.len() as u64).to_be_bytes());
        digest.update(encoded);
        previous = record.sequence().get();
    }
    if capture.has_more && previous >= end {
        return Err(RuntimeStoreError::ReadFailed(
            "live composite claims records beyond its captured head".into(),
        ));
    }
    if !capture.has_more && request.channel_id().is_none() && previous != end {
        return Err(RuntimeStoreError::ReadFailed(
            "live composite omitted records from its captured prefix".into(),
        ));
    }
    Ok(Some(LiveCompositeRead {
        session: capture.session,
        authority: LiveCompositeReadAuthority {
            actor: capture.actor,
            live_head: capture.live_head,
            selection: request,
            record_window_digest: digest.finalize().into(),
        },
        records: capture.records,
        has_more: capture.has_more,
    }))
}
