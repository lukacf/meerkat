//! The per-session import of `meta_gateway storage-migrate-native --apply`
//! (I1-I4).
//!
//! A session that only the 0.9.3 projection side holds (a census
//! `RuntimeMissing` row, or a ready entry of a continuity export) enters Core
//! through Core's own external activation, one session at a time. Meta
//! writes no runtime authority and no catalog row itself: the session store
//! creates the row and crosses it into head-canonical authority, and the
//! runtime store activates the exact proof that crossing returned.
//!
//! - I1: the session's rewrite history validates, and Core can lay it out as
//!   head-canonical rows. This is the census's own check, run again before
//!   the first write, on the session the import writes: the copy with every
//!   image it holds inline, its compacted history included, moved to a blob
//!   reference by Core's own pure transform.
//! - I2: `save_authoritative_projection_if_current_revision(session, None)`,
//!   which only creates the row, in one transaction, once the images the
//!   transform moved are in the realm blob store (`blobs/` in the state
//!   directory, the store the native composition hydrates them from) and
//!   synced there: no row names an image the store does not hold. A rerun
//!   that finds a row compares the transcript digest and the model-routing
//!   control history. An equal row is kept; a different one refuses, and
//!   nothing is written. It never compares whole sessions.
//! - I3: `cross_head_canonical_authority` for this one id. The bulk store
//!   activation is never called: it would cross rows the census never
//!   admitted.
//! - I4: `activate_head_canonical_runtime_authority` with I3's proof.
//!
//! Every step is idempotent, so a run that stopped after any of them
//! finishes on the next `--apply`. The caller holds the migration's runtime
//! exclusion across all four and records each finished session in its
//! journal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use meerkat::SessionStore;
use meerkat_core::BlobStore as _;
use meerkat_core::types::SessionId;
use meerkat_core::{BlobId, HeadCanonicalAuthorityCrossing, InlineImagePayload, Session};
use meerkat_runtime::{HeadCanonicalRuntimeAuthorityActivation, RuntimeStore};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::census::{META_RUNTIME_LOCK_FILE_NAME, SessionCensusClass, layout_checked, remint_copy};
use super::export_census::ExportProjection;
use crate::blob_store::{Base64BlobStoreAdapter, ObjectStoreBlobStore};
use crate::identity_first::{ContinuitySessionStoreAdapter, LocalContinuityStore};
use crate::storage_layout::{BLOB_ROOT_DIR_NAME, RUNTIME_DB_FILE_NAME};

/// Where an apply run reads the sessions it imports. The journal keeps it,
/// so a run that stopped part way reads the same copies again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ImportSource {
    /// The pre-apply backup of the projection database `file` (the run's
    /// backup names the journal holds), read the way the census read the
    /// projection.
    Projection {
        file: String,
        store: ProjectionStore,
    },
    /// A continuity export file, by absolute path, and the digest of its
    /// entries the census read. A file whose entries differ refuses.
    Export {
        path: PathBuf,
        entries_sha256: String,
    },
}

/// Which store a projection database is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProjectionStore {
    /// `sessions.sqlite3`: Core's SQLite session store.
    Sessions,
    /// `continuity.sqlite3`: Meta's continuity store.
    Continuity,
}

/// The stores an import writes: exactly the session store and runtime store
/// the native composition opens on the state directory, and the realm blob
/// store it hydrates images from.
#[derive(Clone)]
pub(crate) struct NativeImportStores {
    pub(crate) sessions: Arc<dyn SessionStore>,
    pub(crate) runtime: Arc<dyn RuntimeStore>,
    pub(crate) blobs: Arc<RealmBlobs>,
}

/// One import step, as refusals and the report name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ImportStep {
    /// I1: decode and validate.
    Validate,
    /// I2: create the session store row, the images it names stored in the
    /// realm blob store first.
    CreateRow,
    /// I3: cross the row into head-canonical authority.
    Cross,
    /// I4: activate the runtime authority from the crossing's proof.
    Activate,
}

