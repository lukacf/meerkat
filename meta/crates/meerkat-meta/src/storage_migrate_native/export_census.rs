//! `meta_gateway storage-migrate-native --continuity-export <file>`: the
//! census of a host whose sessions a continuity provider held. The export
//! file the standalone scripts wrote from that provider is the projection
//! side.
//!
//! The state directory decides the mode:
//! - new: the directory does not exist, or holds no store file. `--apply`
//!   creates the native pair, and every ready entry is `RuntimeMissing`,
//!   imported through I1-I4.
//! - in place: the directory holds `runtime.sqlite` and no local projection
//!   file: a host that kept its runtime store on a local volume and its
//!   sessions in the provider. The export is compared with the committed
//!   sessions exactly as a local projection is: Core ahead is
//!   `ProjectionBehind`, export-only is `RuntimeMissing`, Core-only is
//!   `ProjectionMissing`, and the export ahead or divergent is refused.
//!
//! Refused typed: a local projection file (a second projection side), store
//! files without `runtime.sqlite`, and a directory 0.9.4 already uses.
//!
//! Identities follow the directory's mob storage, as in the local census:
//! the durable rules against Core's last binding when the directory has a
//! mob store, the in-memory rules (a first-boot Resume row per ready entry)
//! when it has none.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use meerkat::SessionStore;
use meerkat_core::types::SessionId;
use meerkat_core::{Session, SessionFilter, SessionMeta, SessionStoreError};
use serde::Serialize;

use super::apply::{ApplyJournal, ApplyPlan, DiscardedRuntime, abandon_journal};
use super::census::{
    CensusMobStore, GatewayStateShape, IdentityCensusStatus, MOB_STORE_SCRATCH_NAME,
    NativeMigrateRefusal, NativeMigrateReport, NativeMigrateRequest, Projection, RuntimeSessions,
    SessionCensusClass, SessionCensusRow, classify_session, copy_database, discarded_bodiless,
    finish_apply, hold_directory, open_runtime_sessions, predict_import_rows, resolve_mob_storage,
    run_journal, runtime_missing_sessions,
};
use super::export_file::{ExportFile, read_entry_at, read_export_file};
use super::identity::{ClaimSource, IdentityClaim, identity_census};
use super::import::{ImportSource, decode_session_document};
use super::layout_status::{NativeLayoutStatus, native_layout_status, runtime_conversion_began};
use crate::storage_layout::{DatabaseSlot, EVENT_LOG_DB_FILE_NAME, RUNTIME_DB_FILE_NAME};
use crate::storage_migrate::MigrateMode;

/// The name of the export file's copy in the scratch directory.
const EXPORT_SCRATCH_NAME: &str = "continuity-export.jsonl";

/// Where an export's sessions go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContinuityExportMode {
    /// A new state directory: `--apply` creates the native pair.
    New,
    /// The host's own state directory, which holds its `runtime.sqlite`.
    InPlace,
}

/// The export file a run read.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct ContinuityExportSummary {
    pub path: PathBuf,
    pub mode: ContinuityExportMode,
    pub writer: String,
    /// The digest of the entry lines; the apply journal records it, and a
    /// rerun with a different file refuses.
    pub entries_sha256: String,
    pub entries: usize,
    pub ready: usize,
}

