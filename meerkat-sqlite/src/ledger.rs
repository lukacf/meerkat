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
//!    same transaction — custody is verified with a runner-owned savepoint
//!    around each body, so a body that COMMITs or ROLLBACKs underneath the
//!    runner is refused ([`SqliteStoreError::MigrationBrokeTransaction`])
//!    even when it re-BEGINs a fresh transaction afterwards.
//!
//! A table merely *named* `meerkat_schema` is not trusted: before any read
//! the pinned column shape is validated against `main`'s catalog, versions
//! must be positive, and at most one row may exist per domain
//! ([`SqliteStoreError::LedgerMalformed`] otherwise). All ledger SQL is
//! `main.`-qualified, so a TEMP table shadowing the name can neither satisfy
//! nor bypass the ledger.
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

const CREATE_LEDGER_SQL: &str = "CREATE TABLE IF NOT EXISTS main.meerkat_schema (
    domain TEXT PRIMARY KEY,
    version INTEGER NOT NULL
)";

/// Custody marker established inside the runner's transaction immediately
/// before each migration body. A savepoint is discarded when its enclosing
/// transaction ends — by COMMIT or ROLLBACK alike — so it survives the body
/// exactly when the runner's transaction does.
const CUSTODY_SAVEPOINT_SQL: &str = "SAVEPOINT meerkat_migration_custody";
const CUSTODY_RELEASE_SQL: &str = "RELEASE SAVEPOINT meerkat_migration_custody";

