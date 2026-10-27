//! Meta's own records on a host store realm.
//!
//! A realm kept in a host's database through the host store SPI
//! ([`meerkat_core::host_store`]) holds Core's runtime, session and mob
//! scopes. The few Meta records that are authority for the realm live beside
//! them in one Meta-owned record family, [`META_DOCUMENT_FAMILY`] (tag `mx`,
//! a tag Core leaves to the layers above it). Each record is a scope of its
//! own whose one pinned entry is the document, so a boot reads it from the
//! listed head; the realm moves a document larger than a head allows to a
//! chunk scope.
//!
//! | Key | Document |
//! |-----|----------|
//! | `realm` | the realm record ([`HostStoreRealmRecord`]) |
//! | `composition/<storage key>` | the mob composition manifest |
//! | `first-boot` | the first-boot manifest |
//! | `held/<SHA-256 of the identity>` | one held-completion mark |
//! | `cursor/<mob id>` | the structural-events subscription cursor |
//! | `idle/<mob id length>:<mob id><member id>` | one member's idle-retirement opt-in |
//!
//! Every document goes through the realm's one [`SpiRealm`] under its fence:
//! a write is one single-scope batch, settled before it answers. A batch
//! that fails poisons that document's scope, and the scope refuses every
//! later call until the realm is reopened (the host restarts the process).
//! The family is claimed once per realm, before any of its scopes is touched,
//! so a document listed at boot is never created a second time.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use meerkat_core::host_store::ScopeKind;
use meerkat_core::types::SessionId;
use meerkat_runtime::store::spi::{
    ArmedScopes, ScopeDelta, ScopeFamily, ScopeId, SpiError, SpiRealm,
};
use serde::{Deserialize, Serialize};

use crate::mob_handle_runtime::DelegateIdleRetireOverride;
use crate::runtime::{MemberIdleRetireOverrideRecord, MetadataStoreError, PersistentMetadataStore};

/// Meta's record family on a host store realm: one record scope per
/// document, its document the pinned entry `v`.
pub(crate) const META_DOCUMENT_FAMILY: ScopeFamily = ScopeFamily {
    tag: "mx",
    kind: ScopeKind::Record,
    pinned: &["v"],
};

/// The one entry of a document scope.
const DOCUMENT_ENTRY: &str = "v";

/// The realm record's key.
pub(crate) const REALM_RECORD_KEY: &str = "realm";

/// The first-boot manifest's key.
pub(crate) const FIRST_BOOT_KEY: &str = "first-boot";

/// The key prefix of the held-completion marks.
pub(crate) const HELD_MARK_PREFIX: &str = "held/";

const CURSOR_PREFIX: &str = "cursor/";
const IDLE_PREFIX: &str = "idle/";

/// The format version of the cursor and idle opt-in documents.
const ENTRY_VERSION: u32 = 1;

/// The composition manifest's key for the mob storage `storage_key`.
pub(crate) fn composition_key(storage_key: &str) -> String {
    format!("composition/{storage_key}")
}

/// The key of one held-completion mark, by the mark's name (the lowercase
/// hex SHA-256 of its identity, as the state directory names its files).
pub(crate) fn held_mark_key(mark_name: &str) -> String {
    format!("{HELD_MARK_PREFIX}{mark_name}")
}

fn cursor_key(mob_id: &str) -> String {
    format!("{CURSOR_PREFIX}{mob_id}")
}

/// Injective over (mob id, member id): the mob id's length comes first.
fn idle_key(mob_id: &str, member_id: &str) -> String {
    format!("{IDLE_PREFIX}{}:{mob_id}{member_id}", mob_id.len())
}

fn lock<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Why a document could not be read or written.
#[derive(Debug, Clone)]
pub(crate) enum MetaDocumentError {
    /// The realm refused or failed the call: a poisoned scope or realm, a
    /// lapsed lease, a host store failure.
    Store { key: String, error: SpiError },
    /// The document's bytes are not what this build writes, or a value
    /// could not be encoded.
    Codec { key: String, detail: String },
}

