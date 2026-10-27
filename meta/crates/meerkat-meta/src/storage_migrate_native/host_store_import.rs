//! Import a continuity export into a host store realm: the library API a
//! host calls with its own [`HostStore`] (which lives in the host's binary,
//! so no gateway verb can open it) before the realm's first start.
//!
//! The export file is the one `tools/continuity-export` writes from a 0.9.3
//! host's continuity provider, read and verified as `meta_gateway
//! storage-migrate-native --continuity-export` reads it. A host store realm
//! is WholeBlob, so each ready entry's session document is created in the
//! realm's runtime store as one document, through the runtime store's
//! create-only import ([`meerkat_runtime::RuntimeStore::import_whole_blob_session`]),
//! which records the document's pending compaction projection intents in the
//! same write. The identities follow the in-memory rule of the native
//! migration: a new realm holds no mob, so every ready entry gets a
//! first-boot `Resume` row, and the realm's first start carries the rows out
//! exactly as it carries out a state directory's first-boot manifest.
//!
//! # Steps
//!
//! 1. Read and verify the export.
//! 2. Open the realm with an exclusive acquire of its fence; a live holder
//!    (a runtime, or another import) refuses, and a takeover is never made.
//! 3. Admit the realm: a new realm; or one whose realm record names this
//!    export's import (a rerun). A realm a runtime has used, or one an import
//!    of another export started, refuses.
//! 4. Census, which a dry run stops after: decode each ready entry, move its
//!    inline images to blob references, and compare the document with the
//!    one the realm holds for the session, if any.
//! 5. Apply: the realm record with the import in progress (from here a
//!    runtime refuses to open the realm, typed); per entry, its images into
//!    the blob store and then its document; the first-boot rows; the import
//!    done. Every step is its own batch and idempotent, so a rerun with the
//!    same export finds each landed session kept and continues. The realm
//!    record and the scopes are the journal.
//!
//! The import holds the realm's fence for its whole run. Once it drops the
//! realm, its lease lapses after the host's [`FenceTiming`](meerkat_core::host_store::FenceTiming)
//! lease; a runtime opening the realm before then meets `Held` with what
//! remains of it.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use meerkat_core::BlobStore as _;
use meerkat_core::host_store::{AcquireMode, FenceToken, HostStore};
use meerkat_core::types::SessionId;
use meerkat_runtime::store::spi::{SpiError, SpiRealm, SpiRealmConfig, SpiRuntimeStore};
use meerkat_runtime::{LogicalRuntimeId, RuntimeStore};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use super::export_file::{ExportFile, read_entry_at, read_export_file};
use super::identity::{ClaimSource, FirstBootRowPlan, IdentityClaim, identity_census};
use super::import::decode_session_document;
use crate::blob_store::{Base64BlobStoreAdapter, BinaryBlobStore};
use crate::host_store_documents::{
    DocumentChange, HostStoreImportRecord, HostStoreImportState, HostStoreRealmRecord,
    MetaDocumentError, MetaDocuments, REALM_RECORD_KEY, REALM_RECORD_VERSION, RealmRecordError,
    decode_realm_record, encode_realm_record, read_realm_record,
};
use crate::identity_first::{FirstBootManifest, FirstBootManifestRefusal, FirstBootRow};
use crate::storage_migrate::MigrateMode;
use crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID;

/// The label the realm's documents carry in the import's refusals.
const IMPORT_LABEL: &str = "import";

/// What to import, from where, into which realm.
pub struct HostStoreImportRequest {
    /// The host store that keeps the realm.
    pub host: Arc<dyn HostStore>,
    /// How the realm opens. The acquire must be [`AcquireMode::Exclusive`]:
    /// an import never fences out a live holder.
    pub realm_config: SpiRealmConfig,
    /// The blob store the runtime will be given
    /// ([`crate::host_store::HostStoreMetaStores::blob_store`]): the images
    /// the documents hold inline move there.
    pub blob_store: Arc<dyn BinaryBlobStore>,
    /// The continuity export file, format version 1.
    pub continuity_export: PathBuf,
    /// A dry run reads the realm and writes no session, document or row; an
    /// apply imports. Either takes the realm's fence like any open and holds
    /// it until its lease lapses, so a runtime started right after it meets
    /// `Held` and waits up to that lease.
    pub mode: MigrateMode,
}

/// The export file an import read.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct HostStoreImportExport {
    pub path: PathBuf,
    pub writer: String,
    /// The digest of the entry lines; the realm record keeps it, and a rerun
    /// with another file refuses.
    pub entries_sha256: String,
    pub entries: usize,
    pub ready: usize,
}

