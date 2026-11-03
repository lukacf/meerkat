//! `--apply` of the native-storage migration: carries out what a clean
//! census admitted, under the exclusion and fence the census held, and
//! journals every step so a run that stopped part way finishes when
//! `--apply` runs again.
//!
//! The steps, in order: back up every database the run changes (beside it,
//! under the registered backup naming), convert `runtime.sqlite` in place
//! onto Core's head-canonical profile, import the sessions only the
//! projection holds, write the first-boot manifest, and retire each
//! projection database: the sessions database by putting its retired stamp
//! at its path, the continuity database by deleting its session tables'
//! rows (a 0.9.4 identity gateway still opens it for its identity records).
//! The journal is written before the first write and removed after the
//! last step; while it exists, the startup predicate reads the directory as
//! [`super::NativeLayoutStatus::MigrationIncomplete`].

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use super::identity::FirstBootRowPlan;
use super::import::{ImportSource, SessionImport};
use super::layout_status::{NATIVE_MIGRATION_JOURNAL_FILE_NAME, write_retired_stamp};
use crate::storage_layout::RUNTIME_DB_FILE_NAME;

/// The journal's format name and version.
const JOURNAL_FORMAT: &str = "meerkat-meta.storage-migrate-native.journal";
const JOURNAL_VERSION: u32 = 1;

/// The suffix of the registered backup names this migration writes.
const BACKUP_PURPOSE: &str = "native-migration";

/// One step of an apply run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ApplyStep {
    /// Every database the run changes, copied beside it.
    BackUp,
    /// Journaled before the first write to `runtime.sqlite`. Information for
    /// the operator only: a stopped run resumes on Core's durable claim,
    /// which the conversion writes before anything else, not on this step.
    StartConversion,
    /// The body-less WholeBlob authorities the operator named, discarded
    /// with Core's own clear, before the conversion that would refuse them.
    DiscardBodiless,
    /// `runtime.sqlite` converted in place onto the head-canonical profile.
    ConvertRuntime,
    /// The sessions only the projection holds, imported into Core's store.
    ImportSessions,
    /// The first-boot manifest the identity runtime carries out.
    WriteFirstBootManifest,
    /// Each projection database retired: stamped, or emptied of its
    /// session rows.
    RetireProjections,
}

/// The steps in the order a run takes them.
const STEPS: [ApplyStep; 7] = [
    ApplyStep::BackUp,
    ApplyStep::StartConversion,
    ApplyStep::DiscardBodiless,
    ApplyStep::ConvertRuntime,
    ApplyStep::ImportSessions,
    ApplyStep::WriteFirstBootManifest,
    ApplyStep::RetireProjections,
];

/// How a projection database is retired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Retirement {
    /// Replaced by the retired stamp of its schema domain, which 0.9.3
    /// refuses to open.
    Stamp,
    /// Kept, with every row of its session tables deleted; its identity
    /// records stay for the 0.9.4 gateways that still open it.
    StripSessionTables,
}

/// A projection database the run retires, with its schema domain and how
/// it is retired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RetiredProjection {
    pub(super) file: String,
    pub(super) domain: String,
    pub(super) retirement: Retirement,
}

/// The continuity tables that hold the projection's session state. The
/// identity records, history and coverage tables beside them stay.
const CONTINUITY_PROJECTION_TABLES: [&str; 4] = [
    "session_snapshots",
    "continuity_session_heads",
    "continuity_strand_messages",
    "continuity_session_rewrites",
];

/// What a clean census asks an apply run to carry out.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct ApplyPlan {
    pub(super) projections: Vec<RetiredProjection>,
    /// The sessions only the projection side holds, imported through I1-I4.
    pub(super) imports: Vec<String>,
    /// Where the imported sessions are read from.
    pub(super) import_source: Option<ImportSource>,
    /// The first-boot manifest rows of the admitted identities.
    pub(super) first_boot: Vec<FirstBootRowPlan>,
    /// The state directory holds no `runtime.sqlite`: the run creates the
    /// native pair instead of converting one.
    pub(super) new_directory: bool,
    pub(super) discard: Vec<DiscardedRuntime>,
}

/// A body-less WholeBlob authority the operator named with
/// `--discard-bodyless`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DiscardedRuntime {
    pub(super) session_id: String,
    pub(super) runtime_id: String,
}

/// The journal of one apply run: the plan it carries out, the backup names
/// it chose before writing anything, and the steps it finished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ApplyJournal {
    format: String,
    version: u32,
    projections: Vec<RetiredProjection>,
    /// Backup file name by database file name, both in the state directory.
    backups: BTreeMap<String, String>,
    /// The body-less WholeBlob authorities to discard.
    #[serde(default)]
    discard: Vec<DiscardedRuntime>,
    done: Vec<ApplyStep>,
    /// The sessions the run imports, in order.
    #[serde(default)]
    imports: Vec<String>,
    /// Where it reads them: a projection backup, or a continuity export
    /// with the digest of its entries.
    #[serde(default)]
    import_source: Option<ImportSource>,
    /// The imports finished: each id is recorded after its I4, and a rerun
    /// skips it.
    #[serde(default)]
    imported: Vec<String>,
    /// The first-boot manifest rows the run writes.
    #[serde(default)]
    first_boot: Vec<FirstBootRowPlan>,
    /// The run creates the native pair in a new directory.
    #[serde(default)]
    new_directory: bool,
    /// The registered backup name of the first-boot manifest's byte copy,
    /// chosen with the other backup names, when the run writes a manifest.
    #[serde(default)]
    first_boot_copy: Option<String>,
}

