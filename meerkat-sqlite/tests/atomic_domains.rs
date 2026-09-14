use meerkat_sqlite::{
    Migration, SchemaDomain, SchemaObject, SchemaObjectKind, SchemaPredecessor, SqliteStoreError,
    apply_domain_migrations, apply_domain_migrations_atomically, domain_version,
    verify_released_schema_fingerprint,
};
use rusqlite::{Connection, Transaction};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn peer_floor_refuses_before_migration_and_on_current_version_reads() -> TestResult {
    let mut conn = Connection::open_in_memory()?;
    apply_domain_migrations_atomically(&mut conn, &[&ALPHA_V1, &BETA_V1])?;
    let requirement = meerkat_sqlite::CoTenantRequirement {
        domain: "beta",
        minimum_version: 2,
    };
    assert!(matches!(
        meerkat_sqlite::apply_domain_migrations_with_requirements(
            &mut conn,
            &[&ALPHA_V2],
            &[requirement]
        ),
        Err(SqliteStoreError::CoTenantActivationRequired {
            found: 1,
            required: 2,
            ..
        })
    ));
    assert_eq!(domain_version(&conn, "alpha")?, Some(1));
    assert!(conn.prepare("SELECT revision FROM alpha").is_err());
    apply_domain_migrations(&mut conn, &ALPHA_V2)?;
    assert!(matches!(
        meerkat_sqlite::apply_domain_migrations_with_requirements(
            &mut conn,
            &[&ALPHA_V2],
            &[requirement]
        ),
        Err(SqliteStoreError::CoTenantActivationRequired {
            found: 1,
            required: 2,
            ..
        })
    ));
    assert_eq!(domain_version(&conn, "alpha")?, Some(2));
    assert_eq!(domain_version(&conn, "beta")?, Some(1));
    Ok(())
}

#[test]
fn optional_peer_floor_never_materializes_an_absent_peer() -> TestResult {
    let mut conn = Connection::open_in_memory()?;
    meerkat_sqlite::apply_domain_migrations_with_requirements(
        &mut conn,
        &[&ALPHA_V2],
        &[meerkat_sqlite::CoTenantRequirement {
            domain: "beta",
            minimum_version: 2,
        }],
    )?;
    assert_eq!(domain_version(&conn, "alpha")?, Some(2));
    assert_eq!(domain_version(&conn, "beta")?, None);
    assert!(conn.prepare("SELECT * FROM beta").is_err());
    Ok(())
}

fn alpha_v1(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch("CREATE TABLE alpha (id INTEGER PRIMARY KEY, value TEXT NOT NULL)")
}

fn beta_v1(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch("CREATE TABLE beta (id INTEGER PRIMARY KEY, value TEXT NOT NULL)")
}

fn alpha_upgrade(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch("ALTER TABLE alpha ADD COLUMN revision INTEGER NOT NULL DEFAULT 1")
}

fn beta_upgrade(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch("ALTER TABLE beta ADD COLUMN revision INTEGER NOT NULL DEFAULT 1")
}

fn beta_fail(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch("INSERT INTO table_that_does_not_exist VALUES (1)")
}

fn alpha_v2(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    alpha_v1(tx)?;
    alpha_upgrade(tx)
}

fn beta_v2(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    beta_v1(tx)?;
    beta_upgrade(tx)
}

const ALPHA_OBJECTS: &[SchemaObject] = &[SchemaObject {
    kind: SchemaObjectKind::Table,
    name: "alpha",
}];
const BETA_OBJECTS: &[SchemaObject] = &[SchemaObject {
    kind: SchemaObjectKind::Table,
    name: "beta",
}];

fn verify_alpha_v1(conn: &Connection) -> Result<(), String> {
    verify_released_schema_fingerprint(conn, &ALPHA_V2, ALPHA_OBJECTS, alpha_v1)
}

fn verify_beta_v1(conn: &Connection) -> Result<(), String> {
    verify_released_schema_fingerprint(conn, &BETA_V2, BETA_OBJECTS, beta_v1)
}

