//! A live copy of each durable identity's current continuity record and
//! committed session, in the JSON Lines shape the continuity export's
//! `jsonl_provider` reads (`tools/continuity-export`).
//!
//! It is a restore source for a lost state directory and is never read at
//! boot. Restore runs the latest rows through `continuity_export.py --provider
//! jsonl_provider:from_env` and `meta_gateway storage-migrate-native` into a
//! fresh state directory.
//!
//! The runtime stays the only authority, and owns the mirror when the builder
//! installs one ([`crate::UnifiedRuntimeBuilder::continuity_mirror`]).
//! - The mirror reads Core's durable member bindings and each bound session
//!   through the runtime's non-owning bridge and its session service.
//! - It keeps, per identity, the record, the session's resume authority and
//!   the document's digest, never the document.
//! - A round re-reads a session only when its authority changed, and hands
//!   the sink the records plus the changed documents.
//! - A failure on one identity is counted and logged, and that identity keeps
//!   its last rows; the others go on.
//! - A put that fails, panics or outlives its deadline is counted, and the
//!   next round retries.
//! - Shutdown writes a last round within a deadline; dropping the runtime
//!   aborts the mirror without one.
//!
//! A session holds each image as a reference into the state directory's blob
//! store, so the mirror also copies every blob a changed session references,
//! once, and before the session's row can go out; a blob it cannot copy holds
//! back only the sessions that reference it. Restore writes those blobs into
//! the new state directory's blob store with
//! `tools/continuity-export/restore_blobs.py`, which checks each id against
//! its bytes.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use base64::Engine as _;
use futures::FutureExt as _;
use meerkat_core::{BlobId, BlobStoreError};
use meerkat_mob::{MobSessionService, ResumeSessionLoad, SessionResumeAuthority};
use sha2::{Digest as _, Sha256};

use crate::blob_store::BinaryBlobStore;
use crate::identity_first::bridge::SessionBridge;

/// The blobs file in a [`JsonlFileMirrorSink`]'s directory.
pub const CONTINUITY_MIRROR_BLOBS_FILE: &str = "blobs.jsonl";

/// How often the mirror runs a round, and how long a put and the last round
/// at shutdown may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContinuityMirrorTimings {
    pub interval: Duration,
    /// Past it a put counts as failed, and the next round offers the same rows
    /// again. Size it for the sink: the first put after a start carries every
    /// session. A put that outlives it never makes a turn wait, it only delays
    /// the next round.
    pub put_deadline: Duration,
    /// Past it shutdown aborts the last round and goes on.
    pub stop_deadline: Duration,
}

impl Default for ContinuityMirrorTimings {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            put_deadline: Duration::from_mins(10),
            stop_deadline: Duration::from_secs(30),
        }
    }
}

/// One identity's row in the records file: the five fields of the export's
/// record. The import reads `identity` and `session_id`; it reads the other
/// three only for its report, so they carry the binding's generation, a
/// runtime id derived from it, and checkpoint version 0.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContinuityMirrorRecord {
    pub identity: String,
    pub agent_runtime_id: String,
    pub session_id: String,
    pub generation: u64,
    pub checkpoint_version: u64,
}

/// One row in the sessions file: the session id and the persisted session
/// document's exact bytes in standard base64.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContinuityMirrorSession {
    pub session_id: String,
    pub data: String,
}

/// One row in the blobs file: a blob a session references, the media type
/// the blob store keeps beside it, and its bytes in standard base64. The id
/// is the store's address for those bytes: `sha256(media_type || 0x00 ||
/// base64 text)` for an image, or `sha256(media_type || 0x00 || bytes)` for a
/// blob under Meta's own address. The restore checks each row against both.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContinuityMirrorBlob {
    pub blob_id: String,
    pub media_type: String,
    pub data: String,
}

/// What one put hands the sink.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContinuityMirrorUpdate {
    /// Strictly increasing across puts and across restarts of the runtime:
    /// wall-clock milliseconds, kept above the previous put's value.
    pub sequence: u64,
    /// Every mirrored identity's record, sorted by identity.
    pub records: Vec<ContinuityMirrorRecord>,
    /// The sessions whose document changed since the last accepted put, sorted
    /// by id; every session on the first put after a start.
    pub changed_sessions: Vec<ContinuityMirrorSession>,
    /// Every session a record names. A sink keeps no other session.
    pub session_ids: BTreeSet<String>,
}