/// One body-less WholeBlob authority an apply run discarded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct DiscardedRuntimeReport {
    pub session_id: String,
    pub runtime_id: String,
    /// The rows Core keeps for the runtime after the clear, by table: every
    /// table with a `runtime_id` column (its input ledger, runtime state and
    /// operations). Core's clear removes the authority, snapshot and catalog
    /// rows, the queued activation and the unreferenced bodies; the
    /// conversion and the 0.9.4 runtime read none of these rows for a
    /// runtime without session authority.
    pub leftover_rows: BTreeMap<String, u64>,
}

/// What an apply run did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct NativeApplyOutcome {
    /// The run finished one an earlier run had started and journaled.
    pub resumed: bool,
    /// An earlier run stopped before `runtime.sqlite` was converted, so its
    /// journal was dropped and this run started over with a new census.
    pub restarted: bool,
    /// Runtimes this run converted that were seeded from their WholeBlob
    /// bodies.
    pub runtimes_seeded: usize,
    /// Runtimes this run converted from the co-located session store's rows.
    pub runtimes_derived: usize,
    /// The pre-apply backups, in the state directory. The storage doctor
    /// lists them; `meta_gateway storage-prune` owns their lifecycle.
    pub backups: Vec<PathBuf>,
    /// The projection databases this run retired: stamped, or emptied of
    /// their session rows.
    pub retired: Vec<String>,
    /// The sessions this run imported through I1-I4.
    pub imported: Vec<SessionImport>,
    /// The digest of the first-boot manifest the run wrote, when it wrote
    /// one.
    pub first_boot_manifest_sha256: Option<String>,
    /// The body-less WholeBlob authorities this run discarded
    /// (`--discard-bodyless`), with the rows Core keeps for each.
    pub discarded: Vec<DiscardedRuntimeReport>,
}

impl ApplyJournal {
    /// Choose the backup names for `plan` and write the journal: the first
    /// write of the run.
    pub(super) fn start(state_dir: &Path, plan: ApplyPlan) -> Result<Self, String> {
        // A new directory has no runtime database to back up.
        let runtime = (!plan.new_directory).then(|| RUNTIME_DB_FILE_NAME.to_owned());
        let backups = runtime
            .into_iter()
            .chain(
                plan.projections
                    .iter()
                    .map(|projection| projection.file.clone()),
            )
            .map(|file| {
                let backup =
                    crate::storage_migrate::meta_backup_artifact_name(&file, BACKUP_PURPOSE);
                (file, backup)
            })
            .collect();
        let journal = Self {
            format: JOURNAL_FORMAT.to_owned(),
            version: JOURNAL_VERSION,
            projections: plan.projections,
            backups,
            discard: plan.discard,
            done: Vec::new(),
            imports: plan.imports,
            import_source: plan.import_source,
            imported: Vec::new(),
            first_boot_copy: (!plan.first_boot.is_empty()).then(|| {
                crate::storage_migrate::meta_backup_artifact_name(
                    crate::identity_first::FIRST_BOOT_MANIFEST_FILE,
                    BACKUP_PURPOSE,
                )
            }),
            first_boot: plan.first_boot,
            new_directory: plan.new_directory,
        };
        journal.write(state_dir)?;
        Ok(journal)
    }

    /// Read the journal an earlier run left. A journal this build does not
    /// know is refused rather than guessed at.
    pub(super) fn read(state_dir: &Path) -> Result<Self, String> {
        let path = state_dir.join(NATIVE_MIGRATION_JOURNAL_FILE_NAME);
        let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let journal: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if journal.format != JOURNAL_FORMAT || journal.version != JOURNAL_VERSION {
            return Err(format!(
                "{} is a {} journal, version {}; this build reads {JOURNAL_FORMAT} version \
                 {JOURNAL_VERSION}",
                path.display(),
                journal.format,
                journal.version
            ));
        }
        Ok(journal)
    }

    fn write(&self, state_dir: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        write_durably(state_dir, NATIVE_MIGRATION_JOURNAL_FILE_NAME, &bytes)
    }

    fn finish(&mut self, state_dir: &Path, step: ApplyStep) -> Result<(), String> {
        self.done.push(step);
        self.write(state_dir)
    }

    /// Record one finished import, durably, after its I4.
    fn record_import(&mut self, state_dir: &Path, session_id: &str) -> Result<(), String> {
        if !self.imported.iter().any(|done| done == session_id) {
            self.imported.push(session_id.to_owned());
        }
        self.write(state_dir)
    }

    /// The continuity export the run imports from, when it imports from
    /// one: its path and the digest of its entries.
    pub(super) fn export_source(&self) -> Option<(&Path, &str)> {
        match &self.import_source {
            Some(ImportSource::Export {
                path,
                entries_sha256,
            }) => Some((path.as_path(), entries_sha256.as_str())),
            _ => None,
        }
    }

    fn remaining(&self) -> Vec<ApplyStep> {
        STEPS
            .into_iter()
            .filter(|step| !self.done.contains(step))
            .collect()
    }
}

