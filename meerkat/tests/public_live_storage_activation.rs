#![cfg(all(not(target_arch = "wasm32"), feature = "session-store"))]

use meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN;
use meerkat_store::sqlite_store::SESSION_STORE_DOMAIN;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn normal_sqlite_realm_installs_both_live_compatibility_barriers() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (_, bundle) = meerkat::open_realm_persistence_in(
        directory.path(),
        "live-schema",
        Some(meerkat_store::RealmBackend::Sqlite),
        None,
    )
    .await?;
    let path = meerkat_store::realm_paths_in(directory.path(), "live-schema").sessions_sqlite_path;
    let conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::ReadOnly)?;
    assert_eq!(
        meerkat_sqlite::domain_version(&conn, "runtime-store")?,
        Some(4)
    );
    assert_eq!(
        meerkat_sqlite::domain_version(&conn, "session-store")?,
        Some(5)
    );
    meerkat_store::sqlite_store::verify_runtime_component_compatibility(&conn)?;
    for table in [
        "runtime_live_heads",
        "runtime_live_events",
        "runtime_live_sources",
        "runtime_live_attempts",
    ] {
        assert!(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='table' AND name=?1)",
            [table],
            |row| row.get::<_, bool>(0)
        )?);
    }
    drop(conn);
    drop(bundle);
    let (_, reopened) = meerkat::open_realm_persistence_in(
        directory.path(),
        "live-schema",
        Some(meerkat_store::RealmBackend::Sqlite),
        None,
    )
    .await?;
    drop(reopened);
    Ok(())
}

#[test]
fn released_session_and_runtime_readers_refuse_new_pair_before_wal_conversion() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("paired.sqlite3");
    let mut conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)?;
    meerkat_sqlite::apply_domain_migrations_atomically(
        &mut conn,
        &[&SESSION_STORE_DOMAIN, &RUNTIME_STORE_DOMAIN],
    )?;
    conn.pragma_update(None, "journal_mode", "DELETE")?;
    drop(conn);
    let before = std::fs::read(&path)?;
    static OLD_RUNTIME: std::sync::LazyLock<meerkat_sqlite::SchemaDomain> =
        std::sync::LazyLock::new(|| meerkat_sqlite::SchemaDomain {
            migrations: &RUNTIME_STORE_DOMAIN.migrations[..3],
            allowed_existing_versions: &[1, 2, 3],
            released_predecessors: &RUNTIME_STORE_DOMAIN.released_predecessors[..2],
            ..RUNTIME_STORE_DOMAIN
        });
    static OLD_SESSION: std::sync::LazyLock<meerkat_sqlite::SchemaDomain> =
        std::sync::LazyLock::new(|| meerkat_sqlite::SchemaDomain {
            migrations: &SESSION_STORE_DOMAIN.migrations[..4],
            allowed_existing_versions: &[1, 2, 3, 4],
            released_predecessors: &SESSION_STORE_DOMAIN.released_predecessors[..3],
            ..SESSION_STORE_DOMAIN
        });
    static RUNTIME_PREFLIGHT: std::sync::LazyLock<[&meerkat_sqlite::SchemaDomain; 1]> =
        std::sync::LazyLock::new(|| [&OLD_RUNTIME]);
    static SESSION_PREFLIGHT: std::sync::LazyLock<[&meerkat_sqlite::SchemaDomain; 1]> =
        std::sync::LazyLock::new(|| [&OLD_SESSION]);
    for domains in [&*SESSION_PREFLIGHT, &*RUNTIME_PREFLIGHT] {
        let result = meerkat_sqlite::open_with(
            &path,
            meerkat_sqlite::ConnectionProfile::PRIMARY,
            meerkat_sqlite::OpenOptions {
                schema_preflight: domains,
                ..Default::default()
            },
        );
        assert!(matches!(
            result,
            Err(meerkat_sqlite::SqliteStoreError::SchemaFromTheFuture { .. })
        ));
        assert_eq!(std::fs::read(&path)?, before);
    }
    Ok(())
}

#[test]
fn whole_blob_runtime_does_not_materialize_a_second_session_store() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("runtime.sqlite3");
    let store = meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(&path)?;
    let conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::ReadOnly)?;
    assert_eq!(
        meerkat_sqlite::domain_version(&conn, "runtime-store")?,
        Some(4)
    );
    assert_eq!(
        meerkat_sqlite::domain_version(&conn, "session-store")?,
        None
    );
    assert_eq!(store.path(), path);
    Ok(())
}