/// Where the mirror writes.
///
/// A put carries every record and only the sessions that changed. A sink that
/// replaces whole files (like [`JsonlFileMirrorSink`]) can ignore the
/// sequence. An append-only sink writes the update's `sequence` on every row
/// and a completion row for that sequence last; a restore then takes the
/// records of the latest COMPLETE sequence and, for each session those records
/// name, its latest row from a complete sequence at or before it. Records of
/// an identity the mirror no longer copies are not in that latest sequence, so
/// a restore does not bring them back.
///
/// Blobs go through [`Self::put_blob`]: the mirror puts every blob a changed
/// session references before the first put that carries that session, and
/// each id once while it runs. Blobs are content addressed: a sink that
/// already holds the id keeps the copy it has, and it never drops a blob, since
/// a session row a restore may still take can reference it.
#[async_trait::async_trait]
pub trait ContinuityMirrorSink: Send + Sync {
    async fn put(&self, update: &ContinuityMirrorUpdate) -> Result<(), String>;
    async fn put_blob(&self, blob: &ContinuityMirrorBlob) -> Result<(), String>;
}

/// Writes `records.jsonl` and `sessions.jsonl` into a directory. Each file is
/// written to a `.partial` file, synced and renamed, then the directory is
/// synced. The sessions file first takes the union of its old rows and the
/// changed ones, then the records file is replaced, then the sessions file is
/// trimmed to the sessions the records name: a reader never finds a record
/// whose session is missing, even after a crash between the steps.
///
/// Each blob is a line appended to `blobs.jsonl` and synced before `put_blob`
/// returns; an id the file already holds is not appended again.
pub struct JsonlFileMirrorSink {
    dir: PathBuf,
    /// One writer at a time: a put whose future passed its deadline keeps
    /// writing on its blocking thread, and the next put waits for it.
    writing: Arc<std::sync::Mutex<()>>,
    /// The ids `blobs.jsonl` holds: read from the file on the first blob put,
    /// and again after an append fails. Its lock keeps one blob writer at a
    /// time.
    blob_ids: Arc<std::sync::Mutex<Option<HashSet<String>>>>,
}

impl JsonlFileMirrorSink {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            writing: Arc::new(std::sync::Mutex::new(())),
            blob_ids: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn put_blocking(&self, update: &ContinuityMirrorUpdate) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| format!("{}: {error}", self.dir.display()))?;
        let mut sessions = read_sessions(&self.dir.join("sessions.jsonl"))?;
        for session in &update.changed_sessions {
            sessions.insert(session.session_id.clone(), session.clone());
        }
        write_jsonl(&self.dir, "sessions.jsonl", sessions.values())?;
        write_jsonl(&self.dir, "records.jsonl", update.records.iter())?;
        sessions.retain(|session_id, _| update.session_ids.contains(session_id));
        write_jsonl(&self.dir, "sessions.jsonl", sessions.values())
    }
}

#[async_trait::async_trait]
impl ContinuityMirrorSink for JsonlFileMirrorSink {
    async fn put(&self, update: &ContinuityMirrorUpdate) -> Result<(), String> {
        let sink = Self {
            dir: self.dir.clone(),
            writing: Arc::clone(&self.writing),
            blob_ids: Arc::clone(&self.blob_ids),
        };
        let update = update.clone();
        tokio::task::spawn_blocking(move || {
            let _writing = sink
                .writing
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            sink.put_blocking(&update)
        })
        .await
        .map_err(|error| format!("the file sink's writer ended: {error}"))?
    }

    async fn put_blob(&self, blob: &ContinuityMirrorBlob) -> Result<(), String> {
        let dir = self.dir.clone();
        let blob_ids = Arc::clone(&self.blob_ids);
        let blob = blob.clone();
        tokio::task::spawn_blocking(move || {
            let mut held = blob_ids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            put_blob_blocking(&dir, &mut held, &blob)
        })
        .await
        .map_err(|error| format!("the file sink's blob writer ended: {error}"))?
    }
}