impl MetaDocumentError {
    /// The realm's own refusal, when the store refused.
    #[cfg(test)]
    pub(crate) fn spi(&self) -> Option<&SpiError> {
        match self {
            Self::Store { error, .. } => Some(error),
            Self::Codec { .. } => None,
        }
    }
}

impl std::fmt::Display for MetaDocumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store { key, error } => write!(f, "host store document {key}: {error}"),
            Self::Codec { key, detail } => write!(f, "host store document {key}: {detail}"),
        }
    }
}

impl std::error::Error for MetaDocumentError {}

/// What [`MetaDocuments::update`] does to a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DocumentChange {
    /// Leave it as it is.
    Keep,
    /// Replace it with these bytes (create it when absent).
    Replace(Vec<u8>),
    /// Remove it.
    Remove,
}

/// The realm's one claim of [`META_DOCUMENT_FAMILY`], kept by the realm
/// ([`SpiRealm::extension`]).
struct MetaFamilyClaim;

/// Meta's documents on one host store realm. Cheap to clone; every clone
/// commits through the same realm handle.
#[derive(Clone)]
pub(crate) struct MetaDocuments {
    realm: Arc<SpiRealm>,
    /// The storage provider's name, which locations and refusals carry.
    provider: Arc<str>,
}

impl std::fmt::Debug for MetaDocuments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetaDocuments")
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

/// Two handles are equal when they reach the same realm handle.
impl PartialEq for MetaDocuments {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.realm, &other.realm) && self.provider == other.provider
    }
}

impl Eq for MetaDocuments {}

impl MetaDocuments {
    /// The documents of `realm`, the family claimed once per realm before
    /// any of its scopes is touched.
    ///
    /// # Errors
    /// A poisoned realm, or listed heads that claim one document twice.
    pub(crate) async fn open(realm: Arc<SpiRealm>, provider: &str) -> Result<Self, SpiError> {
        realm.check_realm()?;
        let claiming = Arc::clone(&realm);
        realm
            .extension(move || async move {
                claiming
                    .claim(META_DOCUMENT_FAMILY)
                    .map(|_| MetaFamilyClaim)
            })
            .await?;
        Ok(Self {
            realm,
            provider: Arc::from(provider),
        })
    }

    /// The location a refusal names a document by: `host-store:<provider>:<key>`.
    pub(crate) fn label(&self, key: &str) -> PathBuf {
        PathBuf::from(format!("host-store:{}:{key}", self.provider))
    }