/// Run the census of `request`, whose projection side is the export file
/// `export`.
pub(super) async fn census_continuity_export(
    request: &NativeMigrateRequest,
    export: &Path,
) -> NativeMigrateReport {
    let state_dir = request.state_dir.as_path();
    let mut report = NativeMigrateReport::new(request.mode, state_dir);
    let mut restarted = false;
    if state_dir.is_dir()
        && let NativeLayoutStatus::MigrationIncomplete { journal } = native_layout_status(state_dir)
    {
        report.layout = Some(NativeLayoutStatus::MigrationIncomplete {
            journal: journal.clone(),
        });
        if !matches!(request.mode, MigrateMode::Apply) {
            report
                .refusals
                .push(NativeMigrateRefusal::MigrationIncomplete { journal });
            return report;
        }
        // As in the local census: only Core's durable claim in runtime.sqlite
        // resumes the run (0.9.3 refuses that store, so nothing wrote the
        // directory since), from a journal this build can read. Without the
        // claim the journal is dropped, readable or not (its backups stay),
        // and the run starts over with a new census.
        match runtime_conversion_began(state_dir) {
            Ok(true) => {
                finish_export_apply(state_dir, export, &mut report).await;
                return report;
            }
            Ok(false) => {
                if let Err(detail) = abandon_journal(state_dir) {
                    report
                        .refusals
                        .push(NativeMigrateRefusal::ApplyFailed { detail });
                    return report;
                }
                restarted = true;
                report.layout = None;
            }
            Err(detail) => {
                report
                    .refusals
                    .push(NativeMigrateRefusal::RuntimeStoreUnreadable { detail });
                return report;
            }
        }
    }
    let (mode, layout) = match export_directory_mode(state_dir) {
        Ok(mode) => mode,
        Err(refusal) => {
            report.refusals.push(refusal);
            return report;
        }
    };
    // A new directory has nothing the startup predicate could report; the
    // export is what migrates.
    if mode == ContinuityExportMode::InPlace {
        report.layout = layout;
    }
    report.shape = Some(match mode {
        ContinuityExportMode::New => GatewayStateShape::ContinuityExportNew,
        ContinuityExportMode::InPlace => GatewayStateShape::ContinuityExportInPlace,
    });
    let mob_storage = match (state_dir.is_dir(), request.mob_store.as_deref()) {
        (true, named) => resolve_mob_storage(state_dir, named),
        (false, Some(named)) if named.is_file() => Ok(CensusMobStore::Durable {
            path: named.to_path_buf(),
        }),
        (false, Some(named)) => Err(NativeMigrateRefusal::MobStoreMissing {
            path: named.to_path_buf(),
        }),
        (false, None) => Ok(CensusMobStore::InMemory),
    };
    let mob_storage = match mob_storage {
        Ok(mob_storage) => mob_storage,
        Err(refusal) => {
            report.refusals.push(refusal);
            return report;
        }
    };
    report.mob_storage = Some(mob_storage.clone());

    let scratch_parent = match (&request.scratch_dir, state_dir.parent()) {
        (Some(dir), _) => dir.clone(),
        (None, Some(parent)) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        (None, _) => PathBuf::from("."),
    };
    let scratch = match tempfile::Builder::new()
        .prefix(".meta-storage-migrate-native-")
        .tempdir_in(&scratch_parent)
    {
        Ok(scratch) => scratch,
        Err(error) => {
            report
                .refusals
                .push(NativeMigrateRefusal::ScratchCopyFailed {
                    file: String::new(),
                    detail: format!("scratch directory in {}: {error}", scratch_parent.display()),
                });
            return report;
        }
    };
    let export_copy = scratch.path().join(EXPORT_SCRATCH_NAME);
    if let Err(error) = std::fs::copy(export, &export_copy) {
        report
            .refusals
            .push(NativeMigrateRefusal::ContinuityExportUnreadable {
                detail: format!("{}: {error}", export.display()),
            });
        return report;
    }
    let mob_copy = match &mob_storage {
        CensusMobStore::Durable { .. } => Some(scratch.path().join(MOB_STORE_SCRATCH_NAME)),
        CensusMobStore::InMemory => None,
    };
    // As in the local census: the runtime locks keep a runtime from starting
    // while the copies are taken, and the fence drains the operations already
    // in flight; an apply run holds both to its last step. A directory that
    // does not exist yet has neither.
    let held = if state_dir.is_dir() {
        match hold_directory(state_dir).await {
            Ok(held) => Some(held),
            Err(refusal) => {
                report.refusals.push(refusal);
                return report;
            }
        }
    } else {
        None
    };
    if let Err(refusal) = copy_stores(
        state_dir,
        scratch.path(),
        mode,
        &mob_storage,
        mob_copy.as_deref(),
    ) {
        report.refusals.push(refusal);
        return report;
    }
    let held = match request.mode {
        MigrateMode::Apply => held,
        _ => {
            drop(held);
            None
        }
    };

    // The file is read from the copy, so the census and the apply of one
    // run see the same bytes.
    let projection = match ExportProjection::read(&export_copy) {
        Ok(projection) => projection,
        Err(detail) => {
            report
                .refusals
                .push(NativeMigrateRefusal::ContinuityExportUnreadable { detail });
            return report;
        }
    };
    report.continuity_export = Some(ContinuityExportSummary {
        path: export.to_path_buf(),
        mode,
        writer: projection.file.writer.clone(),
        entries_sha256: projection.file.entries_sha256.clone(),
        entries: projection.file.entries.len(),
        ready: projection.locations.len(),
    });
    let runtime = match mode {
        ContinuityExportMode::New => RuntimeSessions::empty(),
        ContinuityExportMode::InPlace => {
            match open_runtime_sessions(&scratch.path().join(RUNTIME_DB_FILE_NAME)).await {
                Ok(runtime) => runtime,
                Err(refusal) => {
                    report.refusals.push(refusal);
                    return report;
                }
            }
        }
    };
    let shape = report
        .shape
        .unwrap_or(GatewayStateShape::ContinuityExportNew);
    let census_projection = projection.census_projection();
    let session_ids: BTreeSet<String> = runtime
        .known_session_ids()
        .chain(census_projection.session_ids.iter().cloned())
        .collect();
    for session_id in &session_ids {
        let in_projection = census_projection.session_ids.contains(session_id);
        let identities = census_projection
            .bindings
            .get(session_id)
            .cloned()
            .unwrap_or_default();
        let (mut class, mut detail) = classify_session(
            session_id,
            &runtime,
            in_projection,
            &census_projection.store,
            shape,
            !identities.is_empty(),
        )
        .await;
        if class == SessionCensusClass::BodilessRuntime
            && request.discard_bodyless.contains(session_id)
        {
            (class, detail) =
                discarded_bodiless(session_id, in_projection, &census_projection.store, detail)
                    .await;
        }
        report.sessions.push(SessionCensusRow::new(
            session_id.clone(),
            class,
            identities,
            detail,
            runtime.runtime_state(session_id),
            // The 0.9.3 host whose provider wrote the export is not this
            // directory's to restart.
            false,
        ));
    }
    // A new directory has no session store yet: every import creates its
    // row, and each is still imported for trial on a scratch store.
    let runtime_copy =
        (mode == ContinuityExportMode::InPlace).then(|| scratch.path().join(RUNTIME_DB_FILE_NAME));
    if let Err(detail) = predict_import_rows(
        runtime_copy.as_deref(),
        scratch.path(),
        &census_projection.store,
        &mut report.sessions,
    )
    .await
    {
        report
            .refusals
            .push(NativeMigrateRefusal::RuntimeStoreUnreadable { detail });
        return report;
    }
    for session_id in &request.discard_bodyless {
        if !runtime.bodiless_runtimes().contains_key(session_id) {
            report
                .refusals
                .push(NativeMigrateRefusal::DiscardNotBodiless {
                    session_id: session_id.clone(),
                });
        }
    }
    let refused = report.sessions.iter().filter(|row| !row.admitted).count();
    if refused != 0 {
        report
            .refusals
            .push(NativeMigrateRefusal::SessionsRefused { count: refused });
    }

    let history = match &mob_copy {
        Some(copy) => match meerkat_mob::MobStorage::read_member_binding_history(copy).await {
            Ok(history) => Some(history),
            Err(error) => {
                report
                    .refusals
                    .push(NativeMigrateRefusal::MobStorageUnreadable {
                        detail: error.to_string(),
                    });
                return report;
            }
        },
        None => None,
    };
    let known_sessions: BTreeSet<SessionId> = session_ids
        .iter()
        .filter_map(|session_id| SessionId::parse(session_id).ok())
        .collect();
    let census = identity_census(
        &projection.claims(),
        ClaimSource::ExportFile,
        history.as_ref(),
        &known_sessions,
    );
    let refused = census.refused();
    report.identities = census.rows;
    report.first_boot = census.first_boot;
    report.identity_census = IdentityCensusStatus::Ran;
    if refused != 0 {
        report
            .refusals
            .push(NativeMigrateRefusal::IdentitiesRefused { count: refused });
    }
    if matches!(request.mode, MigrateMode::Apply) && report.refusals.is_empty() {
        let meta_lock_held = held
            .as_ref()
            .is_some_and(|held| held.holds_meta_runtime_lock(state_dir));
        let discard = runtime
            .bodiless_runtimes()
            .iter()
            .filter(|(session_id, _)| request.discard_bodyless.contains(*session_id))
            .flat_map(|(session_id, runtime_ids)| {
                runtime_ids.iter().map(|runtime_id| DiscardedRuntime {
                    session_id: session_id.clone(),
                    runtime_id: runtime_id.clone(),
                })
            })
            .collect();
        apply_export(
            state_dir,
            export,
            mode,
            &projection,
            discard,
            meta_lock_held,
            &mut report,
        )
        .await;
        if let Some(applied) = report.applied.as_mut() {
            applied.restarted = restarted;
        }
    }
    drop(held);
    report
}