/// What I2 found and did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ImportedRow {
    /// The session store held no row; I2 created it.
    Created,
    /// A rerun found the same transcript and model-routing control history
    /// already in the store, and kept that row.
    Kept,
}

/// What I3 answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ImportedCrossing {
    Converted,
    AlreadyCurrent,
}

/// What I4 answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ImportedActivation {
    Installed,
    AlreadyAligned,
    RepresentationAdvanced,
}

/// One imported session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct SessionImport {
    pub session_id: String,
    pub row: ImportedRow,
    pub crossing: ImportedCrossing,
    pub activation: ImportedActivation,
}

/// Why one session's import stopped. The steps before it stay written; each
/// is idempotent, so a rerun continues from there once the cause is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionImportRefusal {
    /// I1: the session's rewrite history does not validate, or Core cannot
    /// lay the session out as head-canonical rows. Nothing was written.
    Invalid { session_id: String, detail: String },
    /// I2 on a rerun: the session store already holds a different row for
    /// the session (another transcript, or another model-routing control
    /// history). Nothing was written.
    DifferentRow { session_id: String, detail: String },
    /// I3: the session store does not own head-canonical authority, so it
    /// cannot issue the proof I4 needs.
    NotHeadCanonical { session_id: String },
    /// A store refused or failed `step`.
    StoreFailed {
        session_id: String,
        step: ImportStep,
        detail: String,
    },
}

impl std::fmt::Display for SessionImportRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { session_id, detail } => {
                write!(f, "session {session_id} does not validate: {detail}")
            }
            Self::DifferentRow { session_id, detail } => write!(
                f,
                "session {session_id}: the session store already holds a different row ({detail})"
            ),
            Self::NotHeadCanonical { session_id } => write!(
                f,
                "session {session_id}: the session store does not own head-canonical authority"
            ),
            Self::StoreFailed {
                session_id,
                step,
                detail,
            } => write!(f, "session {session_id}: {step:?} failed: {detail}"),
        }
    }
}

/// Core's session store and runtime store over one `runtime.sqlite`.
type NativePair = (Arc<dyn SessionStore>, Arc<dyn RuntimeStore>);

/// Open the native pair on the state directory: Core's session store and
/// the head-canonical runtime store, both on `runtime.sqlite`, the stores
/// the native composition opens. A new directory gets both created.
pub(crate) fn open_native_pair(state_dir: &Path) -> Result<NativePair, String> {
    let path = state_dir.join(RUNTIME_DB_FILE_NAME);
    let sessions = meerkat_store::SqliteSessionStore::open(path.clone())
        .map_err(|error| format!("session store on {}: {error}", path.display()))?;
    let runtime = meerkat_runtime::SqliteRuntimeStore::new_head_canonical(path.clone())
        .map_err(|error| format!("runtime store on {}: {error}", path.display()))?;
    Ok((Arc::new(sessions), Arc::new(runtime)))
}

/// The realm blob store `--apply` moves inline images to: `blobs/` in the
/// state directory, the local store the native composition hydrates image
/// references from, holding each image under the address Core gives it
/// ([`meerkat_core::content_blob_id`]).
pub(crate) struct RealmBlobs {
    state_dir: PathBuf,
    local: Arc<ObjectStoreBlobStore>,
    store: Base64BlobStoreAdapter,
}

impl RealmBlobs {
    pub(crate) fn open(state_dir: &Path) -> Result<Self, String> {
        let root = state_dir.join(BLOB_ROOT_DIR_NAME);
        let local = Arc::new(
            ObjectStoreBlobStore::local(root.clone())
                .map_err(|error| format!("blob store at {}: {error}", root.display()))?,
        );
        Ok(Self {
            state_dir: state_dir.to_path_buf(),
            store: Base64BlobStoreAdapter::new(local.clone()),
            local,
        })
    }