    fn store_error(key: &str) -> impl Fn(SpiError) -> MetaDocumentError + '_ {
        move |error| MetaDocumentError::Store {
            key: key.to_string(),
            error,
        }
    }

    /// The document `key`, `None` when the realm holds none.
    pub(crate) async fn read(&self, key: &str) -> Result<Option<Arc<Vec<u8>>>, MetaDocumentError> {
        let id = ScopeId::new(META_DOCUMENT_FAMILY, key);
        let mut guards = self.realm.lock_scopes(std::slice::from_ref(&id)).await;
        let Some(state) = guards.pop() else {
            return Err(MetaDocumentError::Store {
                key: key.to_string(),
                error: SpiError::Misuse("a locked document scope is missing".to_string()),
            });
        };
        self.realm
            .check_scope(&state)
            .map_err(Self::store_error(key))?;
        if !state.exists() {
            return Ok(None);
        }
        let mut pinned = self
            .realm
            .pinned_values(&state)
            .await
            .map_err(Self::store_error(key))?;
        Ok(pinned.remove(DOCUMENT_ENTRY))
    }

    /// Replace the document `key` (`None` removes it): one batch, settled
    /// before this answers.
    pub(crate) async fn write(
        &self,
        key: &str,
        document: Option<Vec<u8>>,
    ) -> Result<(), MetaDocumentError> {
        let change = match document {
            Some(document) => DocumentChange::Replace(document),
            None => DocumentChange::Remove,
        };
        self.update(key, |_| Ok((change, ()))).await
    }

    /// Read the document `key` and decide, under its scope's lock, what it
    /// becomes: `decide` answers the change and the caller's answer. A
    /// change is one batch, settled before this answers, and no other write
    /// of the document lands between the read and it.
    pub(crate) async fn update<T>(
        &self,
        key: &str,
        decide: impl FnOnce(Option<&[u8]>) -> Result<(DocumentChange, T), MetaDocumentError>,
    ) -> Result<T, MetaDocumentError> {
        // A writer past its self-fence window changes nothing.
        self.realm
            .ensure_lease()
            .await
            .map_err(Self::store_error(key))?;
        let id = ScopeId::new(META_DOCUMENT_FAMILY, key);
        let guards = self.realm.lock_scopes(std::slice::from_ref(&id)).await;
        let mut scopes = ArmedScopes::new(Arc::clone(&self.realm), guards);
        let current = {
            let Some(state) = scopes.guards().first() else {
                return Err(MetaDocumentError::Store {
                    key: key.to_string(),
                    error: SpiError::Misuse("a locked document scope is missing".to_string()),
                });
            };
            self.realm
                .check_scope(state)
                .map_err(Self::store_error(key))?;
            if state.exists() {
                self.realm
                    .pinned_values(state)
                    .await
                    .map_err(Self::store_error(key))?
                    .remove(DOCUMENT_ENTRY)
            } else {
                None
            }
        };
        let (change, answer) = decide(current.as_deref().map(Vec::as_slice))?;
        let replacement = match change {
            DocumentChange::Keep => return Ok(answer),
            DocumentChange::Replace(document) => Some(Arc::new(document)),
            DocumentChange::Remove => None,
        };
        // The lock and the read may have outlasted the window.
        self.realm
            .ensure_lease()
            .await
            .map_err(Self::store_error(key))?;
        scopes.arm();
        let delta = ScopeDelta::Changes(vec![(DOCUMENT_ENTRY.to_string(), replacement)]);
        // On a task that owns the scope: a caller dropped meanwhile still
        // lands the batch and adopts it, or poisons the scope.
        scopes
            .commit_detached(vec![(0, delta)])
            .await
            .map_err(Self::store_error(key))?;
        Ok(answer)
    }

    /// The keys under `prefix` this realm knows: listed at boot or written
    /// since. A key may name a document removed since; [`Self::read`]
    /// answers `None` for it.
    pub(crate) fn keys_with_prefix(&self, prefix: &str) -> Vec<String> {
        self.realm
            .ids_of(META_DOCUMENT_FAMILY)
            .into_iter()
            .map(|id| id.key)
            .filter(|key| key.starts_with(prefix))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// The realm record
// ---------------------------------------------------------------------------

/// The realm record's `format`.
pub(crate) const REALM_RECORD_FORMAT: &str = "meerkat-meta.host-store-realm";

/// The realm record format version this build reads and writes.
pub(crate) const REALM_RECORD_VERSION: u32 = 1;

/// What a host store realm records about itself: the meerkat-level realm
/// its stores are keyed by (it takes the place of a state directory's realm
/// manifest), the release that created it, and an import of a continuity
/// export when one was made into it.
///
/// ```json
/// {"format":"meerkat-meta.host-store-realm","version":1,
///  "meerkat_level_realm":"meta","created_by_meta":"0.9.6","import":null}
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HostStoreRealmRecord {
    pub(crate) format: String,
    pub(crate) version: u32,
    /// The meerkat-level realm id the realm's stores are keyed by.
    pub(crate) meerkat_level_realm: String,
    /// The Meta release that wrote the record first.
    pub(crate) created_by_meta: String,
    /// The import of a continuity export into the realm, when one ran.
    #[serde(default)]
    pub(crate) import: Option<HostStoreImportRecord>,
}

impl HostStoreRealmRecord {
    /// A new realm's record.
    pub(crate) fn new(meerkat_level_realm: &str, import: Option<HostStoreImportRecord>) -> Self {
        Self {
            format: REALM_RECORD_FORMAT.to_string(),
            version: REALM_RECORD_VERSION,
            meerkat_level_realm: meerkat_level_realm.to_string(),
            created_by_meta: env!("CARGO_PKG_VERSION").to_string(),
            import,
        }
    }

    /// The import that has not finished, if one has not.
    pub(crate) fn unfinished_import(&self) -> Option<&HostStoreImportRecord> {
        self.import
            .as_ref()
            .filter(|import| import.state == HostStoreImportState::InProgress)
    }
}

/// An import of a continuity export into a host store realm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HostStoreImportRecord {
    /// The export's `entries_sha256`: a rerun with another file refuses.
    pub(crate) entries_sha256: String,
    pub(crate) state: HostStoreImportState,
    /// The sessions the export carries into the realm.
    pub(crate) sessions: u64,
    /// The digest of the first-boot rows the import writes.
    pub(crate) rows_sha256: String,
}