/// What became of one ready entry's session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HostStoreImportClass {
    /// The realm holds no document for the session: an apply creates it.
    New,
    /// The apply created the session's document.
    Created,
    /// The realm already holds this exact document (a rerun).
    Kept,
    /// The realm holds another document for the session: refused, and
    /// nothing is written.
    DifferentRow,
    /// The entry's document does not decode as the record's session, or its
    /// inline images cannot move to the blob store: refused, and nothing is
    /// written.
    Invalid,
}

/// One ready entry of the export.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct HostStoreImportRow {
    pub identity: String,
    pub session_id: String,
    pub class: HostStoreImportClass,
    /// The document's size as the realm keeps it, its inline images moved
    /// to blob references.
    pub document_bytes: u64,
    /// The inline images moved to the blob store.
    pub images: usize,
    /// The pending compaction projection intents the document carries; the
    /// import records them in the runtime store's outbox with the document.
    pub pending_compaction_projections: usize,
    /// Why the row is refused, or what the realm holds instead.
    pub detail: Option<String>,
}

/// Why an import refused or stopped. A refused run wrote nothing more than
/// what an earlier step of it already landed, and a rerun with the same
/// export continues from there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HostStoreImportRefusal {
    /// The `realm_config` asks for a takeover: an import never fences out a
    /// live holder.
    TakeoverRefused,
    /// Another live holder has the realm's fence (a runtime, or another
    /// import): wait `remaining` and run the import again.
    RealmHeld {
        holder: FenceToken,
        remaining: Duration,
    },
    /// The realm is not a new realm and not one this export's import began:
    /// a runtime has used it.
    RealmInUse { detail: String },
    /// An import of another export began in this realm.
    OtherExport { recorded: String, this: String },
    /// A newer Meta wrote the realm.
    RealmFormat { found: u32, supported: u32 },
    /// The export file is not a valid continuity export.
    Export { detail: String },
    /// The census refused rows (`DifferentRow`, `Invalid`); the apply writes
    /// nothing.
    RowsRefused { count: usize },
    /// The host store, the blob store or the runtime store refused or
    /// failed.
    Store { detail: String },
}

impl std::fmt::Display for HostStoreImportRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TakeoverRefused => f.write_str(
                "the import opens the realm with an exclusive acquire only: it never fences out \
                 a live holder",
            ),
            Self::RealmHeld { holder, remaining } => write!(
                f,
                "the host store realm is held by fence token {holder} for {remaining:?} more \
                 (a runtime or another import); run the import again once that time has passed"
            ),
            Self::RealmInUse { detail } => write!(
                f,
                "the host store realm is in use and takes no import: {detail}"
            ),
            Self::OtherExport { recorded, this } => write!(
                f,
                "an import of another export (entries_sha256 {recorded}) began in this realm; \
                 this export is {this}. Finish that import with its own file"
            ),
            Self::RealmFormat { found, supported } => write!(
                f,
                "a newer Meta wrote this realm (realm record format version {found}; this build \
                 reads version {supported})"
            ),
            Self::Export { detail } => write!(f, "the continuity export is refused: {detail}"),
            Self::RowsRefused { count } => write!(
                f,
                "{count} session(s) cannot be imported (see their rows); nothing is written"
            ),
            Self::Store { detail } => write!(f, "the import stopped: {detail}"),
        }
    }
}

/// The report of one import run.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct HostStoreImportReport {
    pub mode: MigrateMode,
    /// The export file, once it read and verified.
    pub export: Option<HostStoreImportExport>,
    /// One row per ready entry, in export order.
    pub sessions: Vec<HostStoreImportRow>,
    /// The first-boot rows the import writes: a `Resume` row per ready
    /// entry.
    pub first_boot_rows: Vec<FirstBootRowPlan>,
    /// Whether the realm holds the first-boot rows now.
    pub first_boot_written: bool,
    /// The identities the export lists without a session: their first wake
    /// is fresh.
    pub uninitialized_identities: Vec<String>,
    /// Whether the realm's import is done, so a runtime may open the realm.
    pub completed: bool,
    pub refusals: Vec<HostStoreImportRefusal>,
}

impl HostStoreImportReport {
    fn new(mode: MigrateMode) -> Self {
        Self {
            mode,
            export: None,
            sessions: Vec::new(),
            first_boot_rows: Vec::new(),
            first_boot_written: false,
            uninitialized_identities: Vec::new(),
            completed: false,
            refusals: Vec::new(),
        }
    }