const ALPHA_V1: SchemaDomain = SchemaDomain {
    name: "alpha",
    migrations: &[Migration {
        version: 1,
        name: "initial",
        apply: alpha_v1,
    }],
    initialize_current: alpha_v1,
    allowed_existing_versions: &[1],
    released_predecessors: &[],
    owned_objects: ALPHA_OBJECTS,
    retired_objects: &[],
    bridge_recoverable_versions: &[],
};
const BETA_V1: SchemaDomain = SchemaDomain {
    name: "beta",
    migrations: &[Migration {
        version: 1,
        name: "initial",
        apply: beta_v1,
    }],
    initialize_current: beta_v1,
    allowed_existing_versions: &[1],
    released_predecessors: &[],
    owned_objects: BETA_OBJECTS,
    retired_objects: &[],
    bridge_recoverable_versions: &[],
};
const ALPHA_V2: SchemaDomain = SchemaDomain {
    migrations: &[
        Migration {
            version: 1,
            name: "initial",
            apply: alpha_v1,
        },
        Migration {
            version: 2,
            name: "revision",
            apply: alpha_upgrade,
        },
    ],
    initialize_current: alpha_v2,
    allowed_existing_versions: &[1, 2],
    released_predecessors: &[SchemaPredecessor {
        version: 1,
        verify: verify_alpha_v1,
    }],
    ..ALPHA_V1
};
const BETA_V2: SchemaDomain = SchemaDomain {
    migrations: &[
        Migration {
            version: 1,
            name: "initial",
            apply: beta_v1,
        },
        Migration {
            version: 2,
            name: "revision",
            apply: beta_upgrade,
        },
    ],
    initialize_current: beta_v2,
    allowed_existing_versions: &[1, 2],
    released_predecessors: &[SchemaPredecessor {
        version: 1,
        verify: verify_beta_v1,
    }],
    ..BETA_V1
};
const BETA_FAILING_V2: SchemaDomain = SchemaDomain {
    migrations: &[
        Migration {
            version: 1,
            name: "initial",
            apply: beta_v1,
        },
        Migration {
            version: 2,
            name: "failing-revision",
            apply: beta_fail,
        },
    ],
    ..BETA_V2
};