/// Carry out a clean export census: journal the plan (the export's absolute
/// path and entries digest with it), then run it. A new directory is
/// created first, to hold the journal; the native pair comes after it.
async fn apply_export(
    state_dir: &Path,
    export: &Path,
    mode: ContinuityExportMode,
    projection: &ExportProjection,
    discard: Vec<DiscardedRuntime>,
    meta_lock_held: bool,
    report: &mut NativeMigrateReport,
) {
    let path = match std::fs::canonicalize(export) {
        Ok(path) => path,
        Err(error) => {
            report
                .refusals
                .push(NativeMigrateRefusal::ContinuityExportUnreadable {
                    detail: format!("{}: {error}", export.display()),
                });
            return;
        }
    };
    if mode == ContinuityExportMode::New
        && let Err(error) = std::fs::create_dir_all(state_dir)
    {
        report.refusals.push(NativeMigrateRefusal::ApplyFailed {
            detail: format!("create {}: {error}", state_dir.display()),
        });
        return;
    }
    let plan = ApplyPlan {
        projections: Vec::new(),
        imports: runtime_missing_sessions(report),
        import_source: Some(ImportSource::Export {
            path,
            entries_sha256: projection.file.entries_sha256.clone(),
        }),
        first_boot: report.first_boot.clone(),
        new_directory: mode == ContinuityExportMode::New,
        discard,
    };
    match ApplyJournal::start(state_dir, plan) {
        Ok(journal) => run_journal(state_dir, journal, false, meta_lock_held, report).await,
        Err(detail) => report
            .refusals
            .push(NativeMigrateRefusal::ApplyFailed { detail }),
    }
}

