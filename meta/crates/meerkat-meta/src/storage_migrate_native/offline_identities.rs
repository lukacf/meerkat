//! `meta_gateway identity-census`: an offline, read-only census of the
//! identities a native (0.9.4) state directory holds, for readers that run
//! on a copy of the directory while no gateway runs on it.
//!
//! Every fact is Core's, or a file Meta keeps beside Core's stores:
//! - each identity's last member binding in the directory's durable mob
//!   store, read with Core's member-history reader the way the gateway's
//!   identity views read it (the session, the generation, the mob member id,
//!   and Meta's runtime id while the member is live);
//! - the runtime id under which `runtime.sqlite` holds that session's
//!   authority, and whether Core's session catalog marks the session
//!   lifecycle-terminal, both from one observation of Core's runtime store;
//! - whether the identity owes its host a held completion (its
//!   held-completion mark, read by the identity runtime's rule) and its
//!   pending first-boot row.
//!
//! The directory is never written, and no database in it is opened: the
//! census copies its top-level files (all but SQLite's shared-memory indexes,
//! the registered backups and the quarantined files) into a scratch
//! directory outside it and reads the copies, so no `-wal`/`-shm` sidecar,
//! checkpoint or lock file appears. The held-completion marks are plain
//! files, read in place. The runtime-lock probe opens existing lock files
//! read-only and releases them once the copies and the marks are read. A
//! directory a runtime holds, or one that is not in the native layout, is
//! refused typed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use meerkat_core::types::SessionId;
use meerkat_runtime::{LogicalRuntimeId, RuntimeStore, SqliteRuntimeStore};
use serde::Serialize;

use super::census::{NativeMigrateRefusal, exclude_live_runtimes, state_dir_mob_store};
use super::layout_status::{NativeLayoutReason, NativeLayoutStatus, native_layout_status};
use crate::identity_first::FirstBootRow;
use crate::identity_first::held_bootstrap::HeldCompletionMarks;
use crate::member_comms_id::{durable_identity_for_roster_member, roster_member_id_for_identity};
use crate::storage_layout::{
    CANONICAL_CONTINUITY_DB_FILE_NAME, DatabaseSlot, RUNTIME_DB_FILE_NAME,
};

/// The report's `format`.
pub const IDENTITY_CENSUS_FORMAT: &str = "meerkat-meta.identity-census";

/// The report's schema version.
pub const IDENTITY_CENSUS_VERSION: u32 = 1;

/// The report's `mob_store_note` when the directory has no durable mob
/// store.
pub const IN_MEMORY_MOB_STORE_NOTE: &str = "the directory holds no durable mob store: in-memory \
     mob storage keeps no member binding across a restart, so each identity's binding facts are \
     null and nothing was lost (a mob store kept outside the directory is not read)";

/// One identity of the directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct IdentityCensusEntry {
    /// The Meta identity.
    pub identity: String,
    /// The session of Core's last binding of the identity since the latest
    /// mob reset, live or retired.
    pub session_id: Option<String>,
    /// Core's generation of that binding.
    pub generation: Option<u64>,
    /// Core's mob member id of that binding: the roster id Core's member
    /// tokens name.
    pub member_id: Option<String>,
    /// Whether Core holds the identity's member live.
    pub live: bool,
    /// Meta's public runtime id of the live member, the one the identity
    /// views report; `None` unless the member is live.
    pub agent_runtime_id: Option<String>,
    /// The runtime id under which `runtime.sqlite` holds the session's
    /// authority: `rt:session:<session id>`, or the legacy alias (the bare
    /// session id); `None` when it holds none.
    pub runtime_id: Option<String>,
    /// Whether Core's session catalog marks the session lifecycle-terminal:
    /// it never resumes.
    pub retired: bool,
    /// Whether the identity owes its host a held completion: its
    /// held-completion mark is present and Core holds no binding of it
    /// since the latest mob reset, the rule the identity runtime reads the
    /// marks by. An ordinary wake refuses such an identity until a held pass
    /// publishes it. A mark beside a binding is one a held pass published
    /// and did not get to remove: it owes nothing, and the next start drops
    /// it.
    pub held_completion_owed: bool,
    /// The identity's row in the first-boot manifest, when one waits for the
    /// gateway's first boot.
    pub pending_first_boot_row: Option<PendingFirstBootRow>,
}