/// One schema migration step for a domain.
#[derive(Debug)]
pub struct Migration {
    /// Target version this migration brings the domain to. Versions are
    /// contiguous and start at 1.
    pub version: i64,
    /// Stable human-readable name (shows up in errors and reports).
    pub name: &'static str,
    /// The migration body. Runs inside the runner's IMMEDIATE transaction;
    /// it must not end that transaction (nested savepoints of its own are
    /// fine). Bodies lifted from historical upgrade functions keep their
    /// internal idempotence guards.
    pub apply: fn(&Transaction<'_>) -> Result<(), rusqlite::Error>,
}

/// A store's schema domain: its ledger name plus the ordered migration list.
#[derive(Debug)]
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
/// A ledger table that fails the pinned-shape or version validation yields
/// [`SqliteStoreError::LedgerMalformed`], never a healed reading.
pub fn domain_version(conn: &Connection, domain: &str) -> Result<Option<i64>, SqliteStoreError> {
    if !ledger_table_exists(conn)? {
        return Ok(None);
    }
    validate_ledger_shape(conn)?;
    read_version(conn, domain)
}

/// Refuse when the file's ledger already records a version of `domain`
/// newer than this binary supports, without mutating anything.
///
/// This is the [`crate::profile::OpenOptions::schema_preflight`] hook: the
/// Primary profile runs it before its mutating pragmas so an old binary
/// leaves a future database's logical content unmodified. Reading the
/// ledger of a WAL-mode file over a read-write connection may still touch
/// its `-wal`/`-shm` sidecars
/// ([`crate::profile::WriteContact::ReadOnlyWalSidecars`]); the main
/// database file itself is not written. Files without a ledger (fresh or
/// pre-ledger) pass; the pinned in-transaction re-check in
/// [`apply_domain_migrations`] remains the migration-time authority.
pub fn refuse_future_schema(
    conn: &Connection,
    domain: &SchemaDomain,
) -> Result<(), SqliteStoreError> {
    if let Some(found) = domain_version(conn, domain.name)? {
        let supported = domain.supported_version();
        if found > supported {
            return Err(SqliteStoreError::SchemaFromTheFuture {
                domain: domain.name.to_string(),
                found,
                supported,
            });
        }
    }
    Ok(())
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
    if ledger_table_exists(conn)? {
        validate_ledger_shape(conn)?;
        if let Some(version) = read_version(conn, domain.name)? {
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
    }

    // The ledger table's own DDL is the one statement allowed outside the
    // protocol transaction: it is idempotent and racing creators converge.
    conn.execute_batch(CREATE_LEDGER_SQL)?;

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    // Re-validate inside the write transaction: the CREATE above is
    // IF NOT EXISTS, so a pre-existing (possibly foreign) table survives it
    // and must not be stamped into.
    validate_ledger_shape(&tx)?;
    // Re-read inside the write transaction: a concurrent open may have
    // migrated between the fast path and lock acquisition.
    let current = read_version(&tx, domain.name)?.unwrap_or(0);
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
        tx.execute_batch(CUSTODY_SAVEPOINT_SQL)?;
        (migration.apply)(&tx).map_err(|source| SqliteStoreError::MigrationFailed {
            domain: domain.name.to_string(),
            version: migration.version,
            name: migration.name.to_string(),
            source,
        })?;
        // The `&Transaction` handed to the body cannot type-prevent COMMIT /
        // ROLLBACK statements, so custody is verified instead. Autocommit
        // going true is the cheap first line, but it misses a body that
        // ended the transaction and then re-BEGAN one; the savepoint is the
        // authority: RELEASE fails exactly when the savepoint no longer
        // exists, i.e. the body ended the runner's transaction (COMMIT and
        // ROLLBACK both discard it), whether or not it opened a new one.
        // Stamping the ledger inside such a foreign transaction would commit
        // separately from — or after rollback of — the schema work.
        if tx.is_autocommit() || tx.execute_batch(CUSTODY_RELEASE_SQL).is_err() {
            return Err(SqliteStoreError::MigrationBrokeTransaction {
                domain: domain.name.to_string(),
                version: migration.version,
                name: migration.name.to_string(),
            });
        }
    }
    tx.execute(
        "INSERT INTO main.meerkat_schema (domain, version) VALUES (?1, ?2)
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
            "SELECT 1 FROM main.sqlite_master WHERE type = 'table' AND name = 'meerkat_schema'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    Ok(exists)
}

fn malformed(detail: String) -> SqliteStoreError {
    SqliteStoreError::LedgerMalformed { detail }
}

/// Validate the pinned ledger shape against `main`'s real catalog.
///
/// `domain` must be `TEXT` and the *sole* primary-key column (a composite
/// key would permit multiple rows per domain); `version` must be
/// `INTEGER NOT NULL`. Columns beyond the pinned pair are tolerated as long
/// as they carry no primary-key position, so a future ledger protocol can
/// extend the table compatibly. The schema-qualified `pragma_table_info`
/// resolves in `main`, so a TEMP shadow cannot satisfy this check.
fn validate_ledger_shape(conn: &Connection) -> Result<(), SqliteStoreError> {
    let mut stmt = conn.prepare(
        "SELECT name, type, \"notnull\", pk FROM pragma_table_info('meerkat_schema', 'main')",
    )?;
    let mut rows = stmt.query([])?;
    let mut domain_ok = false;
    let mut version_ok = false;
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        let decl_type: String = row.get(1)?;
        let notnull: bool = row.get(2)?;
        let pk: i64 = row.get(3)?;
        match name.as_str() {
            "domain" => {
                if !decl_type.eq_ignore_ascii_case("TEXT") || pk != 1 {
                    return Err(malformed(format!(
                        "column `domain` must be `TEXT PRIMARY KEY`, found type `{decl_type}` \
                         with pk position {pk}"
                    )));
                }
                domain_ok = true;
            }
            "version" => {
                if !decl_type.eq_ignore_ascii_case("INTEGER") || !notnull || pk != 0 {
                    return Err(malformed(format!(
                        "column `version` must be non-key `INTEGER NOT NULL`, found type \
                         `{decl_type}` notnull={notnull} pk position {pk}"
                    )));
                }
                version_ok = true;
            }
            other => {
                if pk != 0 {
                    return Err(malformed(format!(
                        "unexpected primary-key column `{other}`"
                    )));
                }
            }
        }
    }
    if !domain_ok || !version_ok {
        return Err(malformed(
            "table lacks the pinned `domain`/`version` columns".to_string(),
        ));
    }
    Ok(())
}

/// Read one domain's version, refusing corrupt ledger state typed: more than
/// one row per domain (impossible under the validated single-column primary
/// key; kept as defense in depth) and non-positive versions (0 is the
/// implicit "no row" reading and negatives are meaningless — a stored
/// non-positive version is damage to refuse, not an old schema to re-migrate
/// over).
fn read_version(conn: &Connection, domain: &str) -> Result<Option<i64>, SqliteStoreError> {
    let mut stmt = conn.prepare("SELECT version FROM main.meerkat_schema WHERE domain = ?1")?;
    let mut rows = stmt.query([domain])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let version: i64 = row.get(0)?;
    if rows.next()?.is_some() {
        return Err(malformed(format!(
            "multiple ledger rows for domain `{domain}`"
        )));
    }
    if version <= 0 {
        return Err(malformed(format!(
            "domain `{domain}` records non-positive version {version}"
        )));
    }
    Ok(Some(version))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
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
    fn malformed_ledger_shape_is_refused_not_healed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        // A foreign table wearing the ledger's name.
        conn.execute_batch("CREATE TABLE meerkat_schema (x INTEGER)")
            .expect("foreign ddl");
        let err = domain_version(&conn, "test-domain").expect_err("refuse read");
        assert!(matches!(err, SqliteStoreError::LedgerMalformed { .. }));
        let err = apply_domain_migrations(&mut conn, &DOMAIN_V1).expect_err("refuse migrate");
        assert!(matches!(err, SqliteStoreError::LedgerMalformed { .. }));
        // Refused, not healed: the foreign table is untouched and unstamped.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM meerkat_schema", [], |r| r.get(0))
            .expect("foreign table survives");
        assert_eq!(count, 0);
    }