fn put_blob_blocking(
    dir: &Path,
    held: &mut Option<HashSet<String>>,
    blob: &ContinuityMirrorBlob,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let path = dir.join(CONTINUITY_MIRROR_BLOBS_FILE);
    let ids = match held.take() {
        Some(ids) => ids,
        None => read_blob_ids(&path)?,
    };
    let ids = held.insert(ids);
    if ids.contains(&blob.blob_id) {
        return Ok(());
    }
    let mut line = serde_json::to_string(blob).map_err(|error| error.to_string())?;
    line.push('\n');
    if let Err(error) = append_line(&path, &line) {
        // A failed append can leave part of the line behind; reading the file
        // again cuts it off before the next append.
        *held = None;
        return Err(error);
    }
    ids.insert(blob.blob_id.clone());
    Ok(())
}

/// The ids the blobs file at `path` holds. A last line without its newline is
/// an append that never finished: it is cut off, so the file stays whole
/// lines and the next append starts a line of its own.
fn read_blob_ids(path: &Path) -> Result<HashSet<String>, String> {
    use std::io::BufRead as _;
    #[derive(serde::Deserialize)]
    struct Row {
        blob_id: String,
    }
    let failed = |error: std::io::Error| format!("{}: {error}", path.display());
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(error) => return Err(failed(error)),
    };
    let mut reader = std::io::BufReader::new(file);
    let mut ids = HashSet::new();
    let mut line = Vec::new();
    let mut whole_lines_end: u64 = 0;
    let mut number = 0_usize;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line).map_err(failed)?;
        if read == 0 {
            return Ok(ids);
        }
        if line.last() != Some(&b'\n') {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .map_err(failed)?;
            file.set_len(whole_lines_end).map_err(failed)?;
            file.sync_data().map_err(failed)?;
            return Ok(ids);
        }
        number += 1;
        whole_lines_end += line.len() as u64;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let row: Row = serde_json::from_slice(&line)
            .map_err(|error| format!("{}:{number}: {error}", path.display()))?;
        ids.insert(row.blob_id);
    }
}

/// Append `line` to the file at `path` and sync it, and the directory too
/// when the append created the file.
fn append_line(path: &Path, line: &str) -> Result<(), String> {
    use std::io::Write as _;
    let failed = |error: std::io::Error| format!("{}: {error}", path.display());
    let created = !path.exists();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(failed)?;
    file.write_all(line.as_bytes()).map_err(failed)?;
    file.sync_data().map_err(failed)?;
    if created && let Some(dir) = path.parent() {
        std::fs::File::open(dir)
            .and_then(|dir| dir.sync_all())
            .map_err(failed)?;
    }
    Ok(())
}

/// Add to `ids` every blob id the serialized session `document` references.
/// Only a document that names a blob is parsed back, with serde_json's own
/// depth limit, as the restore's import parses it.
fn collect_document_blob_ids(
    document: &[u8],
    ids: &mut BTreeSet<String>,
) -> Result<(), serde_json::Error> {
    if mentions_blob_id(document) {
        let parsed: serde_json::Value = serde_json::from_slice(document)?;
        collect_blob_ids(&parsed, ids);
    }
    Ok(())
}

/// Whether a serialized document has a `blob_id` key anywhere: serde_json
/// writes every such key as these exact bytes.
fn mentions_blob_id(document: &[u8]) -> bool {
    const KEY: &[u8] = b"\"blob_id\"";
    document.windows(KEY.len()).any(|window| window == KEY)
}

/// Add to `ids` every blob id `document` references: the value of each
/// `blob_id` key, at any depth, that is a canonical `sha256:<64 hex>` id. That
/// covers images in the transcript and in its history alike, and the realtime
/// content records in the session's metadata.
fn collect_blob_ids(document: &serde_json::Value, ids: &mut BTreeSet<String>) {
    match document {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                match value {
                    serde_json::Value::String(id)
                        if key == "blob_id" && crate::blob_store::is_valid_blob_id_value(id) =>
                    {
                        ids.insert(id.clone());
                    }
                    _ => collect_blob_ids(value, ids),
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_blob_ids(item, ids);
            }
        }
        _ => {}
    }
}