/// Run every step `journal` has not finished, then remove the journal.
/// Blocking: the caller holds the directory's exclusion and fence and runs
/// this on a blocking thread.
pub(super) fn run(
    state_dir: &Path,
    journal: &mut ApplyJournal,
    outcome: &mut NativeApplyOutcome,
    meta_lock_held: bool,
) -> Result<(), String> {
    for step in journal.remaining() {
        match step {
            ApplyStep::BackUp => back_up(state_dir, &journal.backups)?,
            // Nothing to do: the journal records the step before the
            // conversion's first write.
            ApplyStep::StartConversion => {}
            ApplyStep::DiscardBodiless => discard_bodiless(state_dir, &journal.discard)?,
            ApplyStep::ConvertRuntime if journal.new_directory => create_native_pair(state_dir)?,
            ApplyStep::ConvertRuntime => {
                let report = convert_runtime(state_dir)?;
                outcome.runtimes_seeded = report.seeded_count();
                outcome.runtimes_derived = report.derived_count();
            }
            ApplyStep::ImportSessions => {
                import_sessions(state_dir, journal, outcome, meta_lock_held)?;
            }
            ApplyStep::WriteFirstBootManifest => {
                outcome.first_boot_manifest_sha256 = write_first_boot_manifest(
                    state_dir,
                    &journal.first_boot,
                    journal.first_boot_copy.as_deref(),
                )?;
            }
            ApplyStep::RetireProjections => {
                for projection in &journal.projections {
                    match projection.retirement {
                        Retirement::Stamp => {
                            retire(state_dir, &projection.file, &projection.domain)?;
                        }
                        Retirement::StripSessionTables => {
                            strip_session_tables(&state_dir.join(&projection.file))?;
                        }
                    }
                }
            }
        }
        journal.finish(state_dir, step)?;
    }
    outcome.backups = journal
        .backups
        .values()
        .chain(journal.first_boot_copy.iter())
        .map(|backup| state_dir.join(backup))
        .collect();
    outcome.retired = journal
        .projections
        .iter()
        .map(|projection| projection.file.clone())
        .collect();
    let runtime_db = state_dir.join(RUNTIME_DB_FILE_NAME);
    outcome.discarded = journal
        .discard
        .iter()
        .map(|discarded| {
            Ok(DiscardedRuntimeReport {
                session_id: discarded.session_id.clone(),
                runtime_id: discarded.runtime_id.clone(),
                leftover_rows: leftover_rows(&runtime_db, &discarded.runtime_id)?,
            })
        })
        .collect::<Result<_, String>>()?;
    remove_durably(state_dir, NATIVE_MIGRATION_JOURNAL_FILE_NAME)
}

/// Drop the journal of a run that stopped before `runtime.sqlite` was
/// converted. Its backups stay as registered artifacts for
/// `meta_gateway storage-prune`; the run that starts over takes new ones.
pub(super) fn abandon_journal(state_dir: &Path) -> Result<(), String> {
    remove_durably(state_dir, NATIVE_MIGRATION_JOURNAL_FILE_NAME)
}

/// Copy each database into its backup, as one consistent file read through
/// its write-ahead log.
fn back_up(state_dir: &Path, backups: &BTreeMap<String, String>) -> Result<(), String> {
    for (file, backup) in backups {
        let source = state_dir.join(file);
        let target = state_dir.join(backup);
        // A run stopped mid-copy left a partial file under this run's own
        // backup name; the copy starts over.
        remove_if_present(&target)?;
        let conn = Connection::open_with_flags(
            &source,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| format!("open {}: {error}", source.display()))?;
        let target_text = target
            .to_str()
            .ok_or_else(|| format!("{} is not a UTF-8 path", target.display()))?;
        conn.execute("VACUUM INTO ?1", [target_text])
            .map_err(|error| {
                format!(
                    "back up {} to {}: {error}",
                    source.display(),
                    target.display()
                )
            })?;
        drop(conn);
        sync_file(&target)?;
    }
    sync_dir(state_dir)
}

/// Discard each named WholeBlob authority with Core's own clear, which also
/// drops its catalog entry and its queued activation. Proven again here,
/// under the fence: a runtime already discarded (a rerun) is skipped, and
/// one that now holds its body refuses the run.
fn discard_bodiless(state_dir: &Path, discard: &[DiscardedRuntime]) -> Result<(), String> {
    use meerkat_runtime::RuntimeStore as _;
    if discard.is_empty() {
        return Ok(());
    }
    let path = state_dir.join(RUNTIME_DB_FILE_NAME);
    let bodiless = super::census::bodiless_runtime_ids(&path)?;
    let store = meerkat_runtime::SqliteRuntimeStore::new_whole_blob(&path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    let handle = tokio::runtime::Handle::current();
    for discarded in discard {
        if !bodiless.contains(&discarded.runtime_id) {
            let present = handle
                .block_on(store.load_committed_whole_blob_snapshot(&runtime_id_of(discarded)?))
                .is_ok_and(|snapshot| snapshot.is_some());
            if present {
                return Err(format!(
                    "runtime {} now holds a WholeBlob body; refusing to discard it",
                    discarded.runtime_id
                ));
            }
            continue;
        }
        handle
            .block_on(store.clear_session_snapshot(&runtime_id_of(discarded)?))
            .map_err(|error| format!("discard {}: {error}", discarded.runtime_id))?;
    }
    Ok(())
}

/// The rows `runtime_id` still has in `path`, by table: every table with a
/// `runtime_id` column, read-only.
fn leftover_rows(path: &Path, runtime_id: &str) -> Result<BTreeMap<String, u64>, String> {
    let failed = |error: rusqlite::Error| format!("leftover rows in {}: {error}", path.display());
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(failed)?;
    let tables: Vec<String> = {
        let mut statement = conn
            .prepare(
                "SELECT name FROM main.sqlite_master WHERE type = 'table' \
                 AND name NOT LIKE 'sqlite!_%' ESCAPE '!' ORDER BY name",
            )
            .map_err(failed)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(failed)?;
        rows.collect::<Result<_, _>>().map_err(failed)?
    };
    let mut found = BTreeMap::new();
    for table in tables {
        let has_runtime_id: bool = conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM pragma_table_info(?1) WHERE name = 'runtime_id')",
                [&table],
                |row| row.get(0),
            )
            .map_err(failed)?;
        if !has_runtime_id {
            continue;
        }
        let quoted = table.replace('"', "\"\"");
        let count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM main.\"{quoted}\" WHERE runtime_id = ?1"),
                [runtime_id],
                |row| row.get(0),
            )
            .map_err(failed)?;
        if count > 0 {
            found.insert(table, u64::try_from(count).unwrap_or(u64::MAX));
        }
    }
    Ok(found)
}