    /// Core's face of the store.
    pub(crate) fn store(&self) -> &dyn meerkat_core::BlobStore {
        &self.store
    }

    /// Store each image under its content address, then sync them all.
    pub(crate) async fn put(&self, payloads: &[InlineImagePayload]) -> Result<(), String> {
        for payload in payloads {
            let stored = self
                .store
                .put_image(&payload.media_type, &payload.data)
                .await
                .map_err(|error| format!("store image {}: {error}", payload.blob_id.as_str()))?;
            if stored.blob_id != payload.blob_id {
                return Err(format!(
                    "the blob store addressed image {} as {}",
                    payload.blob_id.as_str(),
                    stored.blob_id.as_str()
                ));
            }
        }
        self.sync(payloads.iter().map(|payload| &payload.blob_id))
    }

    /// Make the images `stored` names durable: their files, then every
    /// directory that names them, up to the state directory. The object
    /// store's local puts rename into place without a sync, and a row must
    /// not outlast an image it names.
    pub(crate) fn sync<'a>(
        &self,
        stored: impl IntoIterator<Item = &'a BlobId>,
    ) -> Result<(), String> {
        let mut dirs = std::collections::BTreeSet::new();
        for blob_id in stored {
            let files = self
                .local
                .local_files(blob_id)
                .ok_or_else(|| "the realm blob store has no local root".to_owned())?;
            for file in files {
                super::apply::sync_file(&file)?;
                if let Some(dir) = file.parent() {
                    dirs.insert(dir.to_path_buf());
                }
            }
        }
        if dirs.is_empty() {
            return Ok(());
        }
        dirs.insert(self.state_dir.join(BLOB_ROOT_DIR_NAME));
        dirs.insert(self.state_dir.clone());
        dirs.iter().try_for_each(|dir| super::apply::sync_dir(dir))
    }
}

/// Hold the directory's Meta runtime lock exclusively for as long as the
/// returned file lives. A 0.9.3 directory has no lock file, so this
/// creates it; a holder refuses.
pub(crate) fn hold_meta_runtime_lock(state_dir: &Path) -> Result<std::fs::File, String> {
    let path = state_dir.join(META_RUNTIME_LOCK_FILE_NAME);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(format!(
            "{} is held: a Meta runtime uses the directory; stop it and run --apply again",
            path.display()
        )),
        Err(std::fs::TryLockError::Error(error)) => Err(format!("{}: {error}", path.display())),
    }
}

/// Import `pending` from `source` into the native pair, in order, through
/// I1-I4. `record` runs after each session's I4; the first refusal stops
/// the run, and what it recorded stays imported.
pub(crate) async fn import_planned(
    state_dir: &Path,
    source: &ImportSource,
    backups: &BTreeMap<String, String>,
    pending: &[String],
    mut record: impl FnMut(SessionImport) -> Result<(), String>,
) -> Result<(), String> {
    let reader = ImportReader::open(state_dir, source, backups)?;
    let (sessions, runtime) = open_native_pair(state_dir)?;
    let blobs = Arc::new(RealmBlobs::open(state_dir)?);
    let stores = NativeImportStores {
        sessions,
        runtime,
        blobs,
    };
    for session_id in pending {
        let copy = remint_copy(session_id, &reader.store)
            .await
            .map_err(|detail| {
                format!("session {session_id}: the copy to import does not load: {detail}")
            })?;
        let import = import_session(&stores, &copy.session)
            .await
            .map_err(|refusal| refusal.to_string())?;
        record(import)?;
    }
    Ok(())
}

/// The projection side an import reads, opened over a copy the run owns.
struct ImportReader {
    store: Arc<dyn SessionStore>,
    _scratch: Option<tempfile::TempDir>,
}

