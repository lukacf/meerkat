//! DDL-free connection opening under named policy profiles.
//!
//! Every profile maps to a policy that already existed somewhere in the
//! workspace; the profiles name those policies instead of erasing them:
//!
//! - [`ConnectionProfile::Primary`]: the production writer policy
//!   (WAL + `synchronous=FULL` + the one shared busy timeout). With
//!   `create: false` it is the no-create writer variant previously
//!   hand-rolled for identity databases. WAL establishment is verified
//!   against the mode SQLite reports back, and the journal-mode conversion
//!   only runs after the [`OpenOptions::schema_preflight`] ledger check, so
//!   an old binary refuses a future file before mutating it.
//! - [`ConnectionProfile::ReadOnly`]: passive observation
//!   (`SQLITE_OPEN_READ_ONLY | URI | NO_MUTEX`, shared busy timeout, no
//!   pragma mutation — a reader must not convert a foreign file's journal
//!   mode). The precise no-write guarantee is typed as
//!   [`WriteContact::ReadOnlyWalSidecars`]: logical content is never
//!   altered, but WAL sidecars may be required to read a WAL database.
//! - [`ConnectionProfile::Maintenance`]: fail-fast offline access (zero busy
//!   timeout, never creates, no pragma mutation). This is the profile for
//!   migration/diagnostic work performed by the party holding the exclusive
//!   maintenance fence; it deliberately does not take a shared fence guard.
//!
//! Opening a connection never runs schema DDL. Stores apply their
//! [`crate::ledger`] domain after opening.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

use crate::error::SqliteStoreError;
use crate::ledger::SchemaDomain;

/// The one shared busy timeout for production profiles.
///
/// Previously `SQLITE_BUSY_TIMEOUT_MS` was defined six times across the
/// workspace with values of 5s and 60s. 60s is the value the contended
/// multi-writer store (sessions) was deliberately raised to; the harmonized
/// default adopts it. Callers with a deliberate different policy pass an
/// override via [`OpenOptions::busy_timeout`] so the decision stays named at
/// the call site.
pub const SHARED_BUSY_TIMEOUT: Duration = Duration::from_millis(60_000);

/// Upper bound on the busy timeout actually handed to SQLite.
///
/// `sqlite3_busy_timeout` takes an `int` of milliseconds and rusqlite's
/// conversion panics past `i32::MAX`; a panic in library code is not an
/// acceptable outcome for an oversized configuration value, so anything
/// above this bound (~24.8 days) is clamped to it.
const MAX_BUSY_TIMEOUT: Duration = Duration::from_millis(i32::MAX as u64);

/// Named connection policy profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionProfile {
    /// Production read-write policy: WAL, `synchronous=FULL`, shared busy
    /// timeout. `create: true` creates parent directories and the file;
    /// `create: false` refuses to create (no-create writer, for files whose
    /// creation is owned elsewhere).
    Primary { create: bool },
    /// Passive read-only observation. Never creates the database file, never
    /// mutates pragmas, never alters logical database content. Reading a
    /// database that is in WAL mode may still create or update its
    /// `-wal`/`-shm` sidecar files (see
    /// [`WriteContact::ReadOnlyWalSidecars`]).
    ReadOnly,
    /// Fail-fast maintenance access for offline work: zero busy timeout by
    /// default (a held lock surfaces immediately instead of stalling), never
    /// creates, never mutates pragmas. `write: false` opens read-only.
    Maintenance { write: bool },
}

/// The strongest filesystem no-write guarantee an open under a profile can
/// make. Diagnostic surfaces (doctor) report from this instead of promising
/// a zero-touch open that SQLite cannot deliver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteContact {
    /// The profile writes: mutating pragmas at open (journal-mode
    /// conversion) and/or subsequent DDL/DML.
    ReadWrite,
    /// Logical database content is never modified, but SQLite may still
    /// create or update the `-wal`/`-shm` sidecar files when the database is
    /// in WAL mode: coherent WAL reads require the shared-memory index, and
    /// rusqlite offers no way around that short of `immutable=1`, which is
    /// only sound when no live writer can exist and is therefore not used
    /// here. On truly read-only media a WAL database whose sidecars are
    /// absent fails to read (typed SQLite error) instead of being mutated;
    /// rollback-journal databases are read with no sidecar contact at all.
    ReadOnlyWalSidecars,
}

impl ConnectionProfile {
    /// Convenience: the common create-capable primary profile.
    pub const PRIMARY: Self = Self::Primary { create: true };