/// `--apply` over a directory an earlier export run journaled: finish that
/// run, but only with the file it started with.
async fn finish_export_apply(state_dir: &Path, export: &Path, report: &mut NativeMigrateReport) {
    let journal = match ApplyJournal::read(state_dir) {
        Ok(journal) => journal,
        Err(detail) => {
            report
                .refusals
                .push(NativeMigrateRefusal::MigrationJournalUnreadable { detail });
            return;
        }
    };
    let Some((journaled_path, journaled_sha256)) = journal.export_source() else {
        report
            .refusals
            .push(NativeMigrateRefusal::ContinuityExportChanged {
                detail: "the unfinished run did not import a continuity export: run --apply \
                         without --continuity-export to finish it"
                    .to_owned(),
            });
        return;
    };
    // The run imports from the file its journal names: the file passed now
    // must be that one, so the entries verified here are the entries read.
    let passed = std::fs::canonicalize(export).unwrap_or_else(|_| export.to_path_buf());
    if passed != journaled_path {
        report
            .refusals
            .push(NativeMigrateRefusal::ContinuityExportChanged {
                detail: format!(
                    "the unfinished run imports {}, not {}: pass that file (or put it back \
                     there) to finish the run",
                    journaled_path.display(),
                    export.display()
                ),
            });
        return;
    }
    let file = match ExportProjection::read(export) {
        Ok(projection) => projection.file,
        Err(detail) => {
            report
                .refusals
                .push(NativeMigrateRefusal::ContinuityExportUnreadable { detail });
            return;
        }
    };
    if file.entries_sha256 != journaled_sha256 {
        report
            .refusals
            .push(NativeMigrateRefusal::ContinuityExportChanged {
                detail: format!(
                    "{} has entries_sha256 {}, but the unfinished run imports {journaled_sha256}: \
                     pass the file the run started with",
                    export.display(),
                    file.entries_sha256
                ),
            });
        return;
    }
    finish_apply(state_dir, report).await;
}