/// A row of the first-boot manifest (`storage-migrate-native.first-boot.json`),
/// spelled as the manifest spells it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PendingFirstBootRow {
    /// The first boot continues the identity on `session`.
    Resume { identity: String, session: String },
    /// The first boot starts the identity fresh; Core's last binding of it
    /// still names `retired_session`.
    Fresh {
        identity: String,
        retired_session: String,
    },
}

impl From<&FirstBootRow> for PendingFirstBootRow {
    fn from(row: &FirstBootRow) -> Self {
        match row {
            FirstBootRow::Resume { identity, session } => Self::Resume {
                identity: identity.to_string(),
                session: session.to_string(),
            },
            FirstBootRow::Fresh {
                identity,
                retired_session,
            } => Self::Fresh {
                identity: identity.to_string(),
                retired_session: retired_session.to_string(),
            },
        }
    }
}

/// Why the census refused the directory. Nothing was read past the refusal.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum IdentityCensusRefusal {
    /// The state directory does not exist or is not a directory.
    NotADirectory,
    /// A process holds a runtime lock of the directory: stop it, or run the
    /// census on a copy of the directory.
    RuntimeInUse { file: String },
    /// A runtime lock file could not be probed.
    RuntimeLockUnreadable { file: String, detail: String },
    /// The directory holds 0.9.3 storage (the startup predicate's reasons):
    /// `remedy` names the migration that must run first.
    NeedsMigration {
        reasons: Vec<NativeLayoutReason>,
        remedy: String,
    },
    /// A `storage-migrate-native --apply` run started and has not finished:
    /// `remedy` names the run that finishes it.
    MigrationIncomplete { journal: PathBuf, remedy: String },
    /// A file could not be copied into the scratch directory.
    ScratchCopyFailed { file: String, detail: String },
    /// The mob store could not be found or read.
    MobStorageUnreadable { detail: String },
    /// `runtime.sqlite` could not be read.
    RuntimeStoreUnreadable { detail: String },
    /// The continuity records, the first-boot manifest or the
    /// held-completion marks could not be read.
    IdentityListUnreadable { detail: String },
}

/// The census of one directory: stable JSON, versioned.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct IdentityCensusReport {
    pub format: &'static str,
    pub version: u32,
    pub state_dir: PathBuf,
    /// The durable mob store the bindings were read from (its path in the
    /// state directory); `None` when the directory has none.
    pub mob_store: Option<PathBuf>,
    /// Why every binding fact is null when the directory has no durable mob
    /// store ([`IN_MEMORY_MOB_STORE_NOTE`]); `None` otherwise.
    pub mob_store_note: Option<&'static str>,
    /// One entry per identity the directory names (Core's durable bindings,
    /// the continuity records, the first-boot manifest's rows and the
    /// held-completion marks), in identity order.
    pub identities: Vec<IdentityCensusEntry>,
    /// Why the census refused the directory; `identities` is then empty.
    pub refusal: Option<IdentityCensusRefusal>,
}

impl IdentityCensusReport {
    fn new(state_dir: &Path) -> Self {
        Self {
            format: IDENTITY_CENSUS_FORMAT,
            version: IDENTITY_CENSUS_VERSION,
            state_dir: state_dir.to_path_buf(),
            mob_store: None,
            mob_store_note: None,
            identities: Vec::new(),
            refusal: None,
        }
    }

    fn refused(mut self, refusal: IdentityCensusRefusal) -> Self {
        self.refusal = Some(refusal);
        self
    }
}

