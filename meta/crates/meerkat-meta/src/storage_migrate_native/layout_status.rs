//! The startup predicate of the native layout (#306).
//!
//! A 0.9.4 runtime does not open a state directory that still holds the
//! storage the 0.9.3 continuity facade wrote; `meta_gateway
//! storage-migrate-native` moves that storage onto Core's stores first.
//! [`native_layout_status`] is the one predicate the gateways and the
//! builder's persistent state ask before they open any store.
//!
//! Every probe opens an existing file read-only and writes no database.
//! Reading a write-ahead-log database whose log is absent, SQLite creates
//! the empty log and its shared-memory index beside it, as any reader does.

use std::fmt::{self, Display, Formatter};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use meerkat_store::sqlite_store::SESSION_STORE_DOMAIN;
use rusqlite::Connection;
use serde::Serialize;

use crate::storage_layout::{
    DatabaseSlot, EVENT_LOG_DB_FILE_NAME, EXTENSION_STATE_DB_FILE_NAME,
    MOB_ANNOUNCEMENTS_DB_FILE_NAME, RUNTIME_DB_FILE_NAME,
};
use crate::workgraph_admission::WORKGRAPH_ADMISSION_SIDECAR_FILE;

/// The journal `meta_gateway storage-migrate-native --apply` keeps in the
/// state directory from its first write until its last step is done. While
/// it exists the directory is mid-migration.
pub const NATIVE_MIGRATION_JOURNAL_FILE_NAME: &str = "storage-migrate-native.journal.json";

/// The schema version a retired projection database is stamped with.
///
/// The migration keeps each projection database it retires as a backup and
/// leaves at the live path a SQLite file holding only the `meerkat_schema`
/// row of the file's schema domain, at this version. A 0.9.3 binary refuses
/// that file as a schema from the future instead of starting empty beside
/// the migrated state; [`native_layout_status`] reads it as retired.
pub const RETIRED_PROJECTION_SCHEMA_VERSION: i64 = 999_999;

/// The schema domain of Meta's continuity database. A literal, so the
/// migration keeps reading 0.9.3 files once the continuity store's own
/// constant is gone; a test pins it to that constant while it exists.
pub const CONTINUITY_SCHEMA_DOMAIN: &str = "meta-continuity";

/// The continuity tables that hold session state: the projection the
/// migration retires. While Meta still keeps identity records in the
/// continuity database, a continuity database without session rows is
/// native.
const CONTINUITY_SESSION_TABLES: [&str; 3] = [
    "session_snapshots",
    "continuity_session_heads",
    "continuity_strand_messages",
];

const RUNTIME_PROFILE_PIN_TABLE: &str = "runtime_head_canonical_profile_pin";
const RUNTIME_WHOLE_BLOB_TABLE: &str = "runtime_whole_blob_authority";
const RUNTIME_ACTIVATION_QUEUE_TABLE: &str = "runtime_head_canonical_activation_queue";
const RUNTIME_ACTIVATIONS_TABLE: &str = "runtime_head_canonical_activations";
const RUNTIME_SESSION_AUTHORITY_TABLE: &str = "runtime_session_authority";
const UNFINISHED_ACTIVATION_FILTER: &str = " WHERE state = 'in_progress'";

/// Where a state directory stands against the native layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NativeLayoutStatus {
    /// Core's stores hold every session; nothing the 0.9.3 facade wrote is
    /// left to migrate. A missing state directory is native.
    Native,
    /// The directory holds 0.9.3 storage: `meta_gateway
    /// storage-migrate-native` must run before a 0.9.4 runtime opens it.
    NeedsMigration { reasons: Vec<NativeLayoutReason> },
    /// A `meta_gateway storage-migrate-native --apply` run started and has
    /// not finished: its journal is still in the directory. Running it again
    /// finishes it.
    MigrationIncomplete { journal: PathBuf },
}

/// Why a state directory needs `meta_gateway storage-migrate-native`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NativeLayoutReason {
    /// The continuity database holds session rows: the projection the 0.9.3
    /// facade kept beside the runtime store when a roster provider was
    /// registered.
    ContinuitySessions { file: String },
    /// A sessions database, the 0.9.3 projection when no roster provider was
    /// registered, is present and not retired.
    SessionsProjection { file: String },
    /// `runtime.sqlite` holds WholeBlob runtimes not converted to Core's
    /// head-canonical profile.
    RuntimeNotConverted {
        /// Whether the file carries the head-canonical profile pin. A
        /// head-canonical open of an unconverted file pins it and then
        /// refuses, so the pin alone does not mean converted.
        pinned: bool,
        /// Runtimes with WholeBlob authority.
        whole_blob_runtimes: u64,
        /// Runtimes queued for head-canonical activation.
        queued_activations: u64,
        /// Activations that started and did not complete.
        unfinished_activations: u64,
    },
    /// A file the predicate must read could not be read. The predicate
    /// fails closed: a file it cannot read is never taken as migrated.
    Unreadable { file: String, detail: String },
    /// A file under a name no Meta release gives a database holds the
    /// session store's schema: a 0.9.3 session database kept under a
    /// `store_path` override, or a copy of one. Neither 0.9.4 nor the
    /// migration reads it, so its sessions would be left behind unseen.
    UnrecognizedSessionDatabase { file: String },
}

