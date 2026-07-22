//! Per-file schema migration ledger.
//!
//! Every SQLite file carries a `meerkat_schema(domain TEXT PRIMARY KEY,
//! version INTEGER NOT NULL)` table with exactly one row per schema domain.
//! Each store registers its ordered, idempotent migrations; the existing DDL
//! of every store is migration 0001 of its domain.
//!
//! # The pinned transaction protocol
//!
//! Idempotent migration functions alone do not make concurrent opens safe,
//! so the runner pins a minimal protocol (consumed unchanged by downstream
//! adopters):
//!
//! 1. exactly one ledger row per domain;
//! 2. `BEGIN IMMEDIATE`;
//! 3. re-read the version *inside* that transaction;
//! 4. reject a future version before any mutation
//!    ([`SqliteStoreError::SchemaFromTheFuture`]);
//! 5. execute the pending migrations and the ledger update atomically in the
//!    same transaction.
//!
//! Concurrent opens race safely: the loser's in-transaction re-read sees the
//! winner's committed version and applies nothing.
//!
//! # Migrations on files older than the ledger
//!
//! Files written before the ledger existed carry tables of some historical
//! vintage and no `meerkat_schema` row (version 0). Migration 0001 is the
//! store's `CREATE ... IF NOT EXISTS` DDL, which converges missing tables
//! without touching existing ones; later migrations guard internally (for
//! example `PRAGMA table_info` probes before `ALTER TABLE`) exactly like the
//! historical upgrade functions they were lifted from. Running the full
//! sequence therefore converges a file of any vintage and stamps it — this
//! is also what the offline ledger-baseline migration case does, under the
//! maintenance fence.
//!
//! Foreign domain rows (other stores co-tenanting the same file) are never
//! read or written; the ledger keys strictly by domain name.

use rusqlite::{Connection, OptionalExtension, Transaction};

use crate::error::SqliteStoreError;

const CREATE_LEDGER_SQL: &str = "CREATE TABLE IF NOT EXISTS meerkat_schema (
    domain TEXT PRIMARY KEY,
    version INTEGER NOT NULL
)";

/// One schema migration step for a domain.
pub struct Migration {
    /// Target version this migration brings the domain to. Versions are
    /// contiguous and start at 1.
    pub version: i64,
    /// Stable human-readable name (shows up in errors and reports).
    pub name: &'static str,
    /// The migration body. Runs inside the runner's IMMEDIATE transaction;
    /// it must not manage transactions itself. Bodies lifted from historical
    /// upgrade functions keep their internal idempotence guards.
    pub apply: fn(&Transaction<'_>) -> Result<(), rusqlite::Error>,
}

/// A store's schema domain: its ledger name plus the ordered migration list.
pub struct SchemaDomain {
    /// Ledger key. Kebab-case, stable forever (it is persisted in files).
    pub name: &'static str,
    /// Ordered migrations, versions contiguous from 1.
    pub migrations: &'static [Migration],
}

impl SchemaDomain {
    /// Highest version this binary knows for the domain.
    pub fn supported_version(&self) -> i64 {
        self.migrations.last().map_or(0, |m| m.version)
    }

    fn validate(&self) -> Result<(), SqliteStoreError> {
        for (idx, migration) in self.migrations.iter().enumerate() {
            let expected = idx as i64 + 1;
            if migration.version != expected {
                return Err(SqliteStoreError::InvalidMigrationList {
                    domain: self.name.to_string(),
                    detail: format!(
                        "migration at position {idx} has version {}, expected {expected} \
                         (versions must be contiguous from 1)",
                        migration.version
                    ),
                });
            }
        }
        Ok(())
    }
}

/// Outcome of [`apply_domain_migrations`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerReport {
    /// Version found before this call (0 = no ledger row).
    pub from_version: i64,
    /// Version after this call.
    pub to_version: i64,
}

impl LedgerReport {
    /// True when this call applied at least one migration.
    pub fn migrated(&self) -> bool {
        self.to_version > self.from_version
    }
}

/// Read a domain's ledger version without applying anything.
///
/// `Ok(None)` means the file has no ledger table or no row for the domain —
/// a pre-ledger file. Read-only diagnostic surfaces (doctor) use this.
pub fn domain_version(conn: &Connection, domain: &str) -> Result<Option<i64>, SqliteStoreError> {
    if !ledger_table_exists(conn)? {
        return Ok(None);
    }
    read_version(conn, domain)
}