/// Run the census of `state_dir`. The scratch copies go under `scratch_dir`,
/// or the system temporary directory, and are removed when it returns.
pub async fn identity_census_offline(
    state_dir: &Path,
    scratch_dir: Option<&Path>,
) -> IdentityCensusReport {
    let report = IdentityCensusReport::new(state_dir);
    if !state_dir.is_dir() {
        return report.refused(IdentityCensusRefusal::NotADirectory);
    }
    let scratch = match scratch_dir {
        Some(dir) => tempfile::Builder::new()
            .prefix(".meta-identity-census-")
            .tempdir_in(dir),
        None => tempfile::Builder::new()
            .prefix("meta-identity-census-")
            .tempdir(),
    };
    let scratch = match scratch {
        Ok(scratch) => scratch,
        Err(error) => {
            return report.refused(IdentityCensusRefusal::ScratchCopyFailed {
                file: String::new(),
                detail: format!("scratch directory: {error}"),
            });
        }
    };
    let held_marks = {
        // Held only while copying and reading the marks: a runtime that
        // starts meanwhile could change the files under the copy.
        let _exclusion = match exclude_live_runtimes(state_dir) {
            Ok(exclusion) => exclusion,
            Err(NativeMigrateRefusal::RuntimeInUse { file }) => {
                return report.refused(IdentityCensusRefusal::RuntimeInUse { file });
            }
            Err(NativeMigrateRefusal::RuntimeLockUnreadable { file, detail }) => {
                return report
                    .refused(IdentityCensusRefusal::RuntimeLockUnreadable { file, detail });
            }
            Err(other) => {
                return report.refused(IdentityCensusRefusal::RuntimeLockUnreadable {
                    file: String::new(),
                    detail: format!("{other:?}"),
                });
            }
        };
        if let Err(refusal) = copy_top_level_files(state_dir, scratch.path()) {
            return report.refused(refusal);
        }
        HeldCompletionMarks::in_state_dir(state_dir).read()
    };
    let copy = scratch.path();

    match native_layout_status(copy) {
        NativeLayoutStatus::Native => {}
        NativeLayoutStatus::NeedsMigration { reasons } => {
            return report.refused(IdentityCensusRefusal::NeedsMigration {
                reasons: reasons
                    .into_iter()
                    .map(|reason| reason_in_state_dir(reason, copy, state_dir))
                    .collect(),
                remedy: format!(
                    "migrate it first: meta_gateway storage-migrate-native --state-dir {} \
                     (a dry run), then the same with --apply",
                    state_dir.display()
                ),
            });
        }
        NativeLayoutStatus::MigrationIncomplete { journal } => {
            return report.refused(IdentityCensusRefusal::MigrationIncomplete {
                journal: journal
                    .file_name()
                    .map_or_else(|| state_dir.to_path_buf(), |name| state_dir.join(name)),
                remedy: format!(
                    "finish it: meta_gateway storage-migrate-native --state-dir {} --apply",
                    state_dir.display()
                ),
            });
        }
    }
    let held_marks = match held_marks {
        Ok(marks) => marks,
        Err(error) => {
            return report.refused(IdentityCensusRefusal::IdentityListUnreadable {
                detail: format!("held-completion marks: {error}"),
            });
        }
    };

    let mut report = report;
    let history = match state_dir_mob_store(copy) {
        Ok(Some(mob_store)) => {
            report.mob_store = mob_store.file_name().map(|name| state_dir.join(name));
            match meerkat_mob::MobStorage::read_member_binding_history(&mob_store).await {
                Ok(history) => Some(history),
                Err(error) => {
                    return report.refused(IdentityCensusRefusal::MobStorageUnreadable {
                        detail: error.to_string(),
                    });
                }
            }
        }
        Ok(None) => {
            report.mob_store_note = Some(IN_MEMORY_MOB_STORE_NOTE);
            None
        }
        Err(detail) => {
            return report.refused(IdentityCensusRefusal::MobStorageUnreadable { detail });
        }
    };

    // Every identity the directory names: Core's durable identities (decoded
    // from Core's roster ids the way the startup probe decodes them), the
    // continuity records, the first-boot manifest's rows and the
    // held-completion marks.
    let mut identities: BTreeSet<String> = BTreeSet::new();
    if let Some(history) = &history {
        identities.extend(
            history
                .durable_identity_bindings()
                .map(|binding| durable_identity_for_roster_member(binding.identity().as_ref())),
        );
    }
    match continuity_identities(copy) {
        Ok(found) => identities.extend(found),
        Err(detail) => {
            return report.refused(IdentityCensusRefusal::IdentityListUnreadable { detail });
        }
    }
    let mut pending: BTreeMap<String, PendingFirstBootRow> = BTreeMap::new();
    match crate::identity_first::FirstBootManifest::read(copy) {
        Ok(Some(manifest)) => {
            for row in manifest.rows() {
                pending.insert(row.identity().to_string(), PendingFirstBootRow::from(row));
            }
        }
        Ok(None) => {}
        Err(error) => {
            return report.refused(IdentityCensusRefusal::IdentityListUnreadable {
                detail: format!("first-boot manifest: {error}"),
            });
        }
    }
    identities.extend(pending.keys().cloned());
    let held: BTreeSet<String> = held_marks
        .iter()
        .map(|identity| identity.as_str().to_owned())
        .collect();
    identities.extend(held.iter().cloned());

    // Core's last binding of each, read the way the gateway's identity views
    // read it.
    let bindings: Vec<(String, Option<&meerkat_mob::MemberBinding>)> = identities
        .into_iter()
        .map(|identity| {
            let binding = history.as_ref().and_then(|history| {
                history.last_binding(&roster_member_id_for_identity(&identity))
            });
            (identity, binding)
        })
        .collect();
    // runtime.sqlite is opened only when a binding names a session to look
    // up in it.
    let runtime_db = copy.join(RUNTIME_DB_FILE_NAME);
    let sessions_bound = bindings
        .iter()
        .any(|(_, binding)| binding.and_then(|binding| binding.session_id()).is_some());
    let runtime_store = if sessions_bound && runtime_db.is_file() {
        match SqliteRuntimeStore::new_head_canonical(runtime_db) {
            Ok(store) => Some(store),
            Err(error) => {
                return report.refused(IdentityCensusRefusal::RuntimeStoreUnreadable {
                    detail: error.to_string(),
                });
            }
        }
    } else {
        None
    };

    let mut entries = Vec::with_capacity(bindings.len());
    for (identity, binding) in bindings {
        let session = binding.and_then(|binding| binding.session_id());
        let (runtime_id, retired) = match (session, &runtime_store) {
            (Some(session), Some(store)) => match session_runtime_facts(store, session).await {
                Ok(facts) => facts,
                Err(detail) => {
                    return report
                        .refused(IdentityCensusRefusal::RuntimeStoreUnreadable { detail });
                }
            },
            _ => (None, false),
        };
        entries.push(IdentityCensusEntry {
            session_id: session.map(ToString::to_string),
            generation: binding.map(|binding| binding.generation().get()),
            member_id: binding.map(|binding| binding.identity().as_str().to_owned()),
            live: binding.is_some_and(meerkat_mob::MemberBinding::is_live),
            agent_runtime_id: binding
                .and_then(crate::identity_first::live_binding_runtime_id)
                .map(|runtime_id| runtime_id.to_string()),
            runtime_id,
            retired,
            held_completion_owed: binding.is_none() && held.contains(&identity),
            pending_first_boot_row: pending.remove(&identity),
            identity,
        });
    }
    report.identities = entries;
    report
}