/// The runtime id a discarded authority names. The census refuses a
/// body-less authority under the legacy alias before `--apply` writes
/// anything (Core cannot clear one there), so only the session's runtime id
/// reaches a discard; the alias is still parsed so a hand-edited journal
/// names a typed error rather than a missing session.
fn runtime_id_of(
    discarded: &DiscardedRuntime,
) -> Result<meerkat_runtime::LogicalRuntimeId, String> {
    let session = super::census::session_of_runtime(&discarded.runtime_id)
        .ok_or_else(|| format!("{} names no session runtime", discarded.runtime_id))?;
    [
        meerkat_runtime::LogicalRuntimeId::for_session(&session),
        meerkat_runtime::LogicalRuntimeId::legacy_session_uuid_alias(&session),
    ]
    .into_iter()
    .find(|candidate| candidate.0 == discarded.runtime_id)
    .ok_or_else(|| format!("{} is not a session runtime id", discarded.runtime_id))
}

/// Convert `runtime.sqlite` in place: open the session store on the file
/// first, so the conversion finds the co-located store's schema, then seed
/// every runtime's session there from its committed WholeBlob body. Core
/// first stores every image those bodies hold inline in the realm blob
/// store, and the images are synced there before the conversion's first
/// write: the seeded rows name them by their content address (the WholeBlob
/// bodies keep them inline). A conversion stopped part way finishes here,
/// and a converted file converts nothing.
fn convert_runtime(
    state_dir: &Path,
) -> Result<meerkat_runtime::StandaloneWholeBlobConversionReport, String> {
    let path = state_dir.join(RUNTIME_DB_FILE_NAME);
    let store = meerkat_store::SqliteSessionStore::open(path.clone())
        .map_err(|error| format!("session store schema on {}: {error}", path.display()))?;
    drop(store);
    let blobs = super::import::RealmBlobs::open(state_dir)?;
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|error| format!("the conversion needs the migration's runtime: {error}"))?;
    let stage = handle
        .block_on(
            meerkat_runtime::SqliteRuntimeStore::stage_standalone_whole_blob_media(
                path.clone(),
                blobs.store(),
            ),
        )
        .map_err(|error| format!("store the inline images of {}: {error}", path.display()))?;
    blobs.sync(stage.stored())?;
    meerkat_runtime::SqliteRuntimeStore::convert_standalone_whole_blob_in_place_with_staged_media(
        &path, &stage,
    )
    .map_err(|error| format!("convert {}: {error}", path.display()))
}

/// Create the native pair in a new state directory: Core's session store
/// and the head-canonical runtime store, both on `runtime.sqlite`, the way
/// the native composition opens them.
fn create_native_pair(state_dir: &Path) -> Result<(), String> {
    let (sessions, runtime) = super::import::open_native_pair(state_dir)?;
    drop(sessions);
    drop(runtime);
    sync_dir(state_dir)
}

/// Import every planned session the journal has not recorded yet, through
/// I1-I4, holding the directory's Meta runtime lock across them. A 0.9.3
/// directory has none, so the run creates it; when the lock file was there
/// already (a stopped run created it), the run's directory hold locks it,
/// as `meta_lock_held` says. Each id is journaled after its I4; a stopped
/// run imports the rest when it runs again.
fn import_sessions(
    state_dir: &Path,
    journal: &mut ApplyJournal,
    outcome: &mut NativeApplyOutcome,
    meta_lock_held: bool,
) -> Result<(), String> {
    let pending: Vec<String> = journal
        .imports
        .iter()
        .filter(|id| !journal.imported.contains(id))
        .cloned()
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    let source = journal
        .import_source
        .clone()
        .ok_or_else(|| "the journal plans imports and names no source".to_owned())?;
    let _lock = if meta_lock_held {
        None
    } else {
        Some(super::import::hold_meta_runtime_lock(state_dir)?)
    };
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|error| format!("the import needs the migration's runtime: {error}"))?;
    let backups = journal.backups.clone();
    let mut imported = Vec::new();
    handle.block_on(super::import::import_planned(
        state_dir,
        &source,
        &backups,
        &pending,
        |import: SessionImport| {
            journal.record_import(state_dir, &import.session_id)?;
            imported.push(import);
            Ok(())
        },
    ))?;
    outcome.imported = imported;
    Ok(())
}

/// Write the first-boot manifest of `rows` (none when there are no rows),
/// and keep a byte copy of it as `copy`, the registered backup name the
/// journal chose at its start, so the storage doctor lists it and the
/// manifest reader's refusal can name it. A rerun that finds the manifest
/// it planned keeps it (and writes the copy again); another manifest
/// refuses. Returns the manifest's rows digest.
fn write_first_boot_manifest(
    state_dir: &Path,
    rows: &[FirstBootRowPlan],
    copy: Option<&str>,
) -> Result<Option<String>, String> {
    use crate::identity_first::{FirstBootManifest, FirstBootManifestRefusal};
    if rows.is_empty() {
        return Ok(None);
    }
    let rows = rows
        .iter()
        .map(FirstBootRowPlan::to_manifest_row)
        .collect::<Result<Vec<_>, _>>()?;
    let planned = FirstBootManifest::new(rows.clone())
        .map_err(|refusal| format!("first-boot manifest: {refusal}"))?;
    let manifest = match FirstBootManifest::write_new(state_dir, rows) {
        Ok(manifest) => manifest,
        Err(error) if matches!(error.refusal(), FirstBootManifestRefusal::AlreadyExists) => {
            let existing = FirstBootManifest::read(state_dir)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "the first-boot manifest went away while it was read".to_owned())?;
            if existing.rows_sha256() != planned.rows_sha256() {
                return Err(format!(
                    "{} holds rows {} the run did not plan (planned {}); restore the backups \
                     the journal lists, then remove the journal",
                    FirstBootManifest::path(state_dir).display(),
                    existing.rows_sha256(),
                    planned.rows_sha256()
                ));
            }
            existing
        }
        Err(error) => return Err(error.to_string()),
    };
    let path = FirstBootManifest::path(state_dir);
    let copy = copy.ok_or_else(|| {
        "the journal plans first-boot rows and names no copy of the manifest".to_owned()
    })?;
    let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    write_durably(state_dir, copy, &bytes)?;
    Ok(Some(manifest.rows_sha256().to_owned()))
}