    fn name(self) -> &'static str {
        match self {
            Self::Primary { create: true } => "primary",
            Self::Primary { create: false } => "primary(no-create)",
            Self::ReadOnly => "read-only",
            Self::Maintenance { write: true } => "maintenance(write)",
            Self::Maintenance { write: false } => "maintenance(read)",
        }
    }

    fn default_busy_timeout(self) -> Duration {
        match self {
            Self::Primary { .. } | Self::ReadOnly => SHARED_BUSY_TIMEOUT,
            Self::Maintenance { .. } => Duration::ZERO,
        }
    }

    /// The strongest no-write guarantee an open under this profile makes.
    pub fn write_contact(self) -> WriteContact {
        match self {
            Self::Primary { .. } | Self::Maintenance { write: true } => WriteContact::ReadWrite,
            Self::ReadOnly | Self::Maintenance { write: false } => {
                WriteContact::ReadOnlyWalSidecars
            }
        }
    }
}

/// Per-open overrides. The zero-value default defers everything to the
/// profile.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenOptions {
    /// Override the profile's busy timeout. A store with a deliberate
    /// contention policy (for example a lock-arbitration sidecar that waits
    /// 30s then fails closed) names it here instead of hand-rolling an
    /// opener. Values beyond SQLite's `int` millisecond range are clamped to
    /// `i32::MAX` milliseconds.
    pub busy_timeout: Option<Duration>,
    /// Schema domains checked against the file's `meerkat_schema` ledger
    /// BEFORE any mutating pragma runs. A file whose ledger records a
    /// version newer than a listed domain supports is refused
    /// ([`SqliteStoreError::SchemaFromTheFuture`]) with its logical content
    /// unmodified — sidecars may still be touched, because reading the
    /// ledger of a WAL-mode file over a read-write connection contacts its
    /// `-wal`/`-shm` files (see [`WriteContact::ReadOnlyWalSidecars`]).
    /// Without the preflight, a Primary open by an old binary would convert
    /// a future database's journal mode before the ledger refusal in
    /// [`crate::ledger::apply_domain_migrations`] ever fires. The check runs
    /// unconditionally after every open — never gated on a pre-open
    /// filesystem probe, which a concurrent creator could race — and passes
    /// trivially on files with no ledger table (fresh creates, pre-ledger
    /// files). Empty (the default) skips the preflight.
    pub schema_preflight: &'static [&'static SchemaDomain],
}

/// Open `path` under `profile` with the profile's defaults.
pub fn open(path: &Path, profile: ConnectionProfile) -> Result<Connection, SqliteStoreError> {
    open_with(path, profile, OpenOptions::default())
}

/// Open `path` under `profile` with per-open overrides.
pub fn open_with(
    path: &Path,
    profile: ConnectionProfile,
    options: OpenOptions,
) -> Result<Connection, SqliteStoreError> {
    let conn = match profile {
        ConnectionProfile::Primary { create: true } => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            Connection::open(path)?
        }
        ConnectionProfile::Primary { create: false } => {
            open_existing(path, profile, OpenFlags::SQLITE_OPEN_READ_WRITE)?
        }
        ConnectionProfile::ReadOnly => {
            open_existing(path, profile, OpenFlags::SQLITE_OPEN_READ_ONLY)?
        }
        ConnectionProfile::Maintenance { write } => {
            let base = if write {
                OpenFlags::SQLITE_OPEN_READ_WRITE
            } else {
                OpenFlags::SQLITE_OPEN_READ_ONLY
            };
            open_existing(path, profile, base)?
        }
    };

    let busy = options
        .busy_timeout
        .unwrap_or_else(|| profile.default_busy_timeout())
        .min(MAX_BUSY_TIMEOUT);
    conn.busy_timeout(busy)?;

    // The future-schema preflight runs before any mutating pragma: a refusal
    // leaves a foreign (newer) file's logical content unmodified (WAL-mode
    // files may see sidecar contact from the ledger read). It is not gated
    // on a pre-open filesystem probe — a database created by another process
    // between such a probe and the open would dodge the check. A fresh
    // create carries no ledger table and passes inside
    // `refuse_future_schema` itself.
    for domain in options.schema_preflight {
        crate::ledger::refuse_future_schema(&conn, domain)?;
    }

    if let ConnectionProfile::Primary { .. } = profile {
        set_wal_journal_mode(&conn, path, busy)?;
        conn.pragma_update(None, "synchronous", "FULL")?;
    }

    Ok(conn)
}