impl Display for NativeLayoutReason {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContinuitySessions { file } => {
                write!(f, "{file} holds the 0.9.3 session projection")
            }
            Self::SessionsProjection { file } => {
                write!(f, "{file} is the 0.9.3 session projection")
            }
            Self::RuntimeNotConverted {
                pinned,
                whole_blob_runtimes,
                queued_activations,
                unfinished_activations,
            } => write!(
                f,
                "{RUNTIME_DB_FILE_NAME} holds WholeBlob runtimes not converted to the \
                 head-canonical profile ({whole_blob_runtimes} WholeBlob, \
                 {queued_activations} queued, {unfinished_activations} unfinished, profile {})",
                if *pinned { "pinned" } else { "not pinned" }
            ),
            Self::Unreadable { file, detail } => write!(f, "{file} could not be read: {detail}"),
            Self::UnrecognizedSessionDatabase { file } => write!(
                f,
                "{file} holds a 0.9.3 session database under a name 0.9.4 does not read: {}",
                unrecognized_session_database_remedy(file)
            ),
        }
    }
}

/// The one remedy for a session database under an unknown name, for the
/// startup refusal and the census alike.
pub(crate) fn unrecognized_session_database_remedy(file: &str) -> String {
    format!(
        "run `meta_gateway storage-migrate --state-dir <state_dir> --session-db \
         <state_dir>/{file} --apply` before anything else; if a 0.9.4 runtime has already used \
         the directory, that step refuses, and the file has to be moved out of the state \
         directory (its sessions cannot be brought into a directory 0.9.4 already uses)"
    )
}

impl Display for NativeLayoutStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Native => f.write_str("native"),
            Self::NeedsMigration { reasons } => {
                f.write_str("needs `meta_gateway storage-migrate-native`: ")?;
                for (index, reason) in reasons.iter().enumerate() {
                    if index != 0 {
                        f.write_str("; ")?;
                    }
                    write!(f, "{reason}")?;
                }
                Ok(())
            }
            Self::MigrationIncomplete { journal } => write!(
                f,
                "a `meta_gateway storage-migrate-native --apply` run has not finished (journal \
                 {}); run it again to finish it",
                journal.display()
            ),
        }
    }
}

/// Classify `state_dir` for a 0.9.4 runtime's startup.
///
/// The journal decides first: a started migration is
/// [`NativeLayoutStatus::MigrationIncomplete`] whatever else the directory
/// holds. Otherwise every reason found is reported, in this order:
///
/// 1. a continuity database (any spelling) that is not the retired stamp and
///    holds session rows;
/// 2. a sessions database (any spelling) that is not the retired stamp;
/// 3. a `runtime.sqlite` with WholeBlob runtimes left to convert: WholeBlob
///    authority without the head-canonical profile pin, a runtime queued for
///    activation, or an activation that started and did not complete.
///
/// A file that cannot be read adds [`NativeLayoutReason::Unreadable`]. The
/// probes block on SQLite reads: call this before serving requests, or from
/// a blocking task.
pub fn native_layout_status(state_dir: &Path) -> NativeLayoutStatus {
    let journal = state_dir.join(NATIVE_MIGRATION_JOURNAL_FILE_NAME);
    let mut reasons = Vec::new();
    match present(&journal) {
        Ok(true) => return NativeLayoutStatus::MigrationIncomplete { journal },
        Ok(false) => {}
        Err(detail) => reasons.push(NativeLayoutReason::Unreadable {
            file: NATIVE_MIGRATION_JOURNAL_FILE_NAME.to_owned(),
            detail,
        }),
    }
    for file in spellings(DatabaseSlot::Continuity) {
        probe(state_dir, file, &mut reasons, |conn| {
            let projection =
                !is_retired(conn, CONTINUITY_SCHEMA_DOMAIN)? && holds_session_rows(conn)?;
            Ok(projection.then(|| NativeLayoutReason::ContinuitySessions {
                file: file.to_owned(),
            }))
        });
    }
    for file in spellings(DatabaseSlot::Sessions) {
        probe(state_dir, file, &mut reasons, |conn| {
            let projection = !is_retired(conn, SESSION_STORE_DOMAIN.name)?;
            Ok(projection.then(|| NativeLayoutReason::SessionsProjection {
                file: file.to_owned(),
            }))
        });
    }
    probe(
        state_dir,
        RUNTIME_DB_FILE_NAME,
        &mut reasons,
        runtime_not_converted,
    );
    match unrecognized_session_databases(state_dir) {
        Ok(found) => reasons.extend(found.into_iter().map(|found| match found {
            UnrecognizedFile::SessionDatabase { file } => {
                NativeLayoutReason::UnrecognizedSessionDatabase { file }
            }
            UnrecognizedFile::Unreadable { file, detail } => {
                NativeLayoutReason::Unreadable { file, detail }
            }
        })),
        Err(detail) => reasons.push(NativeLayoutReason::Unreadable {
            file: state_dir.display().to_string(),
            detail,
        }),
    }
    if reasons.is_empty() {
        NativeLayoutStatus::Native
    } else {
        NativeLayoutStatus::NeedsMigration { reasons }
    }
}

/// One file the state-directory scan reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UnrecognizedFile {
    /// It holds the session store's schema and is not a retired stamp.
    SessionDatabase { file: String },
    /// It starts like a SQLite database and could not be read: the scan
    /// fails closed.
    Unreadable { file: String, detail: String },
}