/// Whether an import finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HostStoreImportState {
    /// Started and not finished: no runtime opens the realm.
    InProgress,
    /// Every session and the first-boot rows are in the realm.
    Done,
}

/// Why the realm record could not be read.
#[derive(Debug, Clone)]
pub(crate) enum RealmRecordError {
    /// A newer Meta wrote the record, in a format version this build does not
    /// read.
    Newer { found: u32 },
    /// The record is not a realm record this build reads.
    Malformed(String),
    /// The realm refused or failed the read.
    Store(MetaDocumentError),
}

impl std::fmt::Display for RealmRecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Newer { found } => write!(
                f,
                "the host store realm record is format version {found}; this build reads version \
                 {REALM_RECORD_VERSION}"
            ),
            Self::Malformed(detail) => {
                write!(f, "the host store realm record is malformed: {detail}")
            }
            Self::Store(error) => write!(f, "{error}"),
        }
    }
}

/// Decode a realm record, its format and version first, so a newer record
/// refuses as newer rather than as malformed.
pub(crate) fn decode_realm_record(bytes: &[u8]) -> Result<HostStoreRealmRecord, RealmRecordError> {
    #[derive(Deserialize)]
    struct Probe {
        format: Option<serde_json::Value>,
        version: Option<serde_json::Value>,
    }
    let probe: Probe = serde_json::from_slice(bytes)
        .map_err(|error| RealmRecordError::Malformed(error.to_string()))?;
    match probe.format.as_ref().and_then(serde_json::Value::as_str) {
        Some(REALM_RECORD_FORMAT) => {}
        other => {
            return Err(RealmRecordError::Malformed(format!(
                "its format is {other:?}, not {REALM_RECORD_FORMAT:?}"
            )));
        }
    }
    match probe.version.as_ref().and_then(serde_json::Value::as_u64) {
        Some(version) if version == u64::from(REALM_RECORD_VERSION) => {}
        Some(version) if version > u64::from(REALM_RECORD_VERSION) => {
            return Err(RealmRecordError::Newer {
                found: u32::try_from(version).unwrap_or(u32::MAX),
            });
        }
        other => {
            return Err(RealmRecordError::Malformed(format!(
                "its version is {other:?}"
            )));
        }
    }
    serde_json::from_slice(bytes).map_err(|error| RealmRecordError::Malformed(error.to_string()))
}

/// The realm record, `None` for a realm no build or import wrote yet.
pub(crate) async fn read_realm_record(
    documents: &MetaDocuments,
) -> Result<Option<HostStoreRealmRecord>, RealmRecordError> {
    let Some(bytes) = documents
        .read(REALM_RECORD_KEY)
        .await
        .map_err(RealmRecordError::Store)?
    else {
        return Ok(None);
    };
    decode_realm_record(&bytes).map(Some)
}

/// Encode a realm record as the realm keeps it.
pub(crate) fn encode_realm_record(
    record: &HostStoreRealmRecord,
) -> Result<Vec<u8>, MetaDocumentError> {
    serde_json::to_vec(record).map_err(|error| MetaDocumentError::Codec {
        key: REALM_RECORD_KEY.to_string(),
        detail: error.to_string(),
    })
}