/// The runtime id under which `store` holds `session`'s authority, and
/// whether Core's catalog marks the session lifecycle-terminal, read from
/// one observation per runtime id: Core's session runtime id first, then the
/// legacy alias. With no authority under either, the first catalog entry
/// found says whether the session is terminal.
async fn session_runtime_facts(
    store: &SqliteRuntimeStore,
    session: &SessionId,
) -> Result<(Option<String>, bool), String> {
    let mut catalog_terminal = None;
    for runtime_id in [
        LogicalRuntimeId::for_session(session),
        LogicalRuntimeId::legacy_session_uuid_alias(session),
    ] {
        let observation = store
            .load_session_resume_observation(&runtime_id)
            .await
            .map_err(|error| format!("{runtime_id}: {error}"))?;
        let terminal = observation
            .catalog_entry()
            .map(|entry| entry.lifecycle_terminal().is_some());
        if observation.session_authority().is_some() {
            return Ok((Some(runtime_id.to_string()), terminal.unwrap_or(false)));
        }
        catalog_terminal = catalog_terminal.or(terminal);
    }
    Ok((None, catalog_terminal.unwrap_or(false)))
}

/// Every identity a continuity record names, bound to a session or not, in
/// each continuity database spelling the copy holds.
fn continuity_identities(copy: &Path) -> Result<Vec<String>, String> {
    let mut identities = Vec::new();
    let names = std::iter::once(CANONICAL_CONTINUITY_DB_FILE_NAME)
        .chain(DatabaseSlot::Continuity.legacy_names().iter().copied());
    for name in names {
        let path = copy.join(name);
        if !path.is_file() {
            continue;
        }
        let unreadable = |error: rusqlite::Error| format!("continuity records in {name}: {error}");
        let conn = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(unreadable)?;
        let present: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = \
                 'continuity_records')",
                [],
                |row| row.get(0),
            )
            .map_err(unreadable)?;
        if !present {
            continue;
        }
        let mut statement = conn
            .prepare("SELECT identity FROM continuity_records")
            .map_err(unreadable)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(unreadable)?;
        for row in rows {
            identities.push(row.map_err(unreadable)?);
        }
    }
    Ok(identities)
}

/// A layout reason read on the copy, naming the state directory where it
/// named the copy.
fn reason_in_state_dir(
    reason: NativeLayoutReason,
    copy: &Path,
    state_dir: &Path,
) -> NativeLayoutReason {
    match reason {
        NativeLayoutReason::Unreadable { file, detail } if Path::new(&file) == copy => {
            NativeLayoutReason::Unreadable {
                file: state_dir.display().to_string(),
                detail,
            }
        }
        other => other,
    }
}