/// The one scan of a state directory for session databases under names no
/// Meta release gives a database. The startup
/// predicate and the census both call it.
///
/// It lists the directory one level deep and skips: every name a 0.9.3 or
/// 0.9.4 layout gives a database (each slot's canonical and legacy
/// spellings, `runtime.sqlite`, `event_log.sqlite3`, the extension-state
/// database), the mob announcement database and the workgraph admission
/// sidecar, which Meta opens itself and never reads as a session database;
/// `-wal`, `-shm` and `-journal` siblings; the registered backup,
/// rename-marker and quarantine artifacts; the migration's staged stamp; and
/// every file that does not start with the SQLite header. A candidate is
/// opened read-only (never immutable: a row may sit in the log) and closed
/// before the scan returns; a retired stamp is skipped. It writes nothing,
/// except the empty log and index SQLite leaves beside a write-ahead-log
/// database it reads, and holds no lock afterwards.
pub(crate) fn unrecognized_session_databases(
    state_dir: &Path,
) -> Result<Vec<UnrecognizedFile>, String> {
    let entries = match std::fs::read_dir(state_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", state_dir.display())),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", state_dir.display()))?;
        let name = entry.file_name();
        // A store_path override is a JSON string, so its file name is UTF-8.
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_known_state_file(name) {
            continue;
        }
        let path = entry.path();
        let unreadable = |detail: String| UnrecognizedFile::Unreadable {
            file: name.to_owned(),
            detail,
        };
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => continue,
            Err(error) => {
                found.push(unreadable(error.to_string()));
                continue;
            }
        }
        match starts_with_sqlite_header(&path) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(detail) => {
                found.push(unreadable(detail));
                continue;
            }
        }
        match holds_live_session_schema(&path) {
            Ok(true) => found.push(UnrecognizedFile::SessionDatabase {
                file: name.to_owned(),
            }),
            Ok(false) => {}
            Err(detail) => found.push(unreadable(detail)),
        }
    }
    found.sort_by(|left, right| unrecognized_file_name(left).cmp(unrecognized_file_name(right)));
    Ok(found)
}

fn unrecognized_file_name(found: &UnrecognizedFile) -> &str {
    match found {
        UnrecognizedFile::SessionDatabase { file } | UnrecognizedFile::Unreadable { file, .. } => {
            file
        }
    }
}

/// Whether `name` is a file a Meta layout or its maintenance gives a state
/// directory, so the scan leaves it alone.
fn is_known_state_file(name: &str) -> bool {
    name.ends_with("-wal")
        || name.ends_with("-shm")
        || name.ends_with("-journal")
        || name.ends_with(".native-retire")
        || crate::storage_migrate::is_registered_backup_artifact_name(name)
        || crate::storage_migrate::is_registered_quarantine_artifact_name(name)
        || [
            RUNTIME_DB_FILE_NAME,
            EVENT_LOG_DB_FILE_NAME,
            EXTENSION_STATE_DB_FILE_NAME,
            MOB_ANNOUNCEMENTS_DB_FILE_NAME,
            WORKGRAPH_ADMISSION_SIDECAR_FILE,
        ]
        .contains(&name)
        || DatabaseSlot::ALL
            .into_iter()
            .any(|slot| spellings(slot).any(|spelling| spelling == name))
}

