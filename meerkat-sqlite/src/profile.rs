//! DDL-free connection opening under named policy profiles.
//!
//! Every profile maps to a policy that already existed somewhere in the
//! workspace; the profiles name those policies instead of erasing them:
//!
//! - [`ConnectionProfile::Primary`]: the production writer policy
//!   (WAL + `synchronous=FULL` + the one shared busy timeout). With
//!   `create: false` it is the no-create writer variant previously
//!   hand-rolled for identity databases.
//! - [`ConnectionProfile::ReadOnly`]: passive observation
//!   (`SQLITE_OPEN_READ_ONLY | URI | NO_MUTEX`, shared busy timeout, no
//!   pragma mutation — a reader must not convert a foreign file's journal
//!   mode).
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

/// The one shared busy timeout for production profiles.
///
/// Previously `SQLITE_BUSY_TIMEOUT_MS` was defined six times across the
/// workspace with values of 5s and 60s. 60s is the value the contended
/// multi-writer store (sessions) was deliberately raised to; the harmonized
/// default adopts it. Callers with a deliberate different policy pass an
/// override via [`OpenOptions::busy_timeout`] so the decision stays named at
/// the call site.
pub const SHARED_BUSY_TIMEOUT: Duration = Duration::from_millis(60_000);

/// Named connection policy profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionProfile {
    /// Production read-write policy: WAL, `synchronous=FULL`, shared busy
    /// timeout. `create: true` creates parent directories and the file;
    /// `create: false` refuses to create (no-create writer, for files whose
    /// creation is owned elsewhere).
    Primary { create: bool },
    /// Passive read-only observation. Never creates, never mutates pragmas.
    ReadOnly,
    /// Fail-fast maintenance access for offline work: zero busy timeout by
    /// default (a held lock surfaces immediately instead of stalling), never
    /// creates, never mutates pragmas. `write: false` opens read-only.
    Maintenance { write: bool },
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
}

/// Per-open overrides. The zero-value default defers everything to the
/// profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpenOptions {
    /// Override the profile's busy timeout. A store with a deliberate
    /// contention policy (for example a lock-arbitration sidecar that waits
    /// 30s then fails closed) names it here instead of hand-rolling an
    /// opener.
    pub busy_timeout: Option<Duration>,
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
        .unwrap_or_else(|| profile.default_busy_timeout());
    conn.busy_timeout(busy)?;

    if let ConnectionProfile::Primary { .. } = profile {
        set_wal_journal_mode(&conn, busy)?;
        conn.pragma_update(None, "synchronous", "FULL")?;
    }

    Ok(conn)
}

/// Convert (or confirm) the WAL journal mode with a bounded retry.
///
/// Converting a fresh rollback-journal database to WAL needs an exclusive
/// lock, and SQLite can return `SQLITE_BUSY` from the journal-mode pragma
/// WITHOUT consulting the busy handler while concurrent creators race the
/// conversion. Once a file is WAL the pragma is a lock-free no-op, so the
/// retry only ever spins during the first-create race.
fn set_wal_journal_mode(
    conn: &Connection,
    busy_timeout: Duration,
) -> Result<(), SqliteStoreError> {
    // Bound the retry by the connection's busy policy (with a small floor so
    // zero-timeout profiles still tolerate the momentary create race).
    let deadline = std::time::Instant::now() + busy_timeout.max(Duration::from_millis(250));
    loop {
        match conn.pragma_update(None, "journal_mode", "WAL") {
            Ok(()) => return Ok(()),
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
            },
        )
        .expect("open");
        let timeout: i64 = conn
            .pragma_query_value(None, "busy_timeout", |r| r.get(0))
            .expect("busy_timeout");
        assert_eq!(timeout, 1234);
    }
}