/// The export mode the state directory allows, with the startup predicate's
/// verdict when the directory exists.
fn export_directory_mode(
    state_dir: &Path,
) -> Result<(ContinuityExportMode, Option<NativeLayoutStatus>), NativeMigrateRefusal> {
    if !state_dir.exists() {
        return Ok((ContinuityExportMode::New, None));
    }
    if !state_dir.is_dir() {
        return Err(NativeMigrateRefusal::NotADirectory);
    }
    let layout = native_layout_status(state_dir);
    if let NativeLayoutStatus::MigrationIncomplete { journal } = &layout {
        return Err(NativeMigrateRefusal::MigrationIncomplete {
            journal: journal.clone(),
        });
    }
    let projection_files: Vec<&str> = [DatabaseSlot::Sessions, DatabaseSlot::Continuity]
        .into_iter()
        .flat_map(|slot| {
            std::iter::once(slot.canonical_name()).chain(slot.legacy_names().iter().copied())
        })
        .filter(|file| state_dir.join(file).exists())
        .collect();
    if let Some(file) = projection_files.first() {
        return Err(NativeMigrateRefusal::ContinuityExportDirectory {
            detail: format!(
                "the directory holds the local projection file {file}: an export is the \
                 projection side of a directory without one; migrate this directory without \
                 --continuity-export"
            ),
        });
    }
    let runtime = state_dir.join(RUNTIME_DB_FILE_NAME).exists();
    if runtime {
        if matches!(layout, NativeLayoutStatus::Native) {
            // A file Core never claimed holds no runtime to keep: what a new
            // directory's run left when it stopped before the pin, or an empty
            // 0.9.3 store. It migrates in place. A claimed one belongs to a
            // directory 0.9.4 already uses.
            return match runtime_conversion_began(state_dir) {
                Ok(false) => Ok((ContinuityExportMode::InPlace, None)),
                Ok(true) => Err(NativeMigrateRefusal::ContinuityExportDirectory {
                    detail: "the directory's runtime.sqlite is already in the native layout: \
                             an export imports into a new directory or into the 0.9.3 host's \
                             own directory"
                        .to_owned(),
                }),
                Err(detail) => Err(NativeMigrateRefusal::RuntimeStoreUnreadable { detail }),
            };
        }
        return Ok((ContinuityExportMode::InPlace, Some(layout)));
    }
    if state_dir.join(EVENT_LOG_DB_FILE_NAME).exists() {
        return Err(NativeMigrateRefusal::ContinuityExportDirectory {
            detail: format!(
                "the directory holds {EVENT_LOG_DB_FILE_NAME} without runtime.sqlite: pass the \
                 host's own state directory, or a new one"
            ),
        });
    }
    if let NativeLayoutStatus::NeedsMigration { reasons } = &layout {
        return Err(NativeMigrateRefusal::ContinuityExportDirectory {
            detail: format!("the directory needs a migration of its own: {reasons:?}"),
        });
    }
    Ok((ContinuityExportMode::New, Some(layout)))
}