/// Whether the file starts with the 16-byte SQLite header.
fn starts_with_sqlite_header(path: &Path) -> Result<bool, String> {
    use std::io::Read as _;
    const HEADER: &[u8; 16] = b"SQLite format 3\0";
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut head = [0_u8; 16];
    match file.read_exact(&mut head) {
        Ok(()) => Ok(&head == HEADER),
        Err(error) if error.kind() == ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

/// Whether `path` is `domain`'s retired stamp, read in one read-only
/// snapshot. A file without a schema ledger is not one.
pub(crate) fn is_retired_stamp_file(path: &Path, domain: &str) -> Result<bool, String> {
    let conn = meerkat_sqlite::open(path, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(|error| error.to_string())?;
    let snapshot = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    if !table_exists(&snapshot, "meerkat_schema")? {
        return Ok(false);
    }
    is_retired(&snapshot, domain)
}

/// Whether a SQLite file holds the session store's schema row and is not
/// its retired stamp.
fn holds_live_session_schema(path: &Path) -> Result<bool, String> {
    let conn = meerkat_sqlite::open(path, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(|error| error.to_string())?;
    let snapshot = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    if !table_exists(&snapshot, "meerkat_schema")? {
        return Ok(false);
    }
    if is_retired(&snapshot, SESSION_STORE_DOMAIN.name)? {
        return Ok(false);
    }
    let version = meerkat_sqlite::domain_version(&snapshot, SESSION_STORE_DOMAIN.name)
        .map_err(|error| error.to_string())?;
    Ok(version.is_some())
}

/// The canonical name of `slot`, then its legacy spellings.
fn spellings(slot: DatabaseSlot) -> impl Iterator<Item = &'static str> {
    std::iter::once(slot.canonical_name()).chain(slot.legacy_names().iter().copied())
}

/// Whether `path` names a directory entry; a dangling link counts.
fn present(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

/// Run `check` over one read snapshot of `state_dir/file`, opened
/// read-only, when the file is present; record its reason, or a
/// [`NativeLayoutReason::Unreadable`] when the file cannot be read.
fn probe(
    state_dir: &Path,
    file: &str,
    reasons: &mut Vec<NativeLayoutReason>,
    check: impl FnOnce(&Connection) -> Result<Option<NativeLayoutReason>, String>,
) {
    let path = state_dir.join(file);
    let outcome = present(&path).and_then(|present| {
        if !present {
            return Ok(None);
        }
        let conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::ReadOnly)
            .map_err(|error| error.to_string())?;
        let snapshot = conn
            .unchecked_transaction()
            .map_err(|error| error.to_string())?;
        check(&snapshot)
    });
    match outcome {
        Ok(Some(reason)) => reasons.push(reason),
        Ok(None) => {}
        Err(detail) => reasons.push(NativeLayoutReason::Unreadable {
            file: file.to_owned(),
            detail,
        }),
    }
}

/// Whether `runtime.sqlite` carries Core's head-canonical durable claim: the
/// profile pin, or any `runtime_session_authority` or
/// `runtime_head_canonical_activations` row (Core's
/// `head_canonical_profile_has_durable_claim`). A 0.9.3 binary refuses a
/// runtime store with the claim, and the conversion writes it first. A
/// directory without the file has none.
pub(crate) fn runtime_conversion_began(state_dir: &Path) -> Result<bool, String> {
    let path = state_dir.join(RUNTIME_DB_FILE_NAME);
    if !present(&path)? {
        return Ok(false);
    }
    let conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let snapshot = conn
        .unchecked_transaction()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(
        has_rows(&snapshot, RUNTIME_PROFILE_PIN_TABLE, " WHERE singleton = 1")?
            || has_rows(&snapshot, RUNTIME_SESSION_AUTHORITY_TABLE, "")?
            || has_rows(&snapshot, RUNTIME_ACTIVATIONS_TABLE, "")?,
    )
}

/// Write `domain`'s retired stamp as the new SQLite file `path`: one
/// `meerkat_schema` table holding the one `(domain, 999999)` row, the shape
/// [`is_retired`] reads and a 0.9.3 binary refuses as a schema from the
/// future. A new file keeps the rollback journal, so the stamp carries no
/// sidecar.
pub(crate) fn write_retired_stamp(path: &Path, domain: &str) -> Result<(), String> {
    let conn = Connection::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    conn.execute_batch(
        "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL);",
    )
    .map_err(|error| format!("{}: {error}", path.display()))?;
    conn.execute(
        "INSERT INTO meerkat_schema (domain, version) VALUES (?1, ?2)",
        rusqlite::params![domain, RETIRED_PROJECTION_SCHEMA_VERSION],
    )
    .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(())
}

/// Whether the file is the retired stamp of `domain`: exactly the
/// `meerkat_schema` table, holding the one `(domain, 999999)` row. A file
/// stamped 999999 that holds anything else cannot be taken as retired and
/// reads as unreadable.
pub(crate) fn is_retired(conn: &Connection, domain: &str) -> Result<bool, String> {
    let version =
        meerkat_sqlite::domain_version(conn, domain).map_err(|error| error.to_string())?;
    if version != Some(RETIRED_PROJECTION_SCHEMA_VERSION) {
        return Ok(false);
    }
    let objects: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM main.sqlite_master WHERE name NOT LIKE 'sqlite!_%' ESCAPE '!'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("schema objects: {error}"))?;
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM main.meerkat_schema", [], |row| {
            row.get(0)
        })
        .map_err(|error| format!("meerkat_schema: {error}"))?;
    if objects == 1 && rows == 1 {
        Ok(true)
    } else {
        Err(format!(
            "stamped {RETIRED_PROJECTION_SCHEMA_VERSION} but holds {objects} schema objects and \
             {rows} ledger rows; a retired stamp holds only its own ledger row"
        ))
    }
}