#[test]
fn selected_domains_initialize_and_upgrade_atomically_without_touching_foreign_rows() -> TestResult
{
    let mut conn = Connection::open_in_memory()?;
    let reports = apply_domain_migrations_atomically(&mut conn, &[&ALPHA_V1, &BETA_V1])?;
    assert_eq!(reports.len(), 2);
    assert!(
        reports
            .iter()
            .all(|report| report.from_version == 0 && report.to_version == 1)
    );
    conn.execute_batch("INSERT INTO alpha VALUES (1, 'retained'); INSERT INTO meerkat_schema VALUES ('foreign', 99)")?;
    let reports = apply_domain_migrations_atomically(&mut conn, &[&ALPHA_V2, &BETA_V2])?;
    assert!(
        reports
            .iter()
            .all(|report| report.from_version == 1 && report.to_version == 2)
    );
    assert_eq!(domain_version(&conn, "alpha")?, Some(2));
    assert_eq!(domain_version(&conn, "beta")?, Some(2));
    assert_eq!(domain_version(&conn, "foreign")?, Some(99));
    assert_eq!(
        conn.query_row("SELECT value, revision FROM alpha WHERE id=1", [], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?,
        ("retained".to_owned(), 1)
    );
    let reports = apply_domain_migrations_atomically(&mut conn, &[&ALPHA_V2, &BETA_V2])?;
    assert!(
        reports
            .iter()
            .all(|report| report.from_version == 2 && report.to_version == 2)
    );
    Ok(())
}

#[test]
fn later_migration_failure_rolls_back_earlier_ddl_and_both_ledger_stamps() -> TestResult {
    let mut conn = Connection::open_in_memory()?;
    apply_domain_migrations_atomically(&mut conn, &[&ALPHA_V1, &BETA_V1])?;
    assert!(matches!(
        apply_domain_migrations_atomically(&mut conn, &[&ALPHA_V2, &BETA_FAILING_V2]),
        Err(SqliteStoreError::MigrationFailed { .. })
    ));
    assert_eq!(domain_version(&conn, "alpha")?, Some(1));
    assert_eq!(domain_version(&conn, "beta")?, Some(1));
    assert!(conn.prepare("SELECT revision FROM alpha").is_err());
    assert!(conn.prepare("SELECT revision FROM beta").is_err());
    meerkat_sqlite::preflight_schema_eligibility(&conn, &ALPHA_V1)?;
    meerkat_sqlite::preflight_schema_eligibility(&conn, &BETA_V1)?;
    Ok(())
}

#[test]
fn future_second_domain_refuses_before_fresh_first_domain_is_initialized() -> TestResult {
    let mut conn = Connection::open_in_memory()?;
    apply_domain_migrations(&mut conn, &BETA_V2)?;
    assert!(matches!(
        apply_domain_migrations_atomically(&mut conn, &[&ALPHA_V1, &BETA_V1]),
        Err(SqliteStoreError::SchemaFromTheFuture { .. })
    ));
    assert_eq!(domain_version(&conn, "alpha")?, None);
    assert_eq!(domain_version(&conn, "beta")?, Some(2));
    assert!(conn.prepare("SELECT * FROM alpha").is_err());
    Ok(())
}

#[test]
fn repeated_domain_and_shared_object_ownership_are_rejected() -> TestResult {
    let mut conn = Connection::open_in_memory()?;
    assert!(matches!(
        apply_domain_migrations_atomically(&mut conn, &[&ALPHA_V1, &ALPHA_V1]),
        Err(SqliteStoreError::InvalidMigrationList { .. })
    ));
    let collision = SchemaDomain {
        name: "another-owner",
        ..ALPHA_V1
    };
    assert!(matches!(
        apply_domain_migrations_atomically(&mut conn, &[&ALPHA_V1, &collision]),
        Err(SqliteStoreError::InvalidMigrationList { .. })
    ));
    assert_eq!(domain_version(&conn, "alpha")?, None);
    Ok(())
}

#[test]
fn file_activation_honors_old_operation_guards_and_existing_maintenance_custody() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("nested/paired.sqlite3");
    let timeout = std::time::Duration::ZERO;
    meerkat_sqlite::activate_file_domains(&path, &[&ALPHA_V1, &BETA_V1], timeout)?;
    let old_operation = meerkat_sqlite::OperationGuard::for_database(&path)?;
    assert!(matches!(
        meerkat_sqlite::activate_file_domains(&path, &[&ALPHA_V2, &BETA_V2], timeout),
        Err(SqliteStoreError::MaintenanceFenceHeld { .. })
    ));
    let conn = Connection::open(&path)?;
    assert_eq!(domain_version(&conn, "alpha")?, Some(1));
    assert_eq!(domain_version(&conn, "beta")?, Some(1));
    drop((conn, old_operation));
    let _fence = meerkat_sqlite::ExclusiveFence::acquire(&path, timeout)?;
    meerkat_sqlite::activate_file_domains(&path, &[&ALPHA_V2, &BETA_V2], timeout)?;
    let conn = Connection::open(&path)?;
    assert_eq!(domain_version(&conn, "alpha")?, Some(2));
    assert_eq!(domain_version(&conn, "beta")?, Some(2));
    Ok(())
}

#[test]
fn current_file_activation_does_not_request_a_writer_lock() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("paired.sqlite3");
    let timeout = std::time::Duration::ZERO;
    meerkat_sqlite::activate_file_domains(&path, &[&ALPHA_V1, &BETA_V1], timeout)?;
    let mut writer = Connection::open(&path)?;
    let write = writer.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    write.execute("INSERT INTO alpha VALUES (1, 'in-flight')", [])?;
    meerkat_sqlite::activate_file_domains(&path, &[&ALPHA_V1, &BETA_V1], timeout)?;
    write.rollback()?;
    Ok(())
}