/// Copy the stores the census reads into `scratch`: `runtime.sqlite` in
/// place, and the durable mob store to `mob_copy`.
fn copy_stores(
    state_dir: &Path,
    scratch: &Path,
    mode: ContinuityExportMode,
    mob_storage: &CensusMobStore,
    mob_copy: Option<&Path>,
) -> Result<(), NativeMigrateRefusal> {
    if mode == ContinuityExportMode::InPlace {
        copy_database(
            state_dir,
            scratch,
            RUNTIME_DB_FILE_NAME,
            RUNTIME_DB_FILE_NAME,
        )
        .map_err(|detail| NativeMigrateRefusal::ScratchCopyFailed {
            file: RUNTIME_DB_FILE_NAME.to_owned(),
            detail,
        })?;
    }
    if let (CensusMobStore::Durable { path }, Some(target)) = (mob_storage, mob_copy) {
        copy_with_wal(path, target).map_err(|detail| NativeMigrateRefusal::ScratchCopyFailed {
            file: path.display().to_string(),
            detail,
        })?;
    }
    Ok(())
}

/// Copy a database file and its write-ahead log to `target`.
pub(super) fn copy_with_wal(source: &Path, target: &Path) -> Result<(), String> {
    std::fs::copy(source, target).map_err(|error| format!("{}: {error}", source.display()))?;
    let mut source_wal = source.as_os_str().to_owned();
    source_wal.push("-wal");
    let source_wal = PathBuf::from(source_wal);
    if source_wal.is_file() {
        let mut target_wal = target.as_os_str().to_owned();
        target_wal.push("-wal");
        std::fs::copy(&source_wal, PathBuf::from(target_wal))
            .map_err(|error| format!("{}: {error}", source_wal.display()))?;
    }
    Ok(())
}

/// A verified export file and where each ready entry's line starts.
pub(super) struct ExportProjection {
    pub(super) path: PathBuf,
    pub(super) file: ExportFile,
    /// Each ready entry's session: its line's offset and its identity.
    pub(super) locations: BTreeMap<SessionId, EntryLocation>,
}

/// Where a ready entry's line starts, and what the file pass read there.
#[derive(Debug, Clone)]
pub(super) struct EntryLocation {
    pub(super) offset: u64,
    pub(super) identity: String,
    pub(super) document_sha256: String,
}

impl ExportProjection {
    /// Verify the file at `path` and record where each ready entry is.
    pub(super) fn read(path: &Path) -> Result<Self, String> {
        let mut locations = BTreeMap::new();
        let file = read_export_file(path, |entry, offset, _document| {
            if let Some(record) = &entry.record {
                locations.insert(
                    record.session_id.clone(),
                    EntryLocation {
                        offset,
                        identity: entry.identity.clone(),
                        document_sha256: record.document_sha256.clone(),
                    },
                );
            }
        })
        .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            locations,
        })
    }

    /// The identity census's claims: one per entry.
    pub(super) fn claims(&self) -> Vec<IdentityClaim> {
        self.file
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
            .collect()
    }

    /// The census's projection side: the ready entries' sessions, each bound
    /// to its entry's identity, read through a store that loads one
    /// session at a time from the file.
    pub(super) fn census_projection(&self) -> Projection {
        Projection {
            store: Arc::new(ExportSessionStore {
                path: self.path.clone(),
                locations: Arc::new(self.locations.clone()),
            }),
            session_ids: self.locations.keys().map(ToString::to_string).collect(),
            bindings: self
                .locations
                .iter()
                .map(|(session_id, location)| {
                    (session_id.to_string(), vec![location.identity.clone()])
                })
                .collect(),
        }
    }
}

fn load_entry(
    path: &Path,
    locations: &BTreeMap<SessionId, EntryLocation>,
    id: &SessionId,
) -> Result<Option<Session>, String> {
    let Some(location) = locations.get(id) else {
        return Ok(None);
    };
    let (entry, document) = read_entry_at(path, location.offset)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let same = entry.identity == location.identity
        && entry.record.as_ref().is_some_and(|record| {
            &record.session_id == id && record.document_sha256 == location.document_sha256
        });
    let Some(document) = document.filter(|_| same) else {
        return Err(format!(
            "{}: the entry for session {id} changed after the file was verified",
            path.display()
        ));
    };
    decode_session_document(id, &document)
        .map(Some)
        .map_err(|detail| {
            format!(
                "the session document exported for identity {} does not load: {detail}",
                location.identity
            )
        })
}