/// Copy every top-level regular file of `state_dir` into `scratch`, except
/// SQLite's shared-memory indexes (SQLite rebuilds them from the logs) and
/// the registered backups and quarantined files (nothing here reads them,
/// and the startup predicate's scan skips them too). The startup predicate
/// and the census read the copies exactly as they would read the directory.
pub(crate) fn copy_top_level_files(
    state_dir: &Path,
    scratch: &Path,
) -> Result<(), IdentityCensusRefusal> {
    let unreadable =
        |file: String, detail: String| IdentityCensusRefusal::ScratchCopyFailed { file, detail };
    let entries = std::fs::read_dir(state_dir)
        .map_err(|error| unreadable(state_dir.display().to_string(), error.to_string()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| unreadable(state_dir.display().to_string(), error.to_string()))?;
        let path = entry.path();
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if !path.is_file()
            || name_text.ends_with("-shm")
            || crate::storage_migrate::is_registered_backup_artifact_name(&name_text)
            || crate::storage_migrate::is_registered_quarantine_artifact_name(&name_text)
        {
            continue;
        }
        let copied = scratch.join(&name);
        std::fs::copy(&path, &copied)
            .map_err(|error| unreadable(path.display().to_string(), error.to_string()))?;
        // The copy keeps the source's mode; a read-only one would keep
        // SQLite from recovering its logs on the copy.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&copied, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| unreadable(path.display().to_string(), error.to_string()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// The copy leaves out what nothing reads: the shared-memory indexes,
    /// the registered backups (each `--apply` keeps a set as large as the
    /// databases) and the quarantined files. Logs, journals and the other
    /// top-level files are copied; subdirectories are not.
    #[test]
    fn the_copy_skips_backups_quarantined_files_and_shared_memory_indexes() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "runtime.sqlite",
            "runtime.sqlite-wal",
            "runtime.sqlite-shm",
            "runtime.sqlite-journal",
            "runtime.sqlite.pre-0.9.3-1760000000.native-migration",
            "runtime.sqlite-wal.pre-0.9.3-1760000000.native-migration",
            "storage-migrate-native.first-boot.json",
            "storage-migrate-native.first-boot.json.pre-0.9.4-1760000001.first-boot",
            "continuity.sqlite3.corrupt-1760000002",
            "event_log.sqlite3",
            "notes.pre-release",
        ] {
            std::fs::write(dir.path().join(name), name).unwrap();
        }
        std::fs::create_dir(dir.path().join("held-completion-owed")).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        copy_top_level_files(dir.path(), scratch.path()).unwrap();
        let copied: BTreeSet<String> = std::fs::read_dir(scratch.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            copied,
            [
                "event_log.sqlite3",
                "notes.pre-release",
                "runtime.sqlite",
                "runtime.sqlite-journal",
                "runtime.sqlite-wal",
                "storage-migrate-native.first-boot.json",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
        );
        for name in &copied {
            assert_eq!(
                std::fs::read(scratch.path().join(name)).unwrap(),
                name.as_bytes()
            );
        }
    }

    /// A pending row is spelled exactly as the manifest spells it.
    #[test]
    fn a_pending_first_boot_row_is_the_manifest_row() {
        let session = SessionId::parse("0192f0a0-0000-7000-8000-000000000002").expect("session id");
        let identity =
            |name: &str| crate::identity_first::AgentIdentity::parse(name).expect("identity");
        let dir = tempfile::tempdir().unwrap();
        let rows = vec![
            FirstBootRow::Fresh {
                identity: identity("agent-a"),
                retired_session: session.clone(),
            },
            FirstBootRow::Resume {
                identity: identity("agent-b"),
                session,
            },
        ];
        let manifest = crate::identity_first::FirstBootManifest::write_new(dir.path(), rows)
            .expect("write manifest");
        let written: serde_json::Value = serde_json::from_slice(
            &std::fs::read(crate::identity_first::FirstBootManifest::path(dir.path()))
                .expect("read manifest"),
        )
        .expect("manifest json");
        let ours: Vec<serde_json::Value> = manifest
            .rows()
            .iter()
            .map(|row| serde_json::to_value(PendingFirstBootRow::from(row)).expect("row json"))
            .collect();
        assert_eq!(written["rows"], serde_json::Value::Array(ours));
    }
}