    fn refused(mut self, refusal: HostStoreImportRefusal) -> Self {
        self.refusals.push(refusal);
        self
    }

    /// Whether the run refused nothing.
    pub fn is_clean(&self) -> bool {
        self.refusals.is_empty()
    }
}

/// One ready entry as the census read it.
struct CensusEntry {
    /// Where its line starts in the file.
    offset: u64,
    session_id: SessionId,
    /// The document as the realm keeps it, by digest; `None` for a refused
    /// entry.
    document_sha256: Option<[u8; 32]>,
}

/// What the file pass found.
struct ExportRead {
    file: ExportFile,
    entries: Vec<CensusEntry>,
    rows: Vec<HostStoreImportRow>,
}

/// A ready entry's document as the realm keeps it: decoded as the record's
/// session, its inline images moved to blob references.
struct PreparedDocument {
    bytes: Vec<u8>,
    images: Vec<meerkat_core::InlineImagePayload>,
    pending_compaction_projections: usize,
}

fn prepare_document(session_id: &SessionId, raw: &[u8]) -> Result<PreparedDocument, String> {
    let session = decode_session_document(session_id, raw)?;
    let pending_compaction_projections = session
        .validated_compaction_projection_intents()
        .map_err(|error| format!("its compaction projection intents do not validate: {error}"))?
        .len();
    let (session, images) = match session.with_inline_images_externalized() {
        Ok(None) => (session, Vec::new()),
        Ok(Some(moved)) => (moved.session, moved.payloads),
        Err(error) => {
            return Err(format!(
                "its inline images cannot move to the blob store: {error}"
            ));
        }
    };
    let bytes = session
        .to_persisted_bytes()
        .map_err(|error| format!("the document does not encode: {error}"))?;
    Ok(PreparedDocument {
        bytes,
        images,
        pending_compaction_projections,
    })
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Read and verify the export, and prepare each ready entry's document once.
fn read_export(path: &std::path::Path) -> Result<ExportRead, String> {
    let mut entries = Vec::new();
    let mut rows = Vec::new();
    let file = read_export_file(path, |entry, offset, raw| {
        let Some(record) = entry.record.as_ref() else {
            return;
        };
        let (document_sha256, row) = match prepare_document(&record.session_id, &raw) {
            Ok(prepared) => (
                Some(sha256(&prepared.bytes)),
                HostStoreImportRow {
                    identity: entry.identity.clone(),
                    session_id: record.session_id.to_string(),
                    class: HostStoreImportClass::New,
                    document_bytes: prepared.bytes.len() as u64,
                    images: prepared.images.len(),
                    pending_compaction_projections: prepared.pending_compaction_projections,
                    detail: None,
                },
            ),
            Err(detail) => (
                None,
                HostStoreImportRow {
                    identity: entry.identity.clone(),
                    session_id: record.session_id.to_string(),
                    class: HostStoreImportClass::Invalid,
                    document_bytes: 0,
                    images: 0,
                    pending_compaction_projections: 0,
                    detail: Some(format!("{detail}; {}", record.values_093())),
                },
            ),
        };
        entries.push(CensusEntry {
            offset,
            session_id: record.session_id.clone(),
            document_sha256,
        });
        rows.push(row);
    })
    .map_err(|error| error.to_string())?;
    Ok(ExportRead {
        file,
        entries,
        rows,
    })
}

fn store_refusal(detail: impl std::fmt::Display) -> HostStoreImportRefusal {
    HostStoreImportRefusal::Store {
        detail: detail.to_string(),
    }
}

/// The realm, as an import holds it.
struct ImportRealm {
    /// Held for the import's whole run: dropping it lets the lease lapse.
    _realm: Arc<SpiRealm>,
    documents: MetaDocuments,
    runtime: SpiRuntimeStore,
    mob_storages: usize,
}

async fn open_realm(
    request: &HostStoreImportRequest,
) -> Result<ImportRealm, HostStoreImportRefusal> {
    let realm = SpiRealm::open(Arc::clone(&request.host), request.realm_config.clone())
        .await
        .map_err(|error| match error {
            SpiError::Held { holder, remaining } => {
                HostStoreImportRefusal::RealmHeld { holder, remaining }
            }
            other => store_refusal(other),
        })?;
    let documents = MetaDocuments::open(Arc::clone(&realm), IMPORT_LABEL)
        .await
        .map_err(store_refusal)?;
    let runtime = SpiRuntimeStore::open(Arc::clone(&realm))
        .await
        .map_err(store_refusal)?;
    let mob_storages = meerkat_mob::store::SpiMobStoreHost::for_realm(&realm)
        .await
        .map_err(store_refusal)?
        .stored_mobs()
        .len();
    Ok(ImportRealm {
        _realm: realm,
        documents,
        runtime,
        mob_storages,
    })
}

/// How the realm takes this import.
enum Admission {
    /// No import began here: the apply writes the realm record first.
    New,
    /// This export's import began here and has not finished.
    Resume,
    /// This export's import finished here; nothing is left to write.
    Done,
}

/// Admit the realm: refuse a realm a runtime used and one another export's
/// import began.
async fn admit(
    realm: &ImportRealm,
    entries_sha256: &str,
    sessions: &BTreeSet<SessionId>,
) -> Result<Admission, HostStoreImportRefusal> {
    let record = match read_realm_record(&realm.documents).await {
        Ok(record) => record,
        Err(RealmRecordError::Newer { found }) => {
            return Err(HostStoreImportRefusal::RealmFormat {
                found,
                supported: REALM_RECORD_VERSION,
            });
        }
        Err(error) => return Err(store_refusal(error)),
    };
    if realm.mob_storages > 0 {
        return Err(HostStoreImportRefusal::RealmInUse {
            detail: "it holds mob storage, which a runtime creates at its first start".to_string(),
        });
    }
    let admission = match record {
        None => Admission::New,
        Some(record) => match record.import {
            None => {
                return Err(HostStoreImportRefusal::RealmInUse {
                    detail: "a runtime created its realm record".to_string(),
                });
            }
            Some(import) if import.entries_sha256 != entries_sha256 => {
                return Err(HostStoreImportRefusal::OtherExport {
                    recorded: import.entries_sha256,
                    this: entries_sha256.to_string(),
                });
            }
            Some(import) => match import.state {
                HostStoreImportState::InProgress => Admission::Resume,
                HostStoreImportState::Done => Admission::Done,
            },
        },
    };
    let catalog = realm
        .runtime
        .list_runtime_session_catalog_entries(meerkat_core::SessionFilter::default())
        .await
        .map_err(store_refusal)?;
    if let Some(entry) = catalog
        .iter()
        .find(|entry| !sessions.contains(entry.session_id()))
    {
        return Err(HostStoreImportRefusal::RealmInUse {
            detail: format!(
                "it holds session {}, which the export does not carry",
                entry.session_id()
            ),
        });
    }
    Ok(admission)
}

/// Compare each prepared document with the one the realm holds for its
/// session.
async fn compare_with_realm(
    realm: &ImportRealm,
    entries: &[CensusEntry],
    rows: &mut [HostStoreImportRow],
) -> Result<(), HostStoreImportRefusal> {
    for (entry, row) in entries.iter().zip(rows.iter_mut()) {
        let Some(digest) = entry.document_sha256 else {
            continue;
        };
        let committed = realm
            .runtime
            .load_committed_whole_blob_snapshot(&LogicalRuntimeId::for_session(&entry.session_id))
            .await
            .map_err(store_refusal)?;
        if let Some(committed) = committed {
            if sha256(committed.bytes()) == digest {
                row.class = HostStoreImportClass::Kept;
            } else {
                row.class = HostStoreImportClass::DifferentRow;
                row.detail = Some(format!(
                    "the realm holds another document for the session ({} bytes)",
                    committed.bytes().len()
                ));
            }
        }
    }
    Ok(())
}

/// Import a continuity export into a host store realm (see the module
/// documentation). Every outcome, a refusal included, is in the report.
pub async fn import_continuity_export_into_host_store(
    request: &HostStoreImportRequest,
) -> HostStoreImportReport {
    let mut report = HostStoreImportReport::new(request.mode);
    if request.realm_config.acquire != AcquireMode::Exclusive {
        return report.refused(HostStoreImportRefusal::TakeoverRefused);
    }

    // 1. The export, each ready entry's document prepared once.
    let path = request.continuity_export.clone();
    let read = match tokio::task::spawn_blocking(move || read_export(&path)).await {
        Ok(Ok(read)) => read,
        Ok(Err(detail)) => return report.refused(HostStoreImportRefusal::Export { detail }),
        Err(error) => return report.refused(store_refusal(format!("the export read: {error}"))),
    };
    let ExportRead {
        file,
        entries,
        mut rows,
    } = read;
    report.export = Some(HostStoreImportExport {
        path: request.continuity_export.clone(),
        writer: file.writer.clone(),
        entries_sha256: file.entries_sha256.clone(),
        entries: file.entries.len(),
        ready: entries.len(),
    });
    report.uninitialized_identities = file
        .entries
        .iter()
        .filter(|entry| entry.record.is_none())
        .map(|entry| entry.identity.clone())
        .collect();
    let claims: Vec<IdentityClaim> = file
        .entries
        .iter()
        .map(|entry| IdentityClaim {
            identity: entry.identity.clone(),
            session: entry
                .record
                .as_ref()
                .map(|record| record.session_id.clone()),
            values_093: entry
                .record
                .as_ref()
                .map(super::export_file::ExportRecord::values_093),
        })
        .collect();
    // A new realm holds no mob, so the in-memory rule applies: a Resume row
    // per ready entry.
    let identities = identity_census(&claims, ClaimSource::ExportFile, None, &BTreeSet::new());
    report.first_boot_rows.clone_from(&identities.first_boot);
    let sessions: BTreeSet<SessionId> = entries
        .iter()
        .map(|entry| entry.session_id.clone())
        .collect();

    // 2. and 3. The realm, held for the whole run, and its admission.
    let realm = match open_realm(request).await {
        Ok(realm) => realm,
        Err(refusal) => {
            report.sessions = rows;
            return report.refused(refusal);
        }
    };
    let admission = match admit(&realm, &file.entries_sha256, &sessions).await {
        Ok(admission) => admission,
        Err(refusal) => {
            report.sessions = rows;
            return report.refused(refusal);
        }
    };

    // 4. The census.
    if let Err(refusal) = compare_with_realm(&realm, &entries, &mut rows).await {
        report.sessions = rows;
        return report.refused(refusal);
    }
    let refused = rows
        .iter()
        .filter(|row| {
            matches!(
                row.class,
                HostStoreImportClass::DifferentRow | HostStoreImportClass::Invalid
            )
        })
        .count()
        + identities.refused();
    let manifest_rows = match identities
        .first_boot
        .iter()
        .map(FirstBootRowPlan::to_manifest_row)
        .collect::<Result<Vec<FirstBootRow>, String>>()
    {
        Ok(manifest_rows) => manifest_rows,
        Err(detail) => {
            report.sessions = rows;
            return report.refused(store_refusal(detail));
        }
    };
    if refused > 0 {
        report.sessions = rows;
        return report.refused(HostStoreImportRefusal::RowsRefused { count: refused });
    }
    if matches!(admission, Admission::Done) {
        // A finished import holds every session; a rerun only reports it.
        if let Some(row) = rows
            .iter()
            .find(|row| row.class != HostStoreImportClass::Kept)
        {
            let detail = format!(
                "the realm records this import done, yet it does not hold session {}",
                row.session_id
            );
            report.sessions = rows;
            return report.refused(store_refusal(detail));
        }
        report.sessions = rows;
        report.first_boot_written = true;
        report.completed = true;
        return report;
    }
    if matches!(request.mode, MigrateMode::DryRun) {
        report.sessions = rows;
        return report;
    }

    // 5. The apply.
    let outcome = apply(
        request,
        &realm,
        &file,
        &entries,
        &mut rows,
        manifest_rows,
        matches!(admission, Admission::New),
    )
    .await;
    report.sessions = rows;
    match outcome {
        Ok(()) => {
            report.first_boot_written = true;
            report.completed = true;
            report
        }
        Err(refusal) => report.refused(refusal),
    }
}

/// Step 5: the realm record, each session, the first-boot rows, the import
/// done. Each write is its own batch.
async fn apply(
    request: &HostStoreImportRequest,
    realm: &ImportRealm,
    file: &ExportFile,
    entries: &[CensusEntry],
    rows: &mut [HostStoreImportRow],
    manifest_rows: Vec<FirstBootRow>,
    new_realm: bool,
) -> Result<(), HostStoreImportRefusal> {
    let first_boot = FirstBootManifest::new(manifest_rows.clone())
        .map_err(|refusal| store_refusal(format!("the first-boot rows: {refusal}")))?;
    let rows_sha256 = first_boot.rows_sha256().to_string();

    // a. From here a runtime refuses to open the realm until the import is
    // done, so a half-imported realm never starts identities fresh.
    if new_realm {
        let record = HostStoreRealmRecord::new(
            DEFAULT_MEERKAT_LEVEL_REALM_ID,
            Some(HostStoreImportRecord {
                entries_sha256: file.entries_sha256.clone(),
                state: HostStoreImportState::InProgress,
                sessions: entries.len() as u64,
                rows_sha256: rows_sha256.clone(),
            }),
        );
        let encoded = encode_realm_record(&record).map_err(store_refusal)?;
        realm
            .documents
            .update(REALM_RECORD_KEY, |current| match current {
                None => Ok((DocumentChange::Replace(encoded), ())),
                // Nothing else writes the realm while the import holds it.
                Some(_) => Err(MetaDocumentError::Codec {
                    key: REALM_RECORD_KEY.to_string(),
                    detail: "a realm record appeared while the import held the realm".to_string(),
                }),
            })
            .await
            .map_err(store_refusal)?;
    }

    // b. Each session: its images, then its document, each its own batches.
    let blobs = Base64BlobStoreAdapter::new(Arc::clone(&request.blob_store));
    for (entry, row) in entries.iter().zip(rows.iter_mut()) {
        let path = request.continuity_export.clone();
        let offset = entry.offset;
        let reread = tokio::task::spawn_blocking(move || read_entry_at(&path, offset))
            .await
            .map_err(|error| store_refusal(format!("the export read: {error}")))?
            .map_err(|error| HostStoreImportRefusal::Export {
                detail: error.to_string(),
            })?;
        let (_, Some(raw)) = reread else {
            return Err(HostStoreImportRefusal::Export {
                detail: format!("the entry at byte {offset} carries no document any more"),
            });
        };
        let prepared = prepare_document(&entry.session_id, &raw)
            .map_err(|detail| HostStoreImportRefusal::Export { detail })?;
        if Some(sha256(&prepared.bytes)) != entry.document_sha256 {
            return Err(HostStoreImportRefusal::Export {
                detail: format!(
                    "session {}'s document differs from the one the census read",
                    entry.session_id
                ),
            });
        }
        for image in &prepared.images {
            let stored = blobs
                .put_image(&image.media_type, &image.data)
                .await
                .map_err(|error| {
                    store_refusal(format!("store image {}: {error}", image.blob_id.as_str()))
                })?;
            if stored.blob_id != image.blob_id {
                return Err(store_refusal(format!(
                    "the blob store addressed image {} as {}",
                    image.blob_id.as_str(),
                    stored.blob_id.as_str()
                )));
            }
        }
        let imported = realm
            .runtime
            .import_whole_blob_session(
                &LogicalRuntimeId::for_session(&entry.session_id),
                meerkat_runtime::store::SerializedSessionSnapshot {
                    session_snapshot: Arc::new(prepared.bytes),
                },
            )
            .await
            .map_err(|error| store_refusal(format!("session {}: {error}", entry.session_id)))?;
        row.class = match imported {
            meerkat_runtime::WholeBlobSessionImport::Created(_) => HostStoreImportClass::Created,
            meerkat_runtime::WholeBlobSessionImport::AlreadyCurrent(_) => {
                HostStoreImportClass::Kept
            }
        };
    }

    // c. The first-boot rows, once.
    if !manifest_rows.is_empty() {
        match FirstBootManifest::write_new_host_store(&realm.documents, manifest_rows).await {
            Ok(_) => {}
            Err(error) if matches!(error.refusal(), FirstBootManifestRefusal::AlreadyExists) => {
                let written = FirstBootManifest::read_host_store(&realm.documents)
                    .await
                    .map_err(store_refusal)?;
                if written.as_ref().map(FirstBootManifest::rows_sha256)
                    != Some(rows_sha256.as_str())
                {
                    return Err(store_refusal(
                        "the realm holds first-boot rows other than this export's",
                    ));
                }
            }
            Err(error) => return Err(store_refusal(error)),
        }
    }

    // d. The import done.
    realm
        .documents
        .update(REALM_RECORD_KEY, |current| {
            let codec = |detail: String| MetaDocumentError::Codec {
                key: REALM_RECORD_KEY.to_string(),
                detail,
            };
            let mut record = decode_realm_record(
                current.ok_or_else(|| codec("the realm record is gone".to_string()))?,
            )
            .map_err(|error| codec(error.to_string()))?;
            let Some(import) = record.import.as_mut() else {
                return Err(codec("the realm record names no import".to_string()));
            };
            import.state = HostStoreImportState::Done;
            let encoded = encode_realm_record(&record)?;
            Ok((DocumentChange::Replace(encoded), ()))
        })
        .await
        .map_err(store_refusal)?;
    Ok(())
}