/// Put `domain`'s retired stamp at `file`. The stamp is built beside it and
/// renamed into place. The projection's log is folded into it first, so a
/// run stopped before the rename leaves a complete database behind; then
/// its log and index go, so the stamp is never read with a log written for
/// the database it replaces (the backup holds what they held).
fn retire(state_dir: &Path, file: &str, domain: &str) -> Result<(), String> {
    let live = state_dir.join(file);
    let staged_name = format!("{file}.native-retire");
    let staged = state_dir.join(&staged_name);
    // A run stopped inside the stamp's write can leave its journal too; the
    // next open would roll it into the new stamp.
    for suffix in ["", "-journal", "-wal", "-shm"] {
        remove_if_present(&state_dir.join(format!("{staged_name}{suffix}")))?;
    }
    write_retired_stamp(&staged, domain)?;
    sync_file(&staged)?;
    checkpoint(&live)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        remove_if_present(&state_dir.join(format!("{file}{suffix}")))?;
    }
    fs::rename(&staged, &live)
        .map_err(|error| format!("rename {} -> {}: {error}", staged.display(), live.display()))?;
    sync_dir(state_dir)
}

/// Fold `path`'s write-ahead log into the database and truncate the log. A
/// database that is not in WAL mode, or already a stamp, has nothing to
/// fold.
fn checkpoint(path: &Path) -> Result<(), String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("open {}: {error}", path.display()))?;
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .map_err(|error| format!("checkpoint {}: {error}", path.display()))
}

/// Delete every row of the continuity projection's session tables in one
/// transaction, then fold the log in. A table the file does not have is
/// skipped. Running it again deletes nothing more.
fn strip_session_tables(path: &Path) -> Result<(), String> {
    let mut conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("open {}: {error}", path.display()))?;
    let failed = |error: rusqlite::Error| format!("strip {}: {error}", path.display());
    let tx = conn.transaction().map_err(failed)?;
    for table in CONTINUITY_PROJECTION_TABLES {
        let present: bool = tx
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )
            .map_err(failed)?;
        if present {
            tx.execute(&format!("DELETE FROM \"{table}\""), [])
                .map_err(failed)?;
        }
    }
    tx.commit().map_err(failed)?;
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .map_err(failed)
}

/// Write `bytes` as `name` in `dir`: a temporary file, synced, renamed into
/// place, and the directory synced.
fn write_durably(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let staged = dir.join(format!("{name}.tmp"));
    let mut file =
        File::create(&staged).map_err(|error| format!("{}: {error}", staged.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("{}: {error}", staged.display()))?;
    drop(file);
    let target = dir.join(name);
    fs::rename(&staged, &target).map_err(|error| {
        format!(
            "rename {} -> {}: {error}",
            staged.display(),
            target.display()
        )
    })?;
    sync_dir(dir)
}

fn remove_durably(dir: &Path, name: &str) -> Result<(), String> {
    remove_if_present(&dir.join(name))?;
    sync_dir(dir)
}

fn remove_if_present(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", path.display())),
    }
}

pub(super) fn sync_file(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("sync {}: {error}", path.display()))
}