impl ImportReader {
    fn open(
        state_dir: &Path,
        source: &ImportSource,
        backups: &BTreeMap<String, String>,
    ) -> Result<Self, String> {
        match source {
            ImportSource::Projection { file, store } => {
                let backup = backups.get(file).ok_or_else(|| {
                    format!("the journal names no backup of {file} to import from")
                })?;
                let parent = match state_dir.parent() {
                    Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
                    _ => PathBuf::from("."),
                };
                let scratch = tempfile::Builder::new()
                    .prefix(".meta-storage-migrate-native-")
                    .tempdir_in(&parent)
                    .map_err(|error| {
                        format!("scratch directory in {}: {error}", parent.display())
                    })?;
                let copy = scratch.path().join(file);
                std::fs::copy(state_dir.join(backup), &copy)
                    .map_err(|error| format!("copy the backup {backup}: {error}"))?;
                let store: Arc<dyn SessionStore> = match store {
                    ProjectionStore::Sessions => Arc::new(
                        meerkat_store::SqliteSessionStore::open(copy.clone())
                            .map_err(|error| format!("{}: {error}", copy.display()))?,
                    ),
                    ProjectionStore::Continuity => {
                        let records = LocalContinuityStore::open(&copy)
                            .map_err(|error| format!("{}: {error}", copy.display()))?;
                        Arc::new(ContinuitySessionStoreAdapter::new(Arc::new(records)))
                    }
                };
                Ok(Self {
                    store,
                    _scratch: Some(scratch),
                })
            }
            ImportSource::Export {
                path,
                entries_sha256,
            } => {
                let projection = ExportProjection::read(path)?;
                if &projection.file.entries_sha256 != entries_sha256 {
                    return Err(format!(
                        "{}: the export's entries changed since the census read them \
                         (entries_sha256 {}, the journal holds {entries_sha256}); pass the file \
                         the run started with",
                        path.display(),
                        projection.file.entries_sha256
                    ));
                }
                Ok(Self {
                    store: projection.census_projection().store,
                    _scratch: None,
                })
            }
        }
    }
}

/// Import one session through I1-I4.
pub(crate) async fn import_session(
    stores: &NativeImportStores,
    session: &Session,
) -> Result<SessionImport, SessionImportRefusal> {
    let id = session.id().clone();
    let session_id = id.to_string();
    let failed = |step: ImportStep| {
        let session_id = session_id.clone();
        move |error: String| SessionImportRefusal::StoreFailed {
            session_id,
            step,
            detail: error,
        }
    };

    // I1. The census admitted this copy through the same check; it runs
    // again because the apply reads the copy anew.
    let (class, detail) =
        layout_checked(SessionCensusClass::RuntimeMissing, String::new(), session);
    if class != SessionCensusClass::RuntimeMissing {
        return Err(SessionImportRefusal::Invalid {
            session_id,
            detail: detail.trim_start_matches("; ").to_owned(),
        });
    }

    // The session the import writes: the copy with its inline images moved
    // to blob references, the session I1 laid out, and those images.
    let moved;
    let (session, images) = match session.with_inline_images_externalized() {
        Ok(None) => (session, &[][..]),
        Ok(Some(externalized)) => {
            moved = externalized;
            (&moved.session, moved.payloads.as_slice())
        }
        Err(error) => {
            return Err(SessionImportRefusal::Invalid {
                session_id,
                detail: format!("its inline images cannot move to the blob store: {error}"),
            });
        }
    };

    // I2.
    let existing = stores
        .sessions
        .load(&id)
        .await
        .map_err(|error| error.to_string())
        .map_err(failed(ImportStep::CreateRow))?;
    let row = match existing {
        Some(existing) => {
            if let Some(detail) = row_difference(&existing, session) {
                return Err(SessionImportRefusal::DifferentRow { session_id, detail });
            }
            ImportedRow::Kept
        }
        None => {
            // The images first, synced: no row names an image the store
            // does not hold.
            stores
                .blobs
                .put(images)
                .await
                .map_err(failed(ImportStep::CreateRow))?;
            stores
                .sessions
                .save_authoritative_projection_if_current_revision(session, None)
                .await
                .map_err(|error| error.to_string())
                .map_err(failed(ImportStep::CreateRow))?;
            ImportedRow::Created
        }
    };

    // I3: this one id only.
    let Some(incremental) = Arc::clone(&stores.sessions).as_incremental() else {
        return Err(SessionImportRefusal::NotHeadCanonical { session_id });
    };
    let crossing = incremental
        .cross_head_canonical_authority(&id)
        .await
        .map_err(|error| error.to_string())
        .map_err(failed(ImportStep::Cross))?;
    let (authority, crossing) = match crossing {
        HeadCanonicalAuthorityCrossing::Converted(authority) => {
            (authority, ImportedCrossing::Converted)
        }
        HeadCanonicalAuthorityCrossing::AlreadyCurrent(authority) => {
            (authority, ImportedCrossing::AlreadyCurrent)
        }
        HeadCanonicalAuthorityCrossing::NotApplicable => {
            return Err(SessionImportRefusal::NotHeadCanonical { session_id });
        }
    };

    // I4: the proof straight from I3.
    let activation = stores
        .runtime
        .activate_head_canonical_runtime_authority(authority)
        .await
        .map_err(|error| error.to_string())
        .map_err(failed(ImportStep::Activate))?;
    let activation = match activation {
        HeadCanonicalRuntimeAuthorityActivation::Installed(_) => ImportedActivation::Installed,
        HeadCanonicalRuntimeAuthorityActivation::AlreadyAligned(_) => {
            ImportedActivation::AlreadyAligned
        }
        HeadCanonicalRuntimeAuthorityActivation::RepresentationAdvanced(_) => {
            ImportedActivation::RepresentationAdvanced
        }
    };
    Ok(SessionImport {
        session_id,
        row,
        crossing,
        activation,
    })
}