// ---------------------------------------------------------------------------
// The persistent metadata store
// ---------------------------------------------------------------------------

/// The structural-events subscription cursor as a document.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCursor {
    v: u32,
    cursor: u64,
}

/// One member's idle-retirement opt-in as a document. The key only names
/// it; the document carries the whole record.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredIdleOptIn {
    v: u32,
    mob_id: String,
    member_id: String,
    session_id: SessionId,
    policy: DelegateIdleRetireOverride,
    recorded_at: chrono::DateTime<chrono::Utc>,
}

/// The cursor writes of one mob: the newest value waits while one write is
/// in flight.
#[derive(Debug, Default)]
struct CursorSlot {
    /// The newest cursor this process set.
    latest: Option<u64>,
    /// The newest cursor no write has taken yet.
    pending: Option<u64>,
    /// Whether a write is in flight.
    writing: bool,
    /// The last write that failed, reported to the next caller once.
    failure: Option<String>,
}

type CursorSlots = Arc<StdMutex<BTreeMap<String, CursorSlot>>>;

/// The persistent metadata store of a host store realm: the
/// structural-events subscription cursors and the members' idle-retirement
/// opt-ins, as documents of the realm, so they are as durable as the mob
/// ledger the cursor checkpoints and a restart on another host skips no
/// event.
///
/// Cursor writes are coalesced. A subscription checkpoints after every
/// event, and on a host store each document write is one host batch, so the
/// newest cursor wins: at most one write per mob is in flight, and when it
/// lands the newest cursor set meanwhile is written next. A crash replays
/// the events after the last cursor that landed, which the subscription
/// already treats as safe; none is skipped.
pub(crate) struct SpiMetadataStore {
    documents: MetaDocuments,
    cursors: CursorSlots,
}

impl SpiMetadataStore {
    pub(crate) fn new(documents: MetaDocuments) -> Self {
        Self {
            documents,
            cursors: Arc::default(),
        }
    }

    /// Wait until no cursor write of `mob_id` is in flight or waiting, for at
    /// most half a minute.
    #[cfg(test)]
    pub(crate) async fn cursor_writes_settled(&self, mob_id: &str) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let busy = lock(&self.cursors)
                .get(mob_id)
                .is_some_and(|slot| slot.writing || slot.pending.is_some());
            if !busy {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the cursor writes of {mob_id} did not settle within half a minute"
            );
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    }
}

fn io_error(error: MetaDocumentError) -> MetadataStoreError {
    MetadataStoreError::Io(error.to_string())
}

fn decode_error(key: &str, error: impl std::fmt::Display) -> MetadataStoreError {
    MetadataStoreError::Decode(format!("host store document {key}: {error}"))
}

/// Write the newest cursor of `mob_id` until none waits. Runs on a task of
/// its own, one per mob at a time.
async fn write_cursors(documents: MetaDocuments, cursors: CursorSlots, mob_id: String) {
    let key = cursor_key(&mob_id);
    loop {
        let next = {
            let mut slots = lock(&cursors);
            let Some(slot) = slots.get_mut(&mob_id) else {
                return;
            };
            match slot.pending.take() {
                Some(cursor) => cursor,
                None => {
                    slot.writing = false;
                    return;
                }
            }
        };
        let written = match serde_json::to_vec(&StoredCursor {
            v: ENTRY_VERSION,
            cursor: next,
        }) {
            Ok(document) => documents
                .write(&key, Some(document))
                .await
                .map_err(|error| format!("{error}")),
            Err(error) => Err(format!("encode the cursor: {error}")),
        };
        if let Err(failure) = written {
            tracing::warn!(
                mob_id = %mob_id,
                cursor = next,
                error = %failure,
                "mob_events subscription: the host store did not take the cursor; a restart \
                 replays from the last cursor that landed"
            );
            if let Some(slot) = lock(&cursors).get_mut(&mob_id) {
                slot.failure = Some(failure);
            }
        }
    }
}