/// Read `blob_id` from `store` as a blobs-file row. `None` when the store
/// holds no copy, or none whose bytes match the id under either of the
/// store's addresses: there is nothing to copy, and a restored session finds
/// that image missing, as the runtime does now.
pub async fn read_mirror_blob(
    store: &dyn BinaryBlobStore,
    blob_id: &str,
) -> Result<Option<ContinuityMirrorBlob>, String> {
    let payload = match store.get_bytes(&BlobId::new(blob_id)).await {
        Ok(payload) => payload,
        Err(BlobStoreError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(format!("blob {blob_id}: {error}")),
    };
    let blob = ContinuityMirrorBlob {
        blob_id: blob_id.to_string(),
        media_type: payload.media_type,
        data: base64::engine::general_purpose::STANDARD.encode(&payload.data),
    };
    let address = |content: &[u8]| {
        let mut hasher = Sha256::new();
        hasher.update(blob.media_type.as_bytes());
        hasher.update([0]);
        hasher.update(content);
        format!("sha256:{:x}", hasher.finalize())
    };
    if blob_id == address(blob.data.as_bytes()) || blob_id == address(payload.data.as_ref()) {
        Ok(Some(blob))
    } else {
        Ok(None)
    }
}

fn read_sessions(path: &Path) -> Result<BTreeMap<String, ContinuityMirrorSession>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let mut sessions = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let session: ContinuityMirrorSession = serde_json::from_str(line)
            .map_err(|error| format!("{} line {}: {error}", path.display(), index + 1))?;
        sessions.insert(session.session_id.clone(), session);
    }
    Ok(sessions)
}

fn write_jsonl<'a, T: serde::Serialize + 'a>(
    dir: &Path,
    name: &str,
    rows: impl Iterator<Item = &'a T>,
) -> Result<(), String> {
    use std::io::Write as _;
    let partial = dir.join(format!(".{name}.partial"));
    let target = dir.join(name);
    let fail = |path: &Path, error: std::io::Error| format!("{}: {error}", path.display());
    let mut file = std::fs::File::create(&partial).map_err(|error| fail(&partial, error))?;
    for row in rows {
        let line = serde_json::to_string(row).map_err(|error| error.to_string())?;
        file.write_all(line.as_bytes())
            .and_then(|()| file.write_all(b"\n"))
            .map_err(|error| fail(&partial, error))?;
    }
    file.sync_all().map_err(|error| fail(&partial, error))?;
    drop(file);
    std::fs::rename(&partial, &target).map_err(|error| fail(&target, error))?;
    std::fs::File::open(dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| fail(dir, error))
}

/// What the mirror reads from: the runtime's non-owning session bridge, its
/// session service and its blob store. Holding them keeps nothing of the
/// runtime alive.
#[derive(Clone)]
pub struct ContinuityMirrorSource {
    bridge: Arc<dyn SessionBridge>,
    service: Arc<dyn MobSessionService>,
    /// Holds the images the sessions reference; `None` when the runtime has
    /// no blob store.
    blobs: Option<Arc<dyn BinaryBlobStore>>,
}

/// One mirrored identity, as the mirror keeps it between rounds.
#[derive(Debug, Clone)]
struct Mirrored {
    record: ContinuityMirrorRecord,
    authority: SessionResumeAuthority,
    document_sha256: [u8; 32],
}

/// One identity's outcome in a round.
enum IdentityRead {
    /// The session is unchanged since the last round.
    Unchanged,
    /// A new or changed session, with its row and the ids of the blobs its
    /// document references.
    Changed(Box<Mirrored>, ContinuityMirrorSession, BTreeSet<String>),
    /// No durable session to copy (`Absent`), or an archived one the lifecycle
    /// cannot revive (`ArchivedNotRevivable`): the identity is not mirrored.
    NotMirrored,
    /// The binding moved while the body was read: keep the last rows.
    Moved,
}

impl ContinuityMirrorSource {
    pub(crate) fn new(
        bridge: Arc<dyn SessionBridge>,
        service: Arc<dyn MobSessionService>,
        blobs: Option<Arc<dyn BinaryBlobStore>>,
    ) -> Self {
        Self {
            bridge,
            service,
            blobs,
        }
    }
}

/// One durable identity's latest binding that names a session.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MirrorBinding {
    identity: String,
    session_id: meerkat_core::types::SessionId,
    generation: u64,
}