/// How a row the store already holds differs from the session being
/// imported: its transcript digest, or its model-routing control history.
/// `None` when both agree.
pub(super) fn row_difference(existing: &Session, incoming: &Session) -> Option<String> {
    let digest = |session: &Session| {
        meerkat_core::transcript_messages_digest(session.messages())
            .map_err(|error| format!("transcript digest: {error}"))
    };
    match (digest(existing), digest(incoming)) {
        (Ok(stored), Ok(imported)) if stored != imported => {
            return Some(format!(
                "stored transcript digest {stored}, {} messages; imported {imported}, {} messages",
                existing.messages().len(),
                incoming.messages().len()
            ));
        }
        (Err(error), _) | (_, Err(error)) => return Some(error),
        _ => {}
    }
    (existing.model_routing_control() != incoming.model_routing_control())
        .then(|| "the model-routing control histories differ".to_owned())
}

/// Decode a session document an export entry carries, the way 0.9.3 read
/// its provider's snapshots: the current persisted form, else a released
/// 0.8.10 envelope through Core's one-time importer (its receipt re-proved
/// against these exact bytes). The document must be the record's session.
pub(crate) fn decode_session_document(
    expected: &SessionId,
    bytes: &[u8],
) -> Result<Session, String> {
    let session = match Session::from_persisted_bytes(bytes) {
        Ok(session) => session,
        Err(decode_error) => {
            let imported = meerkat_core::import_released_0810_session(bytes).map_err(|import| {
                format!(
                    "the document decodes neither as a current session ({decode_error}) nor as a \
                     released 0.8.10 envelope ({import})"
                )
            })?;
            let (session, receipt) = imported.into_parts();
            let observed: [u8; 32] = Sha256::digest(bytes).into();
            if receipt.source_document_sha256() != &observed {
                return Err("the released 0.8.10 document changed during its import".to_owned());
            }
            if receipt.session_id() != expected {
                return Err(format!(
                    "the released 0.8.10 document is session {}, not the record's session \
                     {expected}",
                    receipt.session_id()
                ));
            }
            session
        }
    };
    if session.id() != expected {
        return Err(format!(
            "the document is session {}, not the record's session {expected}",
            session.id()
        ));
    }
    Ok(session)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    /// The migration crosses one id at a time and never runs the bulk
    /// store activation, which would cross rows the census never admitted.
    #[test]
    fn the_migration_never_calls_the_bulk_store_activation() {
        let needle = concat!("activate_head_canonical", "_store(");
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/storage_migrate_native");
        let mut read = 0;
        for entry in std::fs::read_dir(dir).expect("read the module directory") {
            let path = entry.expect("entry").path();
            if path.extension().is_some_and(|extension| extension == "rs") {
                let source = std::fs::read_to_string(&path).expect("read source");
                assert!(
                    !source.contains(needle),
                    "{} calls the bulk store activation",
                    path.display()
                );
                read += 1;
            }
        }
        assert!(read >= 4, "read {read} source files");
    }

    #[test]
    fn a_document_for_another_session_is_refused() {
        let session = Session::new();
        let bytes = session.to_persisted_bytes().expect("encode");
        let other = Session::new();
        let error = decode_session_document(other.id(), &bytes).expect_err("another session");
        assert!(error.contains("not the record's session"), "{error}");
        let decoded = decode_session_document(session.id(), &bytes).expect("decode");
        assert_eq!(decoded.id(), session.id());
    }

    #[test]
    fn a_document_that_is_no_session_is_refused() {
        let error = decode_session_document(Session::new().id(), b"{\"synthetic\":true}")
            .expect_err("not a session");
        assert!(error.contains("decodes neither"), "{error}");
    }

    #[test]
    fn rows_differ_by_transcript_or_routing_control_only() {
        let session = Session::new();
        assert_eq!(row_difference(&session, &session.clone()), None);
        let mut longer = session.clone();
        longer.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("one more"),
        ));
        let detail = row_difference(&session, &longer).expect("a longer transcript differs");
        assert!(detail.contains("transcript digest"), "{detail}");
    }

    /// The native pair and realm blob store an import writes, on a new
    /// state directory.
    fn native_stores(state_dir: &Path) -> NativeImportStores {
        let (sessions, runtime) = open_native_pair(state_dir).expect("the native pair");
        let blobs = Arc::new(RealmBlobs::open(state_dir).expect("the realm blob store"));
        NativeImportStores {
            sessions,
            runtime,
            blobs,
        }
    }

    /// A copy whose compacted history holds an inline image imports: the
    /// image goes to the realm blob store before the row that names it, the
    /// row holds the image by its content address, and a rerun keeps the
    /// row.
    #[tokio::test]
    async fn an_import_moves_the_copys_inline_images_to_the_realm_blob_store() {
        const SCREENSHOT: &str = "iVBORw0KGgo=";
        let dir = tempfile::tempdir().unwrap();
        let stores = native_stores(dir.path());
        let session = super::super::census::tests::compacted_over_a_tool_result_image(SCREENSHOT);

        let import = import_session(&stores, &session)
            .await
            .expect("the copy imports");
        assert_eq!(import.row, ImportedRow::Created, "{import:?}");
        let row = stores
            .sessions
            .load(session.id())
            .await
            .unwrap()
            .expect("the row");
        let document = serde_json::to_string(&row).unwrap();
        assert!(
            !document.contains(SCREENSHOT),
            "the row holds the image inline"
        );
        let blob_id = meerkat_core::content_blob_id("image/png", SCREENSHOT);
        assert!(document.contains(blob_id.as_str()), "{document}");
        let blobs = crate::blob_store::Base64BlobStoreAdapter::new(Arc::new(
            crate::blob_store::ObjectStoreBlobStore::local(dir.path().join("blobs")).unwrap(),
        ));
        let image = meerkat_core::BlobStore::get(&blobs, &blob_id)
            .await
            .expect("the image hydrates from blobs/");
        assert_eq!(image.data, SCREENSHOT);

        let again = import_session(&stores, &session).await.expect("the rerun");
        assert_eq!(again.row, ImportedRow::Kept, "{again:?}");
    }
}