/// Sync a directory's entries. Only POSIX directories can be opened and
/// synced; elsewhere the rename is as durable as the platform makes it.
pub(super) fn sync_dir(dir: &Path) -> Result<(), String> {
    if cfg!(unix) {
        File::open(dir)
            .and_then(|handle| handle.sync_all())
            .map_err(|error| format!("sync {}: {error}", dir.display()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    use meerkat_store::sqlite_store::SESSION_STORE_DOMAIN;

    fn plan() -> ApplyPlan {
        ApplyPlan {
            projections: vec![RetiredProjection {
                file: "sessions.sqlite3".to_owned(),
                domain: SESSION_STORE_DOMAIN.name.to_owned(),
                retirement: Retirement::Stamp,
            }],
            ..ApplyPlan::default()
        }
    }

    /// The conversion step on a blocking thread, where `run` takes it.
    async fn convert_on_a_blocking_thread(state_dir: &Path) {
        let state_dir = state_dir.to_path_buf();
        tokio::task::spawn_blocking(move || convert_runtime(&state_dir))
            .await
            .expect("the conversion thread")
            .expect("the conversion");
    }

    /// An unpacked copy of one 0.9.3 gateway state directory (see
    /// `tests/fixtures/meta_093_gateway_states/README.md`).
    fn unpack(shape: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/meta_093_gateway_states")
            .join(format!("{shape}.tar.gz"));
        let file =
            File::open(&archive).unwrap_or_else(|error| panic!("{}: {error}", archive.display()));
        tar::Archive::new(flate2::read::GzDecoder::new(file))
            .unpack(dir.path())
            .unwrap_or_else(|error| panic!("unpack {shape}: {error}"));
        dir
    }

    #[test]
    fn the_journal_is_written_before_any_step_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let journal = ApplyJournal::start(dir.path(), plan()).unwrap();
        assert!(
            dir.path()
                .join(NATIVE_MIGRATION_JOURNAL_FILE_NAME)
                .is_file()
        );
        assert_eq!(ApplyJournal::read(dir.path()).unwrap(), journal);
        assert_eq!(journal.remaining(), STEPS.to_vec());
        // Every database the run changes gets a registered backup name.
        assert_eq!(journal.backups.len(), 2);
        assert!(
            journal
                .backups
                .values()
                .all(|name| crate::storage_migrate::is_registered_backup_artifact_name(name))
        );
    }

    #[test]
    fn a_journal_this_build_does_not_know_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut journal = ApplyJournal::start(dir.path(), plan()).unwrap();
        journal.version = JOURNAL_VERSION + 1;
        journal.write(dir.path()).unwrap();
        assert!(ApplyJournal::read(dir.path()).is_err());

        let path = dir.path().join(NATIVE_MIGRATION_JOURNAL_FILE_NAME);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["version"] = serde_json::json!(JOURNAL_VERSION);
        value["unexpected"] = serde_json::json!(true);
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(ApplyJournal::read(dir.path()).is_err());
    }

    #[test]
    fn a_finished_step_is_not_taken_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut journal = ApplyJournal::start(dir.path(), plan()).unwrap();
        journal.finish(dir.path(), ApplyStep::BackUp).unwrap();
        let read = ApplyJournal::read(dir.path()).unwrap();
        assert_eq!(read.remaining(), STEPS[1..].to_vec());
    }

    #[test]
    fn retiring_a_projection_leaves_its_stamp_and_no_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("sessions.sqlite3");
        {
            let conn = Connection::open(&live).unwrap();
            conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t (x INTEGER);")
                .unwrap();
        }
        fs::write(dir.path().join("sessions.sqlite3-wal"), b"stale").unwrap();
        retire(dir.path(), "sessions.sqlite3", SESSION_STORE_DOMAIN.name).unwrap();
        assert!(!dir.path().join("sessions.sqlite3-wal").exists());
        assert!(!dir.path().join("sessions.sqlite3.native-retire").exists());
        let conn = Connection::open_with_flags(&live, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let version: i64 = conn
            .query_row(
                "SELECT version FROM meerkat_schema WHERE domain = ?1",
                [SESSION_STORE_DOMAIN.name],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, super::super::RETIRED_PROJECTION_SCHEMA_VERSION);
    }

    /// A run stopped after the journaled start of the conversion and its
    /// session-store schema write, but before Core's pin, starts over: 0.9.3
    /// still opens that directory, so only the claim may resume a run. The
    /// first run's backups stay on disk.
    #[tokio::test]
    async fn a_run_stopped_before_the_pin_starts_over_whatever_the_journal_says() {
        use crate::storage_migrate::MigrateMode;
        use crate::storage_migrate_native::{
            NativeLayoutStatus, NativeMigrateRequest, migrate_state_dir_native,
            native_layout_status,
        };
        let dir = unpack("s1b-plain");
        let mut journal = ApplyJournal::start(dir.path(), plan()).unwrap();
        back_up(dir.path(), &journal.backups).unwrap();
        journal.finish(dir.path(), ApplyStep::BackUp).unwrap();
        journal
            .finish(dir.path(), ApplyStep::StartConversion)
            .unwrap();
        // The conversion's first write: the co-located session schema.
        drop(
            meerkat_store::SqliteSessionStore::open(dir.path().join(RUNTIME_DB_FILE_NAME)).unwrap(),
        );
        assert!(
            !super::super::layout_status::runtime_conversion_began(dir.path()).unwrap(),
            "the schema write alone carries no durable claim"
        );
        let backup = dir.path().join(&journal.backups[RUNTIME_DB_FILE_NAME]);
        let report =
            migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), MigrateMode::Apply))
                .await;
        assert!(report.refusals.is_empty(), "{report:#?}");
        let applied = report.applied.expect("applied");
        assert!(applied.restarted && !applied.resumed, "{applied:#?}");
        assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);
        assert!(backup.is_file(), "the first run's backup stays");
    }

    /// A journal that cannot be read decides nothing by itself: without
    /// Core's claim the rerun drops it and starts over; with the claim the
    /// run cannot tell how far the conversion got, so it refuses and leaves
    /// the journal for the operator.
    #[tokio::test]
    async fn an_unreadable_journal_starts_over_without_the_claim_and_refuses_with_it() {
        use crate::storage_migrate::MigrateMode;
        use crate::storage_migrate_native::{
            NativeLayoutStatus, NativeMigrateRefusal, NativeMigrateRequest,
            migrate_state_dir_native, native_layout_status,
        };
        let unclaimed = unpack("s1b-plain");
        fs::write(
            unclaimed.path().join(NATIVE_MIGRATION_JOURNAL_FILE_NAME),
            b"not a journal",
        )
        .unwrap();
        let report = migrate_state_dir_native(&NativeMigrateRequest::new(
            unclaimed.path(),
            MigrateMode::Apply,
        ))
        .await;
        assert!(report.refusals.is_empty(), "{report:#?}");
        let applied = report.applied.expect("applied");
        assert!(applied.restarted && !applied.resumed, "{applied:#?}");
        assert_eq!(
            native_layout_status(unclaimed.path()),
            NativeLayoutStatus::Native
        );

        let claimed = unpack("s1b-plain");
        convert_on_a_blocking_thread(claimed.path()).await;
        fs::write(
            claimed.path().join(NATIVE_MIGRATION_JOURNAL_FILE_NAME),
            b"not a journal",
        )
        .unwrap();
        let report = migrate_state_dir_native(&NativeMigrateRequest::new(
            claimed.path(),
            MigrateMode::Apply,
        ))
        .await;
        assert!(
            report.refusals.iter().any(|refusal| matches!(
                refusal,
                NativeMigrateRefusal::MigrationJournalUnreadable { .. }
            )),
            "{report:#?}"
        );
        assert!(report.applied.is_none(), "{report:#?}");
        assert!(
            claimed
                .path()
                .join(NATIVE_MIGRATION_JOURNAL_FILE_NAME)
                .is_file(),
            "the journal stays for the operator"
        );
    }

    /// A run stopped after the conversion returned and before its step was
    /// journaled finishes from the journal (the start mark, and Core's
    /// claim, are there): it neither restarts nor takes new backups.
    #[tokio::test]
    async fn a_run_stopped_after_an_unjournaled_conversion_finishes() {
        use crate::storage_migrate::MigrateMode;
        use crate::storage_migrate_native::{
            NativeLayoutStatus, NativeMigrateRequest, migrate_state_dir_native,
            native_layout_status,
        };
        let dir = unpack("s1b-plain");
        let mut journal = ApplyJournal::start(dir.path(), plan()).unwrap();
        back_up(dir.path(), &journal.backups).unwrap();
        journal.finish(dir.path(), ApplyStep::BackUp).unwrap();
        journal
            .finish(dir.path(), ApplyStep::StartConversion)
            .unwrap();
        convert_on_a_blocking_thread(dir.path()).await;
        let backups = journal.backups.clone();
        let report =
            migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), MigrateMode::Apply))
                .await;
        assert!(report.refusals.is_empty(), "{report:#?}");
        let applied = report.applied.expect("applied");
        assert!(applied.resumed && !applied.restarted, "{applied:#?}");
        let expected: Vec<PathBuf> = backups.values().map(|name| dir.path().join(name)).collect();
        assert_eq!(applied.backups, expected, "the run kept its first backups");
        assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);
    }

    /// What keeps 0.9.3 off an applied directory: a WholeBlob open of
    /// `runtime.sqlite` (the runtime store 0.9.3 opens at startup) refuses
    /// the converted file on its head-canonical durable claim, the check
    /// 0.9.3's own store makes. The run of the published 0.9.3 binary on an
    /// applied copy is the release evidence for its startup order.
    #[tokio::test]
    async fn a_whole_blob_open_refuses_the_applied_runtime_store() {
        use crate::storage_migrate::MigrateMode;
        use crate::storage_migrate_native::{NativeMigrateRequest, migrate_state_dir_native};
        let dir = unpack("s1b-plain");
        let path = dir.path().join(RUNTIME_DB_FILE_NAME);
        let report =
            migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), MigrateMode::Apply))
                .await;
        assert!(report.applied.is_some(), "{report:#?}");
        assert!(
            meerkat_runtime::SqliteRuntimeStore::new_whole_blob(&path).is_err(),
            "a WholeBlob open must refuse the converted runtime store"
        );
    }

    /// Stripping the continuity projection deletes its session rows and
    /// keeps its identity records, so the startup predicate no longer reads
    /// it as a projection; stripping again changes nothing.
    #[test]
    fn stripping_the_continuity_projection_keeps_its_records() {
        let dir = unpack("s1-roster");
        let path = dir.path().join("continuity.sqlite3");
        let count = |table: &str| -> i64 {
            let conn =
                Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
            conn.query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
                row.get(0)
            })
            .unwrap()
        };
        let records = count("continuity_records");
        assert!(records > 0);
        assert!(
            CONTINUITY_PROJECTION_TABLES
                .iter()
                .any(|table| count(table) > 0)
        );
        for _ in 0..2 {
            strip_session_tables(&path).unwrap();
            assert_eq!(count("continuity_records"), records);
            for table in CONTINUITY_PROJECTION_TABLES {
                assert_eq!(count(table), 0, "{table}");
            }
        }
        let status = crate::storage_migrate_native::native_layout_status(dir.path());
        assert!(
            !format!("{status:?}").contains("ContinuitySessions"),
            "{status:?}"
        );
    }

    /// A run stopped after `runtime.sqlite`'s conversion finishes from its
    /// journal when `--apply` runs again: 0.9.3 refuses the converted file,
    /// so nothing can have written the directory since. A run stopped before
    /// it starts over with a new census, and that census refuses what it
    /// would refuse on a fresh run (here a session only the projection
    /// holds, as a 0.9.3 run in between could leave), writing nothing.
    /// Meanwhile the startup predicate and a dry run refuse the directory.
    #[tokio::test]
    async fn a_stopped_run_resumes_only_past_the_conversion() {
        use crate::storage_migrate::MigrateMode;
        use crate::storage_migrate_native::{
            NativeLayoutStatus, NativeMigrateRefusal, NativeMigrateRequest,
            migrate_state_dir_native, native_layout_status,
        };
        let converted = unpack("s1b-plain");
        let mut journal = ApplyJournal::start(converted.path(), plan()).unwrap();
        back_up(converted.path(), &journal.backups).unwrap();
        journal.finish(converted.path(), ApplyStep::BackUp).unwrap();
        journal
            .finish(converted.path(), ApplyStep::StartConversion)
            .unwrap();
        convert_on_a_blocking_thread(converted.path()).await;
        journal
            .finish(converted.path(), ApplyStep::ConvertRuntime)
            .unwrap();
        assert!(matches!(
            native_layout_status(converted.path()),
            NativeLayoutStatus::MigrationIncomplete { .. }
        ));
        let dry = migrate_state_dir_native(&NativeMigrateRequest::new(
            converted.path(),
            MigrateMode::DryRun,
        ))
        .await;
        assert!(
            dry.refusals
                .iter()
                .any(|refusal| matches!(refusal, NativeMigrateRefusal::MigrationIncomplete { .. })),
            "{dry:#?}"
        );
        let report = migrate_state_dir_native(&NativeMigrateRequest::new(
            converted.path(),
            MigrateMode::Apply,
        ))
        .await;
        assert!(report.refusals.is_empty(), "{report:#?}");
        let applied = report.applied.expect("applied");
        assert!(applied.resumed && !applied.restarted, "{applied:#?}");
        assert_eq!(
            native_layout_status(converted.path()),
            NativeLayoutStatus::Native
        );

        let unconverted = unpack("s1b-plain");
        ApplyJournal::start(unconverted.path(), plan()).unwrap();
        let report = migrate_state_dir_native(&NativeMigrateRequest::new(
            unconverted.path(),
            MigrateMode::Apply,
        ))
        .await;
        assert!(report.refusals.is_empty(), "{report:#?}");
        let applied = report.applied.expect("applied");
        assert!(applied.restarted && !applied.resumed, "{applied:#?}");
        assert_eq!(
            native_layout_status(unconverted.path()),
            NativeLayoutStatus::Native
        );

        // A run that journaled the start of the conversion but stopped before
        // Core's claim, on a directory 0.9.3 may then have written: the rerun
        // drops the journal, re-censuses, and imports the session only the
        // projection holds.
        let changed = unpack("s1b-runtime-missing");
        let mut journal = ApplyJournal::start(changed.path(), plan()).unwrap();
        back_up(changed.path(), &journal.backups).unwrap();
        journal.finish(changed.path(), ApplyStep::BackUp).unwrap();
        journal
            .finish(changed.path(), ApplyStep::StartConversion)
            .unwrap();
        let report = migrate_state_dir_native(&NativeMigrateRequest::new(
            changed.path(),
            MigrateMode::Apply,
        ))
        .await;
        assert!(report.refusals.is_empty(), "{report:#?}");
        let applied = report.applied.expect("applied");
        assert!(applied.restarted && !applied.resumed, "{applied:#?}");
        assert_eq!(applied.imported.len(), 1, "{applied:#?}");
        assert_eq!(
            native_layout_status(changed.path()),
            NativeLayoutStatus::Native
        );
    }

    /// A runtime whose committed WholeBlob body compacted over a tool
    /// result's inline image converts: `--apply` moves the image to the
    /// realm blob store before the conversion's first write, the converted
    /// session names it by its content address, it hydrates from `blobs/`
    /// in the state directory, and no table of `runtime.sqlite` but the
    /// kept 0.9.3 bodies holds it inline.
    #[tokio::test]
    async fn an_applied_runtime_hydrates_its_inline_images_from_the_realm_blob_store() {
        use crate::storage_migrate::MigrateMode;
        use crate::storage_migrate_native::census::tests::compacted_over_a_tool_result_image;
        use crate::storage_migrate_native::{
            NativeLayoutStatus, NativeMigrateRequest, migrate_state_dir_native,
            native_layout_status,
        };
        use meerkat::SessionStore as _;
        use meerkat_runtime::RuntimeStore as _;
        const SCREENSHOT: &str = "iVBORw0KGgo=";
        let dir = unpack("s1b-plain");
        let session = compacted_over_a_tool_result_image(SCREENSHOT);
        let path = dir.path().join(RUNTIME_DB_FILE_NAME);
        let whole_blob = meerkat_runtime::SqliteRuntimeStore::new_whole_blob(&path).unwrap();
        whole_blob
            .commit_session_snapshot(
                &meerkat_runtime::LogicalRuntimeId::for_session(session.id()),
                meerkat_runtime::store::SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(&session).unwrap().into(),
                },
            )
            .await
            .unwrap();
        drop(whole_blob);

        let report =
            migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), MigrateMode::Apply))
                .await;
        assert!(report.refusals.is_empty(), "{report:#?}");
        assert!(report.applied.is_some(), "{report:#?}");
        assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);

        let converted = meerkat_store::SqliteSessionStore::open(path.clone())
            .unwrap()
            .load(session.id())
            .await
            .unwrap()
            .expect("the converted session");
        let document = serde_json::to_string(&converted).unwrap();
        assert!(
            !document.contains(SCREENSHOT),
            "the converted session holds the image inline"
        );
        let blob_id = meerkat_core::content_blob_id("image/png", SCREENSHOT);
        assert!(document.contains(blob_id.as_str()), "{document}");
        let blobs = crate::blob_store::Base64BlobStoreAdapter::new(std::sync::Arc::new(
            crate::blob_store::ObjectStoreBlobStore::local(dir.path().join("blobs")).unwrap(),
        ));
        let image = meerkat_core::BlobStore::get(&blobs, &blob_id)
            .await
            .expect("the image hydrates from blobs/");
        assert_eq!(image.data, SCREENSHOT);

        // No table of runtime.sqlite holds the image inline any more, except
        // the 0.9.3 WholeBlob bodies Core keeps beside the converted rows.
        let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' \
                 AND name <> 'runtime_whole_blob_bodies'",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(!tables.is_empty());
        for table in &tables {
            let mut statement = conn.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
            let columns = statement.column_count();
            let mut rows = statement.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                for column in 0..columns {
                    let bytes = match row.get_ref(column).unwrap() {
                        rusqlite::types::ValueRef::Text(bytes)
                        | rusqlite::types::ValueRef::Blob(bytes) => bytes,
                        _ => continue,
                    };
                    assert!(
                        !bytes
                            .windows(SCREENSHOT.len())
                            .any(|window| window == SCREENSHOT.as_bytes()),
                        "{table} holds the image inline"
                    );
                }
            }
        }
    }
}