/// Convert (or confirm) the WAL journal mode with a bounded retry, verified
/// against the effective mode SQLite reports back.
///
/// Converting a fresh rollback-journal database to WAL needs an exclusive
/// lock, and SQLite can return `SQLITE_BUSY` from the journal-mode pragma
/// WITHOUT consulting the busy handler while concurrent creators race the
/// conversion. Once a file is WAL the pragma is a lock-free no-op, so the
/// retry only ever spins during the first-create race.
fn set_wal_journal_mode(
    conn: &Connection,
    path: &Path,
    busy_timeout: Duration,
) -> Result<(), SqliteStoreError> {
    // Bound the retry by the connection's busy policy (with a small floor so
    // zero-timeout profiles still tolerate the momentary create race).
    let deadline = std::time::Instant::now() + busy_timeout.max(Duration::from_millis(250));
    loop {
        // `pragma_update` would discard the mode the pragma returns, and
        // SQLite can decline the conversion without raising an error — so
        // the returned mode is read back and anything that is not WAL is a
        // typed failure. In-memory databases have no on-disk journal and
        // always report `memory`; they are accepted as-is.
        let result = conn
            .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0));
        match result {
            Ok(mode) if mode.eq_ignore_ascii_case("wal") || mode.eq_ignore_ascii_case("memory") => {
                return Ok(());
            }
            Ok(mode) => {
                return Err(SqliteStoreError::WalNotEstablished {
                    path: path.to_path_buf(),
                    actual: mode,
                });
            }
            Err(error)
                if crate::error::is_busy_or_locked(&error)
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn open_existing(
    path: &Path,
    profile: ConnectionProfile,
    base: OpenFlags,
) -> Result<Connection, SqliteStoreError> {
    if !path.is_file() {
        return Err(SqliteStoreError::OpenRefused {
            path: path.to_path_buf(),
            profile: profile.name(),
            detail: "database file does not exist".to_string(),
        });
    }
    Ok(Connection::open_with_flags(
        path,
        base | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?)
}

/// Begin an IMMEDIATE transaction. Bounded waiting for the write lock is the
/// connection busy handler's job (set at open per profile), not a caller
/// retry loop.
pub fn begin_immediate(conn: &mut Connection) -> Result<Transaction<'_>, SqliteStoreError> {
    Ok(conn.transaction_with_behavior(TransactionBehavior::Immediate)?)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn primary_creates_dirs_sets_wal_and_full() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/sub/test.sqlite3");
        let conn = open(&path, ConnectionProfile::PRIMARY).expect("open");
        let journal: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .expect("journal_mode");
        assert_eq!(journal, "wal");
        let sync: i64 = conn
            .pragma_query_value(None, "synchronous", |r| r.get(0))
            .expect("synchronous");
        assert_eq!(sync, 2, "synchronous=FULL");
    }

    #[test]
    fn non_creating_profiles_refuse_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("missing.sqlite3");
        for profile in [
            ConnectionProfile::Primary { create: false },
            ConnectionProfile::ReadOnly,
            ConnectionProfile::Maintenance { write: true },
            ConnectionProfile::Maintenance { write: false },
        ] {
            let err = open(&path, profile).expect_err("must refuse missing file");
            assert!(
                matches!(err, SqliteStoreError::OpenRefused { .. }),
                "{profile:?}"
            );
        }
        assert!(
            !path.exists(),
            "no profile may create the file as a side effect"
        );
    }

    #[test]
    fn read_only_profile_rejects_writes_and_preserves_journal_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("db.sqlite3");
        {
            let conn = Connection::open(&path).expect("create");
            conn.execute_batch("CREATE TABLE t (x INTEGER)")
                .expect("ddl");
        }
        let conn = open(&path, ConnectionProfile::ReadOnly).expect("open ro");
        let journal: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .expect("journal_mode");
        assert_eq!(
            journal, "delete",
            "reader must not convert the journal mode"
        );
        conn.execute("INSERT INTO t VALUES (1)", [])
            .expect_err("read-only connection must reject writes");
    }

    #[test]
    fn maintenance_profile_fails_fast_on_held_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("db.sqlite3");
        let mut writer = open(&path, ConnectionProfile::PRIMARY).expect("writer");
        writer
            .execute_batch("CREATE TABLE t (x INTEGER)")
            .expect("ddl");
        let tx = begin_immediate(&mut writer).expect("hold write lock");

        let maint = open(&path, ConnectionProfile::Maintenance { write: true }).expect("open");
        let err = maint
            .execute_batch("BEGIN IMMEDIATE")
            .expect_err("zero busy timeout must surface the held lock immediately");
        assert!(crate::error::is_busy_or_locked(&match err {
            rusqlite::Error::SqliteFailure(..) => err,
            other => panic!("unexpected error shape: {other}"),
        }));
        drop(tx);
    }

    #[test]
    fn busy_timeout_override_applies() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("db.sqlite3");
        let conn = open_with(
            &path,
            ConnectionProfile::PRIMARY,
            OpenOptions {
                busy_timeout: Some(Duration::from_millis(1234)),
                ..OpenOptions::default()
            },
        )
        .expect("open");
        let timeout: i64 = conn
            .pragma_query_value(None, "busy_timeout", |r| r.get(0))
            .expect("busy_timeout");
        assert_eq!(timeout, 1234);
    }

    #[test]
    fn oversized_busy_timeout_is_clamped_not_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("db.sqlite3");
        let conn = open_with(
            &path,
            ConnectionProfile::PRIMARY,
            OpenOptions {
                busy_timeout: Some(Duration::MAX),
                ..OpenOptions::default()
            },
        )
        .expect("oversized timeout must clamp, not panic");
        let timeout: i64 = conn
            .pragma_query_value(None, "busy_timeout", |r| r.get(0))
            .expect("busy_timeout");
        assert_eq!(timeout, i64::from(i32::MAX));
    }

    fn preflight_base(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
        tx.execute_batch("CREATE TABLE IF NOT EXISTS preflight_t (x INTEGER)")
    }

    const PREFLIGHT_DOMAIN: SchemaDomain = SchemaDomain {
        name: "preflight-domain",
        migrations: &[crate::ledger::Migration {
            version: 1,
            name: "base",
            apply: preflight_base,
        }],
    };

    #[test]
    fn schema_preflight_refuses_future_file_before_wal_conversion() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("db.sqlite3");
        {
            // A rollback-journal file stamped by a "newer binary". The
            // create-capable profile below must refuse it all the same:
            // the preflight consults only the opened connection's ledger,
            // so a file created by another process at any point before the
            // open (there is no pre-open existence probe to race) is
            // classified by its content, not by who created it.
            let conn = Connection::open(&path).expect("create raw");
            conn.execute_batch(
                "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL);
                 INSERT INTO meerkat_schema VALUES ('preflight-domain', 999);",
            )
            .expect("seed future ledger");
        }
        let err = open_with(
            &path,
            ConnectionProfile::PRIMARY,
            OpenOptions {
                schema_preflight: &[&PREFLIGHT_DOMAIN],
                ..OpenOptions::default()
            },
        )
        .expect_err("future file must be refused");
        assert!(matches!(
            err,
            SqliteStoreError::SchemaFromTheFuture {
                found: 999,
                supported: 1,
                ..
            }
        ));
        // The refusal fired before the mutating pragma: the file still has
        // its original rollback journal mode.
        let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("reopen raw");
        let journal: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .expect("journal_mode");
        assert_eq!(journal, "delete");
    }

    #[test]
    fn schema_preflight_allows_fresh_create_and_current_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fresh.sqlite3");
        let options = OpenOptions {
            schema_preflight: &[&PREFLIGHT_DOMAIN],
            ..OpenOptions::default()
        };
        // The preflight runs on fresh creates too (no filesystem-based
        // exemption); it passes because the new file has no ledger table.
        let mut conn = open_with(&path, ConnectionProfile::PRIMARY, options)
            .expect("fresh create carries no ledger table and passes preflight");
        crate::ledger::apply_domain_migrations(&mut conn, &PREFLIGHT_DOMAIN).expect("stamp");
        drop(conn);
        open_with(&path, ConnectionProfile::PRIMARY, options)
            .expect("current file passes preflight");
    }

    #[test]
    fn primary_in_memory_reports_memory_journal_and_opens() {
        // In-memory databases cannot hold WAL; the verified journal-mode
        // setup accepts their `memory` reading instead of refusing them.
        let conn = open(Path::new(":memory:"), ConnectionProfile::PRIMARY).expect("open");
        let journal: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .expect("journal_mode");
        assert_eq!(journal, "memory");
    }

    #[test]
    fn write_contact_names_the_wal_sidecar_caveat() {
        assert_eq!(
            ConnectionProfile::PRIMARY.write_contact(),
            WriteContact::ReadWrite
        );
        assert_eq!(
            ConnectionProfile::Primary { create: false }.write_contact(),
            WriteContact::ReadWrite
        );
        assert_eq!(
            ConnectionProfile::Maintenance { write: true }.write_contact(),
            WriteContact::ReadWrite
        );
        assert_eq!(
            ConnectionProfile::ReadOnly.write_contact(),
            WriteContact::ReadOnlyWalSidecars
        );
        assert_eq!(
            ConnectionProfile::Maintenance { write: false }.write_contact(),
            WriteContact::ReadOnlyWalSidecars
        );
    }
}