fn holds_session_rows(conn: &Connection) -> Result<bool, String> {
    for table in CONTINUITY_SESSION_TABLES {
        if has_rows(conn, table, "")? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn runtime_not_converted(conn: &Connection) -> Result<Option<NativeLayoutReason>, String> {
    let pinned = has_rows(conn, RUNTIME_PROFILE_PIN_TABLE, " WHERE singleton = 1")?;
    let whole_blob = has_rows(conn, RUNTIME_WHOLE_BLOB_TABLE, "")?;
    let queued = has_rows(conn, RUNTIME_ACTIVATION_QUEUE_TABLE, "")?;
    let unfinished = has_rows(
        conn,
        RUNTIME_ACTIVATIONS_TABLE,
        UNFINISHED_ACTIVATION_FILTER,
    )?;
    if !(queued || unfinished || (whole_blob && !pinned)) {
        return Ok(None);
    }
    // Counted only for an unconverted file; a native file pays the probes
    // above alone.
    Ok(Some(NativeLayoutReason::RuntimeNotConverted {
        pinned,
        whole_blob_runtimes: count_rows(conn, RUNTIME_WHOLE_BLOB_TABLE, "")?,
        queued_activations: count_rows(conn, RUNTIME_ACTIVATION_QUEUE_TABLE, "")?,
        unfinished_activations: count_rows(
            conn,
            RUNTIME_ACTIVATIONS_TABLE,
            UNFINISHED_ACTIVATION_FILTER,
        )?,
    }))
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM main.sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )
    .map_err(|error| format!("{table}: {error}"))
}

/// Whether `table` has a row matching `filter`; a file without the table
/// has none.
fn has_rows(conn: &Connection, table: &str, filter: &str) -> Result<bool, String> {
    if !table_exists(conn, table)? {
        return Ok(false);
    }
    conn.query_row(
        &format!("SELECT EXISTS (SELECT 1 FROM main.\"{table}\"{filter})"),
        [],
        |row| row.get(0),
    )
    .map_err(|error| format!("{table}: {error}"))
}

fn count_rows(conn: &Connection, table: &str, filter: &str) -> Result<u64, String> {
    if !table_exists(conn, table)? {
        return Ok(0);
    }
    conn.query_row(
        &format!("SELECT COUNT(*) FROM main.\"{table}\"{filter}"),
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count.max(0) as u64)
    .map_err(|error| format!("{table}: {error}"))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::collections::BTreeMap;

    use sha2::{Digest, Sha256};

    use super::*;
    use crate::identity_first::LocalContinuityStore;
    use crate::storage_layout::{
        CANONICAL_CONTINUITY_DB_FILE_NAME, CANONICAL_SESSIONS_DB_FILE_NAME,
    };

    const SHAPES: [&str; 8] = [
        "s1b-plain",
        "s1b-projection-behind",
        "s1b-runtime-missing",
        "s1-roster",
        "s1-projection-behind",
        "s1-runtime-missing",
        "s1e-memory-mob",
        "s1-compact-member-broken-killed",
    ];

    /// An unpacked copy of one 0.9.3 gateway state directory (see
    /// `tests/fixtures/meta_093_gateway_states/README.md`).
    fn unpack(shape: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/meta_093_gateway_states")
            .join(format!("{shape}.tar.gz"));
        let file = std::fs::File::open(&archive)
            .unwrap_or_else(|error| panic!("{}: {error}", archive.display()));
        tar::Archive::new(flate2::read::GzDecoder::new(file))
            .unpack(dir.path())
            .unwrap_or_else(|error| panic!("unpack {shape}: {error}"));
        dir
    }

    /// Replace `file` and its sidecars with the retired stamp of `domain`,
    /// the shape the migration leaves at a retired projection's path.
    fn retire(dir: &Path, file: &str, domain: &str) {
        for suffix in ["", "-wal", "-shm"] {
            let path = dir.join(format!("{file}{suffix}"));
            if path.exists() {
                std::fs::remove_file(&path).unwrap();
            }
        }
        write_retired_stamp(&dir.join(file), domain).unwrap();
    }

    fn reasons(status: NativeLayoutStatus) -> Vec<NativeLayoutReason> {
        match status {
            NativeLayoutStatus::NeedsMigration { reasons } => reasons,
            other => panic!("expected NeedsMigration, got {other:?}"),
        }
    }

    fn projection_reason(shape: &str) -> NativeLayoutReason {
        if shape.starts_with("s1b-") {
            NativeLayoutReason::SessionsProjection {
                file: CANONICAL_SESSIONS_DB_FILE_NAME.to_owned(),
            }
        } else {
            NativeLayoutReason::ContinuitySessions {
                file: CANONICAL_CONTINUITY_DB_FILE_NAME.to_owned(),
            }
        }
    }

    /// The runtime reason of an untouched 0.9.3 `runtime.sqlite`: no pin,
    /// every WholeBlob runtime queued, nothing started.
    fn assert_untouched_093_runtime(reason: &NativeLayoutReason, shape: &str) {
        match reason {
            NativeLayoutReason::RuntimeNotConverted {
                pinned: false,
                whole_blob_runtimes,
                queued_activations,
                unfinished_activations: 0,
            } if *whole_blob_runtimes != 0 && queued_activations == whole_blob_runtimes => {}
            other => panic!("{shape}: unexpected runtime reason {other:?}"),
        }
    }

    /// Every file of `dir` with its sha256, except what any SQLite reader of
    /// a write-ahead-log database may create: the shared-memory index and an
    /// empty log.
    fn snapshot(dir: &Path) -> BTreeMap<String, String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .filter_map(|path| {
                let name = path.file_name()?.to_str()?.to_owned();
                let bytes = std::fs::read(&path).unwrap();
                let reader_sidecar =
                    name.ends_with("-shm") || (name.ends_with("-wal") && bytes.is_empty());
                (!reader_sidecar).then(|| (name, format!("{:x}", Sha256::digest(&bytes))))
            })
            .collect()
    }

    /// The tables of a retired stamp and its one ledger row.
    fn stamp_contents(path: &Path) -> (Vec<String>, Vec<(String, i64)>) {
        let conn = Connection::open(path).unwrap();
        let tables = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<String>, _>>()
            .unwrap();
        let rows = conn
            .prepare("SELECT domain, version FROM meerkat_schema ORDER BY domain")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<(String, i64)>, _>>()
            .unwrap();
        (tables, rows)
    }

    #[test]
    fn a_missing_or_empty_state_directory_is_native() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);
        assert_eq!(
            native_layout_status(&dir.path().join("absent")),
            NativeLayoutStatus::Native
        );
    }

    #[test]
    fn every_093_gateway_shape_needs_migration_for_its_projection_and_runtime_store() {
        for shape in SHAPES {
            let dir = unpack(shape);
            let reasons = reasons(native_layout_status(dir.path()));
            assert_eq!(reasons.len(), 2, "{shape}: {reasons:?}");
            assert_eq!(reasons[0], projection_reason(shape), "{shape}");
            assert_untouched_093_runtime(&reasons[1], shape);
        }
    }

    #[test]
    fn a_retired_projection_leaves_only_the_runtime_conversion() {
        for (shape, file, domain) in [
            (
                "s1b-plain",
                CANONICAL_SESSIONS_DB_FILE_NAME,
                SESSION_STORE_DOMAIN.name,
            ),
            (
                "s1-roster",
                CANONICAL_CONTINUITY_DB_FILE_NAME,
                CONTINUITY_SCHEMA_DOMAIN,
            ),
        ] {
            let dir = unpack(shape);
            retire(dir.path(), file, domain);
            let reasons = reasons(native_layout_status(dir.path()));
            assert_eq!(reasons.len(), 1, "{shape}: {reasons:?}");
            assert_untouched_093_runtime(&reasons[0], shape);
        }
    }

    /// The retired stamp is what keeps a 0.9.3 binary off a migrated
    /// directory: the 0.9.3 session store and Meta's continuity store both
    /// refuse it as a schema from the future. Refusing leaves the stamp a
    /// stamp (the continuity open may switch its journal mode first, and the
    /// session store open leaves its fence file), so it keeps refusing.
    #[test]
    fn the_093_stores_refuse_the_retired_stamp_and_it_stays_retired() {
        let dir = tempfile::tempdir().unwrap();
        let sessions_path = dir.path().join(CANONICAL_SESSIONS_DB_FILE_NAME);
        let continuity_path = dir.path().join(CANONICAL_CONTINUITY_DB_FILE_NAME);
        retire(
            dir.path(),
            CANONICAL_SESSIONS_DB_FILE_NAME,
            SESSION_STORE_DOMAIN.name,
        );
        retire(
            dir.path(),
            CANONICAL_CONTINUITY_DB_FILE_NAME,
            CONTINUITY_SCHEMA_DOMAIN,
        );
        for _ in 0..2 {
            let sessions = meerkat_store::SqliteSessionStore::open(&sessions_path)
                .map(drop)
                .expect_err("the session store refuses the retired stamp");
            assert!(
                sessions.to_string().contains("999999"),
                "names the stamped version: {sessions}"
            );
            let continuity = LocalContinuityStore::open(&continuity_path)
                .map(drop)
                .expect_err("the continuity store refuses the retired stamp");
            assert!(
                continuity.to_string().contains("999999"),
                "names the stamped version: {continuity}"
            );
        }
        for (path, domain) in [
            (&sessions_path, SESSION_STORE_DOMAIN.name),
            (&continuity_path, CONTINUITY_SCHEMA_DOMAIN),
        ] {
            assert_eq!(
                stamp_contents(path),
                (
                    vec!["meerkat_schema".to_owned()],
                    vec![(domain.to_owned(), RETIRED_PROJECTION_SCHEMA_VERSION)]
                ),
                "{}",
                path.display()
            );
        }
        assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);
    }

    #[test]
    fn a_head_canonical_runtime_store_is_native() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join(RUNTIME_DB_FILE_NAME);
        let sessions = meerkat_store::SqliteSessionStore::open(&runtime).unwrap();
        let store = meerkat_runtime::SqliteRuntimeStore::new_head_canonical(&runtime).unwrap();
        drop((store, sessions));
        assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);
    }

    /// A head-canonical open of an unconverted 0.9.3 `runtime.sqlite` (what
    /// a 0.9.4 runtime would do if it skipped this predicate) pins the file
    /// and refuses: the pinned file still needs the migration.
    #[test]
    fn a_refused_head_canonical_open_of_a_093_runtime_store_still_needs_migration() {
        let dir = unpack("s1b-plain");
        let runtime = dir.path().join(RUNTIME_DB_FILE_NAME);
        let sessions = meerkat_store::SqliteSessionStore::open(&runtime).unwrap();
        let refused = meerkat_runtime::SqliteRuntimeStore::new_head_canonical(&runtime)
            .map(drop)
            .expect_err("an unconverted 0.9.3 runtime store refuses a head-canonical open");
        drop(sessions);
        let reasons = reasons(native_layout_status(dir.path()));
        match reasons.as_slice() {
            [
                NativeLayoutReason::SessionsProjection { .. },
                NativeLayoutReason::RuntimeNotConverted { pinned: true, .. },
            ] => {}
            other => panic!("after {refused}: {other:?}"),
        }
    }

    /// A session database under a name no Meta release reads (a 0.9.3
    /// store_path override) keeps 0.9.4 from starting, even on a directory
    /// that is otherwise fresh. Control: without it the directory is native.
    #[test]
    fn a_session_database_under_an_unknown_name_needs_migration() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);
        drop(meerkat_store::SqliteSessionStore::open(dir.path().join("my-sessions.db")).unwrap());
        assert_eq!(
            reasons(native_layout_status(dir.path())),
            vec![NativeLayoutReason::UnrecognizedSessionDatabase {
                file: "my-sessions.db".to_owned()
            }]
        );
        retire(dir.path(), "my-sessions.db", SESSION_STORE_DOMAIN.name);
        assert_eq!(
            native_layout_status(dir.path()),
            NativeLayoutStatus::Native,
            "a retired stamp under any name is the storage-migrate tripwire, not a database"
        );
    }

    /// The scan skips what a migrated directory keeps (registered backups,
    /// rename markers, retired stamps), Meta's own databases (it never opens
    /// the mob announcement database or the workgraph admission sidecar, so a
    /// damaged one is its owner's refusal, not this scan's), unknown files
    /// that are not SQLite, and
    /// SQLite files without the session schema; it fails closed on a SQLite
    /// file it cannot read. It writes nothing.
    #[test]
    fn the_scan_skips_what_a_migrated_directory_keeps_and_fails_closed() {
        let dir = unpack("s1b-plain");
        let path = dir.path();
        std::fs::copy(
            path.join("sessions.sqlite3"),
            path.join("sessions.sqlite3.pre-0.9.4-1760000000.native-migration"),
        )
        .unwrap();
        std::fs::copy(
            path.join("sessions.sqlite3"),
            path.join("sessions.db.pre-0.9.3-1750000000.renamed"),
        )
        .unwrap();
        std::fs::write(path.join("notes.txt"), b"not a database").unwrap();
        let mut corrupt = b"SQLite format 3\0".to_vec();
        corrupt.extend_from_slice(&[0xff; 512]);
        std::fs::write(path.join(MOB_ANNOUNCEMENTS_DB_FILE_NAME), &corrupt).unwrap();
        std::fs::write(path.join(WORKGRAPH_ADMISSION_SIDECAR_FILE), &corrupt).unwrap();
        {
            let conn = Connection::open(path.join("other.sqlite")).unwrap();
            conn.execute_batch("CREATE TABLE t (x INTEGER);").unwrap();
        }
        let before = snapshot(path);
        assert_eq!(unrecognized_session_databases(path).unwrap(), Vec::new());
        assert_eq!(snapshot(path), before, "the scan writes nothing");

        std::fs::write(path.join("broken.db"), corrupt).unwrap();
        assert!(matches!(
            unrecognized_session_databases(path).unwrap().as_slice(),
            [UnrecognizedFile::Unreadable { file, .. }] if file == "broken.db"
        ));
    }

    /// The census and the startup predicate reach the scan through the one
    /// helper: neither keeps a scan of its own.
    #[test]
    fn the_census_and_the_predicate_share_the_one_scan() {
        // The patterns are split so this test's own text never matches them.
        let census = include_str!("census.rs");
        let layout = include_str!("layout_status.rs");
        let call = concat!("unrecognized_session", "_databases(state_dir)");
        let definition = concat!("fn unrecognized_session", "_databases(");
        assert!(census.contains(call));
        assert!(layout.contains(concat!(
            "match unrecognized_session",
            "_databases(state_dir)"
        )));
        assert!(!census.contains(definition));
        assert_eq!(layout.matches(definition).count(), 1);
    }

    #[test]
    fn the_continuity_domain_literal_is_the_continuity_stores_domain() {
        assert_eq!(
            CONTINUITY_SCHEMA_DOMAIN,
            crate::identity_first::local_store::META_CONTINUITY_DOMAIN.name
        );
    }

    /// The two projection files retire separately:
    /// sessions.sqlite3 by its stamp, continuity.sqlite3 by losing its
    /// session rows. Native needs both. Either half alone still needs the
    /// migration, and with the journal present the directory is an
    /// unfinished run whatever else it holds.
    #[test]
    fn a_half_retired_directory_still_needs_migration() {
        let dir = unpack("s1b-plain");
        drop(
            LocalContinuityStore::open(dir.path().join(CANONICAL_CONTINUITY_DB_FILE_NAME)).unwrap(),
        );
        let found = reasons(native_layout_status(dir.path()));
        assert!(
            found
                .iter()
                .any(|reason| matches!(reason, NativeLayoutReason::SessionsProjection { .. })),
            "{found:?}"
        );
        assert!(
            !found
                .iter()
                .any(|reason| matches!(reason, NativeLayoutReason::ContinuitySessions { .. })),
            "{found:?}"
        );

        let dir = unpack("s1-roster");
        retire(
            dir.path(),
            CANONICAL_SESSIONS_DB_FILE_NAME,
            SESSION_STORE_DOMAIN.name,
        );
        let found = reasons(native_layout_status(dir.path()));
        assert!(
            found
                .iter()
                .any(|reason| matches!(reason, NativeLayoutReason::ContinuitySessions { .. })),
            "{found:?}"
        );
        assert!(
            !found
                .iter()
                .any(|reason| matches!(reason, NativeLayoutReason::SessionsProjection { .. })),
            "{found:?}"
        );

        std::fs::write(dir.path().join(NATIVE_MIGRATION_JOURNAL_FILE_NAME), b"{}").unwrap();
        assert!(matches!(
            native_layout_status(dir.path()),
            NativeLayoutStatus::MigrationIncomplete { .. }
        ));
    }

    #[test]
    fn a_continuity_database_without_session_rows_is_native() {
        let dir = tempfile::tempdir().unwrap();
        drop(
            LocalContinuityStore::open(dir.path().join(CANONICAL_CONTINUITY_DB_FILE_NAME)).unwrap(),
        );
        assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);
    }

    /// A native directory in use: the Core pair on runtime.sqlite holding a
    /// session, reopened through the pair, beside a continuity database with
    /// identity records only, the runtime lock, and a retired sessions stamp.
    /// A converted 0.9.3 directory (runtime rows, completed activations) is
    /// the apply's closure test.
    #[tokio::test]
    async fn a_used_native_directory_stays_native_across_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join(RUNTIME_DB_FILE_NAME);
        let session = {
            let mut session = meerkat_core::Session::new();
            session.push(meerkat_core::Message::User(
                meerkat_core::types::UserMessage::text("opening"),
            ));
            session
        };
        for _ in 0..2 {
            let sessions = meerkat_store::SqliteSessionStore::open(&runtime).unwrap();
            let store = meerkat_runtime::SqliteRuntimeStore::new_head_canonical(&runtime).unwrap();
            meerkat::SessionStore::save(&sessions, &session)
                .await
                .unwrap();
            drop((store, sessions));
        }
        drop(
            LocalContinuityStore::open(dir.path().join(CANONICAL_CONTINUITY_DB_FILE_NAME)).unwrap(),
        );
        std::fs::write(dir.path().join("meta-runtime.lock"), b"").unwrap();
        retire(
            dir.path(),
            CANONICAL_SESSIONS_DB_FILE_NAME,
            SESSION_STORE_DOMAIN.name,
        );
        assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);
    }

    /// The stamp is the whole file: a projection that still holds its data
    /// with its ledger row at 999999 is not retired, and fails closed.
    #[test]
    fn a_stamped_version_on_a_database_with_data_is_unreadable() {
        for (shape, file, domain) in [
            (
                "s1b-plain",
                CANONICAL_SESSIONS_DB_FILE_NAME,
                SESSION_STORE_DOMAIN.name,
            ),
            (
                "s1-roster",
                CANONICAL_CONTINUITY_DB_FILE_NAME,
                CONTINUITY_SCHEMA_DOMAIN,
            ),
        ] {
            let dir = unpack(shape);
            Connection::open(dir.path().join(file))
                .unwrap()
                .execute(
                    "UPDATE meerkat_schema SET version = ?1 WHERE domain = ?2",
                    rusqlite::params![RETIRED_PROJECTION_SCHEMA_VERSION, domain],
                )
                .unwrap();
            let reasons = reasons(native_layout_status(dir.path()));
            match &reasons[0] {
                NativeLayoutReason::Unreadable {
                    file: unreadable,
                    detail,
                } if unreadable == file && detail.contains("only its own ledger row") => {}
                other => panic!("{shape}: {other:?} in {reasons:?}"),
            }
        }
    }

    #[test]
    fn legacy_spellings_are_probed_like_the_canonical_names() {
        let dir = unpack("s1-roster");
        for suffix in ["", "-wal"] {
            std::fs::rename(
                dir.path()
                    .join(format!("{CANONICAL_CONTINUITY_DB_FILE_NAME}{suffix}")),
                dir.path().join(format!("continuity.db{suffix}")),
            )
            .unwrap();
        }
        drop(meerkat_store::SqliteSessionStore::open(dir.path().join("sessions.db")).unwrap());
        let reasons = reasons(native_layout_status(dir.path()));
        assert_eq!(
            reasons[..2],
            [
                NativeLayoutReason::ContinuitySessions {
                    file: "continuity.db".to_owned()
                },
                NativeLayoutReason::SessionsProjection {
                    file: "sessions.db".to_owned()
                },
            ],
            "{reasons:?}"
        );
        assert_eq!(reasons.len(), 3, "{reasons:?}");
    }

    #[test]
    fn the_journal_reports_an_unfinished_migration_whatever_else_is_present() {
        let dir = unpack("s1b-plain");
        let journal = dir.path().join(NATIVE_MIGRATION_JOURNAL_FILE_NAME);
        std::fs::write(&journal, b"{}").unwrap();
        assert_eq!(
            native_layout_status(dir.path()),
            NativeLayoutStatus::MigrationIncomplete { journal }
        );
    }

    #[test]
    fn a_database_that_cannot_be_read_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(CANONICAL_CONTINUITY_DB_FILE_NAME),
            b"this is not a SQLite database, and it is longer than one header",
        )
        .unwrap();
        match reasons(native_layout_status(dir.path())).as_slice() {
            [NativeLayoutReason::Unreadable { file, detail }]
                if file == CANONICAL_CONTINUITY_DB_FILE_NAME && !detail.is_empty() => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_predicate_writes_no_database_file() {
        for shape in ["s1b-plain", "s1-roster"] {
            let dir = unpack(shape);
            let before = snapshot(dir.path());
            let _status = native_layout_status(dir.path());
            assert_eq!(snapshot(dir.path()), before, "{shape}");
        }
    }

    #[test]
    fn the_status_text_names_each_reason_and_the_verb() {
        let dir = unpack("s1b-plain");
        let text = native_layout_status(dir.path()).to_string();
        for needle in [
            "meta_gateway storage-migrate-native",
            "sessions.sqlite3 is the 0.9.3 session projection",
            "runtime.sqlite holds WholeBlob runtimes",
            "profile not pinned",
        ] {
            assert!(text.contains(needle), "{needle:?} missing from {text:?}");
        }
    }
}