/// The reads a round makes. [`ContinuityMirrorSource`] answers them from the
/// runtime; the unit rows answer them from a fake.
#[async_trait::async_trait]
trait MirrorRead: BlobRead {
    /// Each durable identity whose latest binding names a session.
    async fn bindings(&self) -> Result<Vec<MirrorBinding>, String>;
    /// The session's resume authority, without its body.
    async fn authority(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<SessionResumeAuthority, String>;
    /// The session's body, read only.
    async fn load(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<ResumeSessionLoad, String>;
    /// The identity's binding now: its session and generation.
    async fn current_binding(
        &self,
        identity: &str,
    ) -> Result<Option<(meerkat_core::types::SessionId, u64)>, String>;
}

#[async_trait::async_trait]
impl MirrorRead for ContinuityMirrorSource {
    async fn bindings(&self) -> Result<Vec<MirrorBinding>, String> {
        let history = self
            .bridge
            .member_binding_history()
            .await
            .map_err(|error| format!("member binding history: {error}"))?;
        Ok(history
            .durable_identity_bindings()
            .filter_map(|binding| {
                binding.session_id().map(|session_id| MirrorBinding {
                    identity: crate::member_comms_id::durable_identity_for_roster_member(
                        binding.identity().as_ref(),
                    ),
                    session_id: session_id.clone(),
                    generation: binding.generation().get(),
                })
            })
            .collect())
    }

    async fn authority(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<SessionResumeAuthority, String> {
        self.service
            .observe_session_resume_authority(session_id)
            .await
            .map_err(|error| format!("session {session_id}: {error}"))
    }

    async fn load(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<ResumeSessionLoad, String> {
        self.service
            .load_session_for_resume(session_id)
            .await
            .map_err(|error| format!("session {session_id}: {error}"))
    }

    async fn current_binding(
        &self,
        identity: &str,
    ) -> Result<Option<(meerkat_core::types::SessionId, u64)>, String> {
        let durable_identity = crate::identity_first::AgentIdentity::parse(identity)
            .map_err(|error| format!("identity {identity}: {error}"))?;
        let current = self
            .bridge
            .last_member_binding(&durable_identity)
            .await
            .map_err(|error| format!("member binding: {error}"))?;
        Ok(current.and_then(|binding| {
            binding
                .session_id()
                .map(|session_id| (session_id.clone(), binding.generation().get()))
        }))
    }
}

async fn read_identity(
    source: &dyn MirrorRead,
    binding: &MirrorBinding,
    previous: Option<&Mirrored>,
) -> Result<IdentityRead, String> {
    let identity = binding.identity.as_str();
    let session_id = &binding.session_id;
    let generation = binding.generation;
    let authority = source.authority(session_id).await?;
    // A service that keeps no store authority (an ephemeral one) answers
    // the empty bundle: then only the document digest below can tell.
    let same_binding = previous.is_some_and(|previous| {
        previous.record.session_id == session_id.to_string()
            && previous.record.generation == generation
    });
    if same_binding
        && authority != SessionResumeAuthority::default()
        && previous.is_some_and(|previous| previous.authority == authority)
    {
        return Ok(IdentityRead::Unchanged);
    }
    let session = match source.load(session_id).await? {
        ResumeSessionLoad::Active(session) | ResumeSessionLoad::Revivable(session) => session,
        ResumeSessionLoad::ArchivedNotRevivable { .. } | ResumeSessionLoad::Absent => {
            return Ok(IdentityRead::NotMirrored);
        }
        // A load this build does not know: the identity keeps its last rows.
        _ => return Err(format!("session {session_id}: an unknown resume load")),
    };
    // A consistent pair: if the identity's binding moved while the body was
    // read, this tick keeps the last rows and the next one reads again.
    if source.current_binding(identity).await? != Some((session_id.clone(), generation)) {
        return Ok(IdentityRead::Moved);
    }
    let document = session
        .to_persisted_bytes()
        .map_err(|error| format!("session {session_id}: {error}"))?;
    let document_sha256: [u8; 32] = Sha256::digest(&document).into();
    if same_binding && previous.is_some_and(|previous| previous.document_sha256 == document_sha256)
    {
        return Ok(IdentityRead::Unchanged);
    }
    let record = ContinuityMirrorRecord {
        identity: identity.to_owned(),
        agent_runtime_id: format!("rt:{identity}:{generation}"),
        session_id: session_id.to_string(),
        generation,
        checkpoint_version: 0,
    };
    let mut blob_ids = BTreeSet::new();
    collect_document_blob_ids(&document, &mut blob_ids)
        .map_err(|error| format!("session {session_id}: {error}"))?;
    let row = ContinuityMirrorSession {
        session_id: session_id.to_string(),
        data: base64::engine::general_purpose::STANDARD.encode(&document),
    };
    Ok(IdentityRead::Changed(
        Box::new(Mirrored {
            record,
            authority,
            document_sha256,
        }),
        row,
        blob_ids,
    ))
}

/// Where a round reads a blob from.
#[async_trait::async_trait]
trait BlobRead: Send + Sync {
    async fn read_blob(&self, blob_id: &str) -> Result<Option<ContinuityMirrorBlob>, String>;
}

#[async_trait::async_trait]
impl BlobRead for ContinuityMirrorSource {
    async fn read_blob(&self, blob_id: &str) -> Result<Option<ContinuityMirrorBlob>, String> {
        match &self.blobs {
            Some(store) => read_mirror_blob(store.as_ref(), blob_id).await,
            None => Ok(None),
        }
    }
}

/// Counts a mirror keeps. Shutdown and the health surfaces read them.
#[derive(Debug, Default)]
pub struct ContinuityMirrorCounts {
    /// Puts the sink accepted.
    pub writes: AtomicU64,
    /// Rounds or identities that failed: a binding read, a session read, a
    /// put that failed, panicked or passed its deadline.
    pub failures: AtomicU64,
    /// Identities seen with no session to copy (`Absent`, or archived and not
    /// revivable) and nothing copied before: there is nothing to keep.
    pub not_mirrored: AtomicU64,
    /// Rounds that kept an identity's last copied rows because its session
    /// read `Absent` or archived-and-not-revivable while its binding was
    /// unchanged: the copy never follows a loss.
    pub kept_after_loss: AtomicU64,
    /// Blobs the sink accepted.
    pub blobs: AtomicU64,
    /// Times a referenced blob had no copy in the runtime's blob store that
    /// matches its id. The session goes out without it.
    pub missing_blobs: AtomicU64,
}

/// The mirror's state between rounds.
#[derive(Default)]
struct MirrorState {
    identities: BTreeMap<String, Mirrored>,
    /// Session rows changed since the last accepted put, kept until one is
    /// accepted.
    pending: BTreeMap<String, ContinuityMirrorSession>,
    /// The update the sink last accepted, compared without its documents.
    accepted: Option<(Vec<ContinuityMirrorRecord>, BTreeSet<String>)>,
    /// The last sequence handed to the sink.
    sequence: u64,
    /// Identities already warned about an unreadable session.
    warned_lost: BTreeSet<String>,
    /// The blobs the sink accepted while this mirror runs.
    put_blobs: BTreeSet<String>,
}

/// Wall-clock milliseconds, kept strictly above `previous`.
fn next_sequence(previous: u64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    now.max(previous.saturating_add(1))
}

async fn round(
    source: &dyn MirrorRead,
    sink: &dyn ContinuityMirrorSink,
    put_deadline: Duration,
    state: &mut MirrorState,
    counts: &ContinuityMirrorCounts,
) {
    let bindings = match source.bindings().await {
        Ok(bindings) => bindings,
        Err(error) => {
            counts.failures.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(error = %error, "continuity mirror: reading the member bindings failed");
            return;
        }
    };
    let mut identities = BTreeMap::new();
    for binding in &bindings {
        let identity = binding.identity.clone();
        let session_id = &binding.session_id;
        let previous = state.identities.get(&identity);
        match read_identity(source, binding, previous).await {
            Ok(IdentityRead::Unchanged | IdentityRead::Moved) => {
                if let Some(previous) = previous {
                    identities.insert(identity, previous.clone());
                }
            }
            Ok(IdentityRead::Changed(mirrored, row, blob_ids)) => {
                // The session's blobs go out before its row can. A blob that
                // cannot be copied holds back this identity alone.
                if put_new_blobs(
                    source,
                    sink,
                    put_deadline,
                    &blob_ids,
                    &mut state.put_blobs,
                    counts,
                )
                .await
                {
                    state.pending.insert(row.session_id.clone(), row);
                    identities.insert(identity, *mirrored);
                } else {
                    tracing::warn!(
                        identity = %identity,
                        "continuity mirror: a blob of the session could not be copied; it keeps \
                         its last rows"
                    );
                    if let Some(previous) = previous {
                        identities.insert(identity, previous.clone());
                    }
                }
            }
            Ok(IdentityRead::NotMirrored) => {
                // The copy never follows a loss: under the same binding the
                // last copied rows stay. Only a binding that moved, or an
                // identity gone from the bindings, drops them.
                let same_binding = previous.is_some_and(|previous| {
                    previous.record.session_id == session_id.to_string()
                        && previous.record.generation == binding.generation
                });
                match previous.filter(|_| same_binding) {
                    Some(previous) => {
                        counts.kept_after_loss.fetch_add(1, Ordering::Relaxed);
                        if state.warned_lost.insert(identity.clone()) {
                            tracing::warn!(
                                identity = %identity,
                                session_id = %session_id,
                                "continuity mirror: session unreadable, keeping the last copied rows"
                            );
                        }
                        identities.insert(identity, previous.clone());
                    }
                    None => {
                        counts.not_mirrored.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Err(error) => {
                counts.failures.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    identity = %identity,
                    error = %error,
                    "continuity mirror: reading an identity failed; it keeps its last rows"
                );
                if let Some(previous) = previous {
                    identities.insert(identity, previous.clone());
                }
            }
        }
    }
    state.identities = identities;
    let records: Vec<_> = state
        .identities
        .values()
        .map(|mirrored| mirrored.record.clone())
        .collect();
    let session_ids: BTreeSet<String> = records
        .iter()
        .map(|record| record.session_id.clone())
        .collect();
    state
        .pending
        .retain(|session_id, _| session_ids.contains(session_id));
    if state.pending.is_empty()
        && state.accepted.as_ref() == Some(&(records.clone(), session_ids.clone()))
    {
        return;
    }
    state.sequence = next_sequence(state.sequence);
    let update = ContinuityMirrorUpdate {
        sequence: state.sequence,
        records,
        changed_sessions: state.pending.values().cloned().collect(),
        session_ids,
    };
    let put =
        AssertUnwindSafe(tokio::time::timeout(put_deadline, sink.put(&update))).catch_unwind();
    match put.await {
        Ok(Ok(Ok(()))) => {
            counts.writes.fetch_add(1, Ordering::Relaxed);
            state.pending.clear();
            state.accepted = Some((update.records, update.session_ids));
        }
        Ok(Ok(Err(error))) => {
            counts.failures.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(error = %error, "continuity mirror: the sink refused the update");
        }
        Ok(Err(_elapsed)) => {
            counts.failures.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                deadline_ms = u64::try_from(put_deadline.as_millis()).unwrap_or(u64::MAX),
                records = update.records.len(),
                sessions = update.changed_sessions.len(),
                session_bytes = update
                    .changed_sessions
                    .iter()
                    .map(|session| session.data.len())
                    .sum::<usize>(),
                "continuity mirror: the sink did not answer before its put deadline; the next round \
                 offers the same rows again (raise the deadline for a slow sink)"
            );
        }
        Err(_panic) => {
            counts.failures.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("continuity mirror: the sink panicked");
        }
    }
}

/// Put every blob in `blob_ids` that the sink has not accepted yet. A blob
/// the runtime's store holds no matching copy of is counted and skipped.
/// Returns false when a read failed or the sink did not accept a blob (it
/// refused, panicked or passed the put deadline): the session that references
/// it waits for the next round.
async fn put_new_blobs(
    source: &dyn BlobRead,
    sink: &dyn ContinuityMirrorSink,
    put_deadline: Duration,
    blob_ids: &BTreeSet<String>,
    put_blobs: &mut BTreeSet<String>,
    counts: &ContinuityMirrorCounts,
) -> bool {
    let new_blobs: Vec<String> = blob_ids.difference(put_blobs).cloned().collect();
    for blob_id in new_blobs {
        let blob = match source.read_blob(&blob_id).await {
            Ok(Some(blob)) => blob,
            Ok(None) => {
                counts.missing_blobs.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    blob_id = %blob_id,
                    "continuity mirror: the blob store holds no copy of a referenced blob \
                     that matches its id"
                );
                continue;
            }
            Err(error) => {
                counts.failures.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(error = %error, "continuity mirror: reading a blob failed");
                return false;
            }
        };
        let put = AssertUnwindSafe(tokio::time::timeout(put_deadline, sink.put_blob(&blob)))
            .catch_unwind();
        match put.await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => {
                counts.failures.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(error = %error, "continuity mirror: the sink refused a blob");
                return false;
            }
            Ok(Err(_elapsed)) => {
                counts.failures.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    blob_id = %blob_id,
                    "continuity mirror: the sink did not take a blob before the put deadline"
                );
                return false;
            }
            Err(_panic) => {
                counts.failures.fetch_add(1, Ordering::Relaxed);
                tracing::warn!("continuity mirror: the sink panicked on a blob");
                return false;
            }
        }
        counts.blobs.fetch_add(1, Ordering::Relaxed);
        put_blobs.insert(blob_id);
    }
    true
}

/// A running mirror. The runtime that owns it stops it at shutdown; dropping
/// it aborts the task without a last round.
pub struct ContinuityMirror {
    stop: tokio::sync::watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
    counts: Arc<ContinuityMirrorCounts>,
    stop_deadline: Duration,
}

impl ContinuityMirror {
    /// Run a round now and then every `timings.interval`.
    pub fn spawn(
        source: ContinuityMirrorSource,
        sink: Arc<dyn ContinuityMirrorSink>,
        timings: ContinuityMirrorTimings,
    ) -> Self {
        let (stop, mut stopped) = tokio::sync::watch::channel(false);
        let counts = Arc::new(ContinuityMirrorCounts::default());
        let task_counts = Arc::clone(&counts);
        let task = tokio::spawn(async move {
            let mut state = MirrorState::default();
            loop {
                let ran = AssertUnwindSafe(round(
                    &source,
                    sink.as_ref(),
                    timings.put_deadline,
                    &mut state,
                    &task_counts,
                ))
                .catch_unwind()
                .await;
                if ran.is_err() {
                    task_counts.failures.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!("continuity mirror: a round panicked; the next one starts over");
                    state = MirrorState::default();
                }
                if *stopped.borrow() {
                    return;
                }
                tokio::select! {
                    () = tokio::time::sleep(timings.interval) => {}
                    // A stop runs one last round above, then returns.
                    _ = stopped.changed() => {}
                }
            }
        });
        Self {
            stop,
            task: Some(task),
            counts,
            stop_deadline: timings.stop_deadline,
        }
    }

    pub fn counts(&self) -> Arc<ContinuityMirrorCounts> {
        Arc::clone(&self.counts)
    }

    /// Run a last round and end the task, within the stop deadline; past it
    /// the task is aborted and the stop counts as a failure.
    pub async fn stop(mut self) -> Arc<ContinuityMirrorCounts> {
        let _ = self.stop.send(true);
        if let Some(mut task) = self.task.take()
            && tokio::time::timeout(self.stop_deadline, &mut task)
                .await
                .is_err()
        {
            task.abort();
            self.counts.failures.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                deadline_ms = self.stop_deadline.as_millis() as u64,
                "continuity mirror: the last round did not end before its deadline"
            );
        }
        Arc::clone(&self.counts)
    }
}

impl Drop for ContinuityMirror {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl super::UnifiedRuntime {
    /// Start the continuity mirror the builder asked for, owned by this
    /// runtime. It reads through the runtime's own non-owning bridge.
    pub(crate) async fn install_continuity_mirror(
        &self,
        sink: Arc<dyn ContinuityMirrorSink>,
        timings: ContinuityMirrorTimings,
    ) -> Result<(), String> {
        if self.identity_first_context.is_none() {
            return Err(
                "this runtime is not identity-first, so it has no durable identity to copy"
                    .to_string(),
            );
        }
        let bridge = self
            .session_bridge
            .clone()
            .ok_or_else(|| "it needs an identity-first runtime (no session bridge)".to_string())?;
        let service = self
            .mob_runtime
            .session_service()
            .cloned()
            .ok_or_else(|| "it needs a session service".to_string())?;
        let mirror = ContinuityMirror::spawn(
            ContinuityMirrorSource::new(bridge, service, self.binary_blob_store()),
            sink,
            timings,
        );
        *self.continuity_mirror.lock().await = Some(mirror);
        Ok(())
    }

    /// The installed continuity mirror's counts, or `None` without one (or
    /// once shutdown has stopped it).
    pub async fn continuity_mirror_counts(&self) -> Option<Arc<ContinuityMirrorCounts>> {
        self.continuity_mirror
            .lock()
            .await
            .as_ref()
            .map(ContinuityMirror::counts)
    }
}

#[cfg(test)]
#[path = "continuity_mirror_blob_tests.rs"]
mod blob_tests;

#[cfg(test)]
#[path = "continuity_mirror_core_tests.rs"]
mod core_tests;