    #[test]
    fn non_positive_versions_are_refused_not_healed() {
        for bad_version in [0i64, -3] {
            let dir = tempfile::tempdir().expect("tempdir");
            let mut conn = temp_conn(&dir);
            conn.execute_batch(
                "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL)",
            )
            .expect("ledger ddl");
            conn.execute(
                "INSERT INTO meerkat_schema (domain, version) VALUES ('test-domain', ?1)",
                [bad_version],
            )
            .expect("seed bad version");
            let err = domain_version(&conn, "test-domain").expect_err("refuse read");
            assert!(matches!(err, SqliteStoreError::LedgerMalformed { .. }));
            let err = apply_domain_migrations(&mut conn, &DOMAIN_V1).expect_err("refuse migrate");
            assert!(matches!(err, SqliteStoreError::LedgerMalformed { .. }));
            // The bad row must survive untouched for forensics.
            let stored: i64 = conn
                .query_row(
                    "SELECT version FROM meerkat_schema WHERE domain = 'test-domain'",
                    [],
                    |r| r.get(0),
                )
                .expect("row survives");
            assert_eq!(stored, bad_version);
        }
    }

    #[test]
    fn duplicate_domain_rows_are_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = temp_conn(&dir);
        // No primary key: shape validation would already refuse this table;
        // the row-cardinality guard is exercised directly as defense in
        // depth.
        conn.execute_batch(
            "CREATE TABLE meerkat_schema (domain TEXT, version INTEGER NOT NULL);
             INSERT INTO meerkat_schema VALUES ('dup-domain', 1);
             INSERT INTO meerkat_schema VALUES ('dup-domain', 2);",
        )
        .expect("seed duplicates");
        let err = read_version(&conn, "dup-domain").expect_err("refuse duplicates");
        match err {
            SqliteStoreError::LedgerMalformed { detail } => {
                assert!(detail.contains("multiple ledger rows"), "{detail}");
            }
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn temp_shadowing_cannot_hijack_the_ledger() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        apply_domain_migrations(&mut conn, &DOMAIN_V1).expect("stamp v1");
        // A TEMP shadow claiming a future version: unqualified reads would
        // see 999 and refuse; the main-qualified ledger keeps reading truth.
        conn.execute_batch(
            "CREATE TEMP TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL);
             INSERT INTO temp.meerkat_schema VALUES ('test-domain', 999);",
        )
        .expect("temp shadow");
        assert_eq!(domain_version(&conn, "test-domain").expect("read"), Some(1));
        let report = apply_domain_migrations(&mut conn, &DOMAIN_V1).expect("noop against main");
        assert!(!report.migrated());
    }