/// Bring `domain` up to date in the file behind `conn`, per the pinned
/// protocol. Returns the version movement.
///
/// The fast path (already at the supported version) costs one read; a future
/// version is refused before any mutation.
pub fn apply_domain_migrations(
    conn: &mut Connection,
    domain: &SchemaDomain,
) -> Result<LedgerReport, SqliteStoreError> {
    domain.validate()?;
    let supported = domain.supported_version();

    // Fast path: no write transaction when the file is already current.
    if ledger_table_exists(conn)?
        && let Some(version) = read_version(conn, domain.name)?
    {
        if version == supported {
            return Ok(LedgerReport {
                from_version: version,
                to_version: version,
            });
        }
        if version > supported {
            return Err(SqliteStoreError::SchemaFromTheFuture {
                domain: domain.name.to_string(),
                found: version,
                supported,
            });
        }
    }

    // The ledger table's own DDL is the one statement allowed outside the
    // protocol transaction: it is idempotent and racing creators converge.
    conn.execute_batch(CREATE_LEDGER_SQL)?;

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    // Re-read inside the write transaction: a concurrent open may have
    // migrated between the fast path and lock acquisition.
    let current = read_version_tx(&tx, domain.name)?.unwrap_or(0);
    if current > supported {
        // Reject before any mutation; the transaction commits nothing.
        return Err(SqliteStoreError::SchemaFromTheFuture {
            domain: domain.name.to_string(),
            found: current,
            supported,
        });
    }
    if current == supported {
        return Ok(LedgerReport {
            from_version: current,
            to_version: current,
        });
    }

    for migration in domain.migrations.iter().filter(|m| m.version > current) {
        (migration.apply)(&tx).map_err(|source| SqliteStoreError::MigrationFailed {
            domain: domain.name.to_string(),
            version: migration.version,
            name: migration.name.to_string(),
            source,
        })?;
    }
    tx.execute(
        "INSERT INTO meerkat_schema (domain, version) VALUES (?1, ?2)
         ON CONFLICT(domain) DO UPDATE SET version = excluded.version",
        rusqlite::params![domain.name, supported],
    )?;
    tx.commit()?;

    Ok(LedgerReport {
        from_version: current,
        to_version: supported,
    })
}

fn ledger_table_exists(conn: &Connection) -> Result<bool, SqliteStoreError> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meerkat_schema'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    Ok(exists)
}

fn read_version(conn: &Connection, domain: &str) -> Result<Option<i64>, SqliteStoreError> {
    Ok(conn
        .query_row(
            "SELECT version FROM meerkat_schema WHERE domain = ?1",
            [domain],
            |row| row.get(0),
        )
        .optional()?)
}