/// The export's ready entries as a read-only session store: `load` reads one
/// entry's line again and decodes its document. Nothing writes through it.
struct ExportSessionStore {
    path: PathBuf,
    /// Shared with each load's blocking task, so a load clones no map.
    locations: Arc<BTreeMap<SessionId, EntryLocation>>,
}

impl ExportSessionStore {
    fn read_only() -> SessionStoreError {
        SessionStoreError::Internal("the continuity export is read-only".to_owned())
    }
}

#[async_trait::async_trait]
impl SessionStore for ExportSessionStore {
    async fn save(&self, _session: &Session) -> Result<(), SessionStoreError> {
        Err(Self::read_only())
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
        let path = self.path.clone();
        let locations = Arc::clone(&self.locations);
        let id = id.clone();
        tokio::task::spawn_blocking(move || load_entry(&path, &locations, &id))
            .await
            .map_err(|error| SessionStoreError::Internal(format!("export read task: {error}")))?
            .map_err(SessionStoreError::Internal)
    }

    async fn list(&self, _filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
        Err(SessionStoreError::Internal(
            "the continuity export is read by session id only".to_owned(),
        ))
    }

    async fn delete(&self, _id: &SessionId) -> Result<(), SessionStoreError> {
        Err(Self::read_only())
    }