    #[test]
    fn migration_that_ends_the_transaction_is_refused_unstamped() {
        fn commits_underneath(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
            tx.execute_batch("CREATE TABLE escaped_commit (x INTEGER); COMMIT")
        }
        fn rolls_back_underneath(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
            tx.execute_batch("ROLLBACK")
        }
        // The re-BEGIN variants leave autocommit false at the custody check:
        // only the savepoint detects that the runner's transaction is gone
        // and the ledger stamp would land in a foreign one.
        fn commits_then_begins(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
            tx.execute_batch("CREATE TABLE escaped_commit_begin (x INTEGER); COMMIT; BEGIN")
        }
        fn rolls_back_then_begins(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
            tx.execute_batch("ROLLBACK; BEGIN")
        }
        const COMMITS: SchemaDomain = SchemaDomain {
            name: "custody-commit",
            migrations: &[Migration {
                version: 1,
                name: "commits-underneath",
                apply: commits_underneath,
            }],
        };
        const ROLLS_BACK: SchemaDomain = SchemaDomain {
            name: "custody-rollback",
            migrations: &[Migration {
                version: 1,
                name: "rolls-back-underneath",
                apply: rolls_back_underneath,
            }],
        };
        const COMMITS_THEN_BEGINS: SchemaDomain = SchemaDomain {
            name: "custody-commit-begin",
            migrations: &[Migration {
                version: 1,
                name: "commits-then-begins",
                apply: commits_then_begins,
            }],
        };
        const ROLLS_BACK_THEN_BEGINS: SchemaDomain = SchemaDomain {
            name: "custody-rollback-begin",
            migrations: &[Migration {
                version: 1,
                name: "rolls-back-then-begins",
                apply: rolls_back_then_begins,
            }],
        };
        for (domain, expected_name) in [
            (&COMMITS, "commits-underneath"),
            (&ROLLS_BACK, "rolls-back-underneath"),
            (&COMMITS_THEN_BEGINS, "commits-then-begins"),
            (&ROLLS_BACK_THEN_BEGINS, "rolls-back-then-begins"),
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let mut conn = temp_conn(&dir);
            let err = apply_domain_migrations(&mut conn, domain).expect_err("custody violation");
            match err {
                SqliteStoreError::MigrationBrokeTransaction {
                    domain: err_domain,
                    version,
                    name,
                } => {
                    assert_eq!(err_domain, domain.name);
                    assert_eq!(version, 1);
                    assert_eq!(name, expected_name);
                }
                other => panic!("wrong error: {other}"),
            }
            // The stamp never landed: custody broke before the ledger write.
            assert_eq!(domain_version(&conn, domain.name).expect("read"), None);
        }
    }

    #[test]
    fn migration_using_its_own_savepoints_keeps_custody() {
        // A body may nest its own savepoints; custody only trips when the
        // runner's enclosing transaction (and with it the custody savepoint)
        // is gone.
        fn nests_savepoints(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
            tx.execute_batch(
                "SAVEPOINT body_sp;
                 CREATE TABLE sp_t (x INTEGER);
                 RELEASE SAVEPOINT body_sp",
            )
        }
        const NESTED: SchemaDomain = SchemaDomain {
            name: "custody-nested-savepoint",
            migrations: &[Migration {
                version: 1,
                name: "nests-savepoints",
                apply: nests_savepoints,
            }],
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        let report = apply_domain_migrations(&mut conn, &NESTED).expect("apply");
        assert_eq!(report.to_version, 1);
        assert_eq!(domain_version(&conn, NESTED.name).expect("read"), Some(1));
    }

    #[test]
    fn refuse_future_schema_passes_fresh_and_current_refuses_future() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        refuse_future_schema(&conn, &DOMAIN_V1).expect("no ledger yet");
        apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("stamp v2");
        refuse_future_schema(&conn, &DOMAIN_V2).expect("current");
        let err = refuse_future_schema(&conn, &DOMAIN_V1).expect_err("future for old binary");
        assert!(matches!(
            err,
            SqliteStoreError::SchemaFromTheFuture {
                found: 2,
                supported: 1,
                ..
            }
        ));
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