fn read_version_tx(tx: &Transaction<'_>, domain: &str) -> Result<Option<i64>, SqliteStoreError> {
    Ok(tx
        .query_row(
            "SELECT version FROM meerkat_schema WHERE domain = ?1",
            [domain],
            |row| row.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{ConnectionProfile, open};

    fn create_t1(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
        tx.execute_batch("CREATE TABLE IF NOT EXISTS t1 (x INTEGER)")
    }

    fn add_column_guarded(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
        let has_column = tx
            .prepare("PRAGMA table_info(t1)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "y");
        if !has_column {
            tx.execute_batch("ALTER TABLE t1 ADD COLUMN y TEXT")?;
        }
        Ok(())
    }

    const DOMAIN_V1: SchemaDomain = SchemaDomain {
        name: "test-domain",
        migrations: &[Migration {
            version: 1,
            name: "base",
            apply: create_t1,
        }],
    };

    const DOMAIN_V2: SchemaDomain = SchemaDomain {
        name: "test-domain",
        migrations: &[
            Migration {
                version: 1,
                name: "base",
                apply: create_t1,
            },
            Migration {
                version: 2,
                name: "add-y",
                apply: add_column_guarded,
            },
        ],
    };

    fn temp_conn(dir: &tempfile::TempDir) -> Connection {
        open(&dir.path().join("db.sqlite3"), ConnectionProfile::PRIMARY).expect("open")
    }

    #[test]
    fn fresh_file_applies_all_and_stamps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        let report = apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("apply");
        assert_eq!(
            report,
            LedgerReport {
                from_version: 0,
                to_version: 2
            }
        );
        assert_eq!(domain_version(&conn, "test-domain").expect("read"), Some(2));
        conn.execute("INSERT INTO t1 (x, y) VALUES (1, 'a')", [])
            .expect("schema converged");
    }

    #[test]
    fn second_open_is_noop_fast_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("first");
        let report = apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("second");
        assert!(!report.migrated());
    }

    #[test]
    fn upgrade_applies_only_pending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        apply_domain_migrations(&mut conn, &DOMAIN_V1).expect("v1");
        let report = apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("v2");
        assert_eq!(
            report,
            LedgerReport {
                from_version: 1,
                to_version: 2
            }
        );
    }

    #[test]
    fn pre_ledger_file_with_existing_tables_converges_and_stamps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        // Simulate a legacy file: tables exist (old vintage, no y column),
        // no ledger.
        conn.execute_batch("CREATE TABLE t1 (x INTEGER)")
            .expect("legacy ddl");
        let report = apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("baseline");
        assert_eq!(
            report,
            LedgerReport {
                from_version: 0,
                to_version: 2
            }
        );
        conn.execute("INSERT INTO t1 (x, y) VALUES (1, 'a')", [])
            .expect("guarded alter converged the legacy table");
    }

    #[test]
    fn future_version_is_refused_before_any_mutation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("stamp v2");
        // An older binary knows only v1.
        let err = apply_domain_migrations(&mut conn, &DOMAIN_V1).expect_err("refuse");
        match err {
            SqliteStoreError::SchemaFromTheFuture {
                domain,
                found,
                supported,
            } => {
                assert_eq!(domain, "test-domain");
                assert_eq!(found, 2);
                assert_eq!(supported, 1);
            }
            other => panic!("wrong error: {other}"),
        }
        // Nothing moved.
        assert_eq!(domain_version(&conn, "test-domain").expect("read"), Some(2));
    }

    #[test]
    fn foreign_domain_rows_are_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("mine");
        conn.execute(
            "INSERT INTO meerkat_schema (domain, version) VALUES ('foreign-domain', 7)",
            [],
        )
        .expect("foreign row");
        apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("noop");
        let foreign: i64 = conn
            .query_row(
                "SELECT version FROM meerkat_schema WHERE domain = 'foreign-domain'",
                [],
                |r| r.get(0),
            )
            .expect("foreign row survives");
        assert_eq!(foreign, 7);
    }

    #[test]
    fn invalid_migration_list_is_refused_without_touching_the_file() {
        const BAD: SchemaDomain = SchemaDomain {
            name: "bad-domain",
            migrations: &[Migration {
                version: 3,
                name: "gap",
                apply: create_t1,
            }],
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        let err = apply_domain_migrations(&mut conn, &BAD).expect_err("refuse");
        assert!(matches!(err, SqliteStoreError::InvalidMigrationList { .. }));
        assert!(!ledger_table_exists(&conn).expect("check"));
    }

    #[test]
    fn failed_migration_rolls_back_atomically() {
        fn fail(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
            tx.execute_batch("CREATE TABLE half_done (x INTEGER)")?;
            tx.execute_batch("THIS IS NOT SQL")
        }
        const FAILING: SchemaDomain = SchemaDomain {
            name: "failing-domain",
            migrations: &[
                Migration {
                    version: 1,
                    name: "base",
                    apply: create_t1,
                },
                Migration {
                    version: 2,
                    name: "explodes",
                    apply: fail,
                },
            ],
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        let err = apply_domain_migrations(&mut conn, &FAILING).expect_err("must fail");
        assert!(matches!(
            err,
            SqliteStoreError::MigrationFailed { version: 2, .. }
        ));
        // Atomic: neither the v1 table, the half-done table, nor a ledger row
        // survives.
        assert_eq!(domain_version(&conn, "failing-domain").expect("read"), None);
        let tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('t1','half_done')",
            )
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert!(tables.is_empty(), "rollback left tables behind: {tables:?}");
    }

    #[test]
    fn concurrent_opens_race_safely() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("db.sqlite3");
        let mut handles = Vec::new();
        for _ in 0..8 {
            let path = path.clone();
            handles.push(std::thread::spawn(move || {
                let mut conn = open(&path, ConnectionProfile::PRIMARY).expect("open");
                apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("apply")
            }));
        }
        let mut migrated = 0;
        for handle in handles {
            let report = handle.join().expect("thread");
            assert_eq!(report.to_version, 2);
            if report.migrated() {
                migrated += 1;
            }
        }
        assert!(migrated >= 1, "someone must have migrated");
        let conn = open(&path, ConnectionProfile::ReadOnly).expect("reopen");
        assert_eq!(domain_version(&conn, "test-domain").expect("read"), Some(2));
    }
}