    async fn delete_if_current_revision(
        &self,
        _id: &SessionId,
        _expected_current_revision: &str,
    ) -> Result<bool, SessionStoreError> {
        Err(Self::read_only())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::storage_migrate_native::{NativeMigrateRequest, migrate_state_dir_native};

    /// The in-memory remote capture's export, unpacked into `dir`.
    fn memory_export(dir: &Path) -> PathBuf {
        let export = dir.join("export.jsonl");
        let source = std::fs::File::open(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/released_0_9_3_h_remote/memory-export.jsonl.gz"),
        )
        .expect("fixture");
        let mut out = std::fs::File::create(&export).expect("export");
        std::io::copy(&mut flate2::read::GzDecoder::new(source), &mut out).expect("gunzip");
        export
    }

    /// A stopped export run that journaled no conversion start, over a
    /// directory with no runtime.sqlite yet: the journal is dropped and the
    /// run starts over with a new census, importing every ready entry.
    #[tokio::test]
    async fn an_export_run_stopped_before_the_conversion_starts_over() {
        let scratch = tempfile::tempdir().expect("scratch");
        let export = memory_export(scratch.path());
        let state = scratch.path().join("state");
        std::fs::create_dir(&state).expect("state dir");
        let digest = ExportProjection::read(&export)
            .expect("export reads")
            .file
            .entries_sha256;
        ApplyJournal::start(
            &state,
            ApplyPlan {
                import_source: Some(ImportSource::Export {
                    path: export.clone(),
                    entries_sha256: digest,
                }),
                new_directory: true,
                ..ApplyPlan::default()
            },
        )
        .expect("journal");

        let report = migrate_state_dir_native(
            &NativeMigrateRequest::new(&state, MigrateMode::Apply).with_continuity_export(&export),
        )
        .await;
        assert!(report.refusals.is_empty(), "{report:#?}");
        let applied = report.applied.as_ref().expect("applied");
        assert!(applied.restarted && !applied.resumed, "{applied:#?}");
        assert_eq!(applied.imported.len(), 4, "{applied:#?}");
        assert_eq!(native_layout_status(&state), NativeLayoutStatus::Native);
    }

    /// A runtime.sqlite Core never claimed (here: only the session store's
    /// schema, what a new directory's run leaves when it stops before the
    /// pin) holds no runtime to keep: the export migrates in place over it.
    #[tokio::test]
    async fn an_unclaimed_runtime_store_takes_the_export_in_place() {
        let scratch = tempfile::tempdir().expect("scratch");
        let export = memory_export(scratch.path());
        let state = scratch.path().join("state");
        std::fs::create_dir(&state).expect("state dir");
        drop(
            meerkat_store::SqliteSessionStore::open(state.join(RUNTIME_DB_FILE_NAME))
                .expect("session schema"),
        );
        assert!(matches!(
            native_layout_status(&state),
            NativeLayoutStatus::Native
        ));

        let report = migrate_state_dir_native(
            &NativeMigrateRequest::new(&state, MigrateMode::Apply).with_continuity_export(&export),
        )
        .await;
        assert!(report.refusals.is_empty(), "{report:#?}");
        assert_eq!(
            report.shape,
            Some(GatewayStateShape::ContinuityExportInPlace)
        );
        assert_eq!(report.applied.as_ref().expect("applied").imported.len(), 4);
        assert_eq!(native_layout_status(&state), NativeLayoutStatus::Native);
    }

    /// A stopped export run finishes only from the file its journal names:
    /// the same entries under another path refuse, naming the journaled file.
    #[tokio::test]
    async fn a_resumed_export_run_refuses_a_file_at_another_path() {
        let scratch = tempfile::tempdir().expect("scratch");
        let export = memory_export(scratch.path());
        let moved = scratch.path().join("moved-export.jsonl");
        std::fs::copy(&export, &moved).expect("copy");
        let state = scratch.path().join("state");
        std::fs::create_dir(&state).expect("state dir");
        let (sessions, runtime) = super::super::import::open_native_pair(&state).expect("pair");
        drop((sessions, runtime));
        let digest = ExportProjection::read(&export)
            .expect("export reads")
            .file
            .entries_sha256;
        ApplyJournal::start(
            &state,
            ApplyPlan {
                import_source: Some(ImportSource::Export {
                    path: std::fs::canonicalize(&export).expect("canonical"),
                    entries_sha256: digest,
                }),
                new_directory: true,
                ..ApplyPlan::default()
            },
        )
        .expect("journal");

        let report = migrate_state_dir_native(
            &NativeMigrateRequest::new(&state, MigrateMode::Apply).with_continuity_export(&moved),
        )
        .await;
        let [NativeMigrateRefusal::ContinuityExportChanged { detail }] = report.refusals.as_slice()
        else {
            panic!("{report:#?}");
        };
        assert!(detail.contains("export.jsonl"), "{detail}");
        assert!(report.applied.is_none());
    }

    /// A stopped export run whose runtime.sqlite already carries Core's
    /// durable claim resumes from its journal, but only with the file it
    /// started with: another file's entries refuse typed, and nothing runs.
    #[tokio::test]
    async fn a_resumed_export_run_refuses_another_file() {
        let scratch = tempfile::tempdir().expect("scratch");
        let export = memory_export(scratch.path());
        let state = scratch.path().join("state");
        std::fs::create_dir(&state).expect("state dir");
        // The native pair, as the stopped run's first step created it.
        let (sessions, runtime) = super::super::import::open_native_pair(&state).expect("pair");
        drop((sessions, runtime));
        ApplyJournal::start(
            &state,
            ApplyPlan {
                import_source: Some(ImportSource::Export {
                    path: export.clone(),
                    entries_sha256: "0".repeat(64),
                }),
                new_directory: true,
                ..ApplyPlan::default()
            },
        )
        .expect("journal");

        let report = migrate_state_dir_native(
            &NativeMigrateRequest::new(&state, MigrateMode::Apply).with_continuity_export(&export),
        )
        .await;
        assert!(
            matches!(
                report.refusals.as_slice(),
                [NativeMigrateRefusal::ContinuityExportChanged { .. }]
            ),
            "{report:#?}"
        );
        assert!(report.applied.is_none());
        assert!(matches!(
            native_layout_status(&state),
            NativeLayoutStatus::MigrationIncomplete { .. }
        ));
    }
}