#[async_trait]
impl PersistentMetadataStore for SpiMetadataStore {
    async fn get_subscription_cursor(
        &self,
        mob_id: &str,
    ) -> Result<Option<u64>, MetadataStoreError> {
        if let Some(latest) = lock(&self.cursors).get(mob_id).and_then(|slot| slot.latest) {
            return Ok(Some(latest));
        }
        let key = cursor_key(mob_id);
        let Some(bytes) = self.documents.read(&key).await.map_err(io_error)? else {
            return Ok(None);
        };
        let stored: StoredCursor =
            serde_json::from_slice(&bytes).map_err(|error| decode_error(&key, error))?;
        if stored.v != ENTRY_VERSION {
            return Err(decode_error(&key, format!("version {}", stored.v)));
        }
        Ok(Some(stored.cursor))
    }

    /// Coalesced: answers once the cursor waits for its write (see the type
    /// documentation). A write that failed is reported to the next call.
    async fn set_subscription_cursor(
        &self,
        mob_id: &str,
        cursor: u64,
    ) -> Result<(), MetadataStoreError> {
        let (start, failure) = {
            let mut slots = lock(&self.cursors);
            let slot = slots.entry(mob_id.to_string()).or_default();
            slot.latest = Some(cursor);
            slot.pending = Some(cursor);
            let start = !slot.writing;
            slot.writing = true;
            (start, slot.failure.take())
        };
        if start {
            tokio::spawn(write_cursors(
                self.documents.clone(),
                Arc::clone(&self.cursors),
                mob_id.to_string(),
            ));
        }
        match failure {
            Some(failure) => Err(MetadataStoreError::Io(failure)),
            None => Ok(()),
        }
    }

    async fn load_member_idle_retire_overrides(
        &self,
    ) -> Result<Vec<MemberIdleRetireOverrideRecord>, MetadataStoreError> {
        let mut records = Vec::new();
        for key in self.documents.keys_with_prefix(IDLE_PREFIX) {
            let Some(bytes) = self.documents.read(&key).await.map_err(io_error)? else {
                continue;
            };
            let stored: StoredIdleOptIn =
                serde_json::from_slice(&bytes).map_err(|error| decode_error(&key, error))?;
            if stored.v != ENTRY_VERSION {
                return Err(decode_error(&key, format!("version {}", stored.v)));
            }
            records.push(MemberIdleRetireOverrideRecord {
                mob_id: stored.mob_id,
                member_id: stored.member_id,
                session_id: stored.session_id,
                policy: stored.policy,
                recorded_at: stored.recorded_at,
            });
        }
        Ok(records)
    }

    async fn set_member_idle_retire_override(
        &self,
        record: &MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        let key = idle_key(&record.mob_id, &record.member_id);
        let document = serde_json::to_vec(&StoredIdleOptIn {
            v: ENTRY_VERSION,
            mob_id: record.mob_id.clone(),
            member_id: record.member_id.clone(),
            session_id: record.session_id.clone(),
            policy: record.policy,
            recorded_at: record.recorded_at,
        })
        .map_err(|error| MetadataStoreError::Io(format!("encode idle-retire opt-in: {error}")))?;
        self.documents
            .write(&key, Some(document))
            .await
            .map_err(io_error)
    }

    /// Removed only while it is still bound to `record.session_id`, decided
    /// under the document's lock.
    async fn clear_member_idle_retire_override(
        &self,
        record: &MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        #[derive(Deserialize)]
        struct StoredSession {
            session_id: SessionId,
        }
        let key = idle_key(&record.mob_id, &record.member_id);
        self.documents
            .update(&key, |current| {
                let Some(bytes) = current else {
                    return Ok((DocumentChange::Keep, ()));
                };
                let stored: StoredSession =
                    serde_json::from_slice(bytes).map_err(|error| MetaDocumentError::Codec {
                        key: key.clone(),
                        detail: error.to_string(),
                    })?;
                let change = if stored.session_id == record.session_id {
                    DocumentChange::Remove
                } else {
                    DocumentChange::Keep
                };
                Ok((change, ()))
            })
            .await
            .map_err(io_error)
    }
}
