#![cfg(all(not(target_arch = "wasm32"), feature = "sqlite-store"))]

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use meerkat_runtime::store::{SqliteRuntimeStore, sqlite::RUNTIME_STORE_DOMAIN};
use meerkat_sqlite::{Connection, ConnectionProfile};
use meerkat_store::{
    SqliteSessionStore,
    sqlite_store::{SESSION_STORE_DOMAIN, ensure_schema},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn exact_released_0810_corpus_imports_and_fully_loads_through_joint_owner() -> TestResult {
    use meerkat_core::{SessionHead, SessionId};
    use meerkat_store::{IncrementalSessionStore, SessionStore};
    let workspace_root = match std::env::var_os("MEERKAT_WORKSPACE_ROOT") {
        Some(root) => std::path::PathBuf::from(root),
        None => Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or("runtime manifest has no workspace parent")?
            .to_path_buf(),
    };
    let source = workspace_root.join(
        "meerkat-runtime/tests/fixtures/v0_8_10_released_realm/corpus/realm/sessions.sqlite3",
    );
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("sessions.sqlite3");
    std::fs::copy(source, &path)?;
    let raw = meerkat_sqlite::open(&path, ConnectionProfile::ReadOnly)?;
    let raw_id: String = raw.query_row(
        "SELECT session_id FROM session_heads ORDER BY session_id LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    drop(raw);
    let id = SessionId::parse(&raw_id)?;
    SqliteRuntimeStore::new_head_canonical(&path)?;
    let store = SqliteSessionStore::open(&path)?;
    let session = store
        .load(&id)
        .await?
        .ok_or("released corpus session missing")?;
    assert_eq!(session.version(), meerkat_core::SESSION_VERSION);
    assert_eq!(session.messages().len(), 5);
    assert!(
        !session
            .metadata()
            .contains_key("session_checkpoint_stamp_v1"),
        "the frozen importer must retire released checkpoint metadata"
    );
    assert!(
        !session
            .metadata()
            .contains_key("session_system_context_state"),
        "the frozen importer must adopt and retire released System context"
    );
    session.validated_transcript_history_state()?;
    let head = IncrementalSessionStore::load_head(&store, &id)
        .await?
        .ok_or("released head missing")?;
    assert_eq!(head.version, meerkat_core::SESSION_VERSION);
    let migrated = meerkat_sqlite::open(&path, ConnectionProfile::ReadOnly)?;
    let (row_version, bytes, stored_token): (i64, Vec<u8>, String) = migrated.query_row(
        "SELECT version, head_json, cas_token FROM session_heads WHERE session_id = ?1",
        [id.to_string()],
        |row| {
            Ok((
                row.get(0)?,
                row.get::<_, meerkat_sqlite::JsonColumnBytes>(1)?
                    .into_bytes(),
                row.get(2)?,
            ))
        },
    )?;
    let embedded_head: SessionHead = serde_json::from_slice(&bytes)?;
    assert_eq!(row_version, i64::from(meerkat_core::SESSION_VERSION));
    assert_eq!(embedded_head.version, meerkat_core::SESSION_VERSION);
    assert_eq!(stored_token, meerkat_core::session_head_cas_token(&head)?);
    assert_eq!(
        meerkat_sqlite::domain_version(&migrated, "session-store")?,
        Some(5)
    );
    assert_eq!(
        meerkat_sqlite::domain_version(&migrated, "runtime-store")?,
        Some(4)
    );
    Ok(())
}

fn released_domains(path: &Path, session: bool, runtime: bool) -> TestResult {
    let mut conn = meerkat_sqlite::open(path, ConnectionProfile::PRIMARY)?;
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL)",
    )?;
    for (domain, version, enabled) in [
        (&SESSION_STORE_DOMAIN, 4, session),
        (&RUNTIME_STORE_DOMAIN, 3, runtime),
    ] {
        if enabled {
            for migration in domain.migrations.iter().take(version) {
                (migration.apply)(&tx)?;
            }
            tx.execute(
                "INSERT INTO meerkat_schema VALUES (?1, ?2)",
                (domain.name, version),
            )?;
        }
    }
    tx.commit()?;
    conn.pragma_update(None, "journal_mode", "DELETE")?;
    if session {
        meerkat_sqlite::preflight_schema_eligibility(&conn, &SESSION_STORE_DOMAIN)?;
    }
    if runtime {
        meerkat_sqlite::preflight_schema_eligibility(&conn, &RUNTIME_STORE_DOMAIN)?;
    }
    Ok(())
}

fn catalog(conn: &Connection) -> Result<Vec<(String, String, Option<String>)>, rusqlite::Error> {
    conn.prepare("SELECT type, name, sql FROM sqlite_schema ORDER BY type, name")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect()
}

fn assert_single_domain_refuses_without_mutation(operation: &str) -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join(format!("{operation}.sqlite3"));
    released_domains(&path, true, true)?;
    let before = catalog(&Connection::open(&path)?)?;
    let refused = match operation {
        "session-open" => matches!(
            SqliteSessionStore::open(&path),
            Err(meerkat_store::StoreError::CoTenantActivationRequired {
                found: 3,
                required: 4,
                ..
            })
        ),
        "ensure-schema" => matches!(
            ensure_schema(&mut Connection::open(&path)?),
            Err(meerkat_store::StoreError::CoTenantActivationRequired {
                found: 3,
                required: 4,
                ..
            })
        ),
        "whole-blob-open" => matches!(
            SqliteRuntimeStore::new_whole_blob(&path),
            Err(
                meerkat_runtime::store::RuntimeStoreError::CoTenantActivationRequired {
                    found: 4,
                    required: 5,
                    ..
                }
            )
        ),
        _ => return Err("unexpected test operation".into()),
    };
    assert!(refused, "{operation} must request joint-owner activation");
    let conn = Connection::open(&path)?;
    assert!(
        catalog(&conn)? == before,
        "{operation} changed a schema before refusing"
    );
    assert_eq!(
        meerkat_sqlite::domain_version(&conn, "session-store")?,
        Some(4)
    );
    assert_eq!(
        meerkat_sqlite::domain_version(&conn, "runtime-store")?,
        Some(3)
    );
    Ok(())
}

#[test]
fn session_open_cannot_partially_activate_existing_cotenants() -> TestResult {
    assert_single_domain_refuses_without_mutation("session-open")
}

#[test]
fn ensure_schema_cannot_partially_activate_existing_cotenants() -> TestResult {
    assert_single_domain_refuses_without_mutation("ensure-schema")
}

#[test]
fn whole_blob_open_cannot_partially_activate_existing_cotenants() -> TestResult {
    assert_single_domain_refuses_without_mutation("whole-blob-open")
}

#[test]
fn standalone_domains_still_upgrade_without_creating_foreign_domains() -> TestResult {
    let directory = tempfile::tempdir()?;
    let session_path = directory.path().join("session.sqlite3");
    released_domains(&session_path, true, false)?;
    SqliteSessionStore::open(&session_path)?;
    let conn = Connection::open(&session_path)?;
    assert_eq!(
        meerkat_sqlite::domain_version(&conn, "session-store")?,
        Some(5)
    );
    assert_eq!(
        meerkat_sqlite::domain_version(&conn, "runtime-store")?,
        None
    );
    let runtime_path = directory.path().join("runtime.sqlite3");
    released_domains(&runtime_path, false, true)?;
    SqliteRuntimeStore::new_whole_blob(&runtime_path)?;
    let conn = Connection::open(&runtime_path)?;
    assert_eq!(
        meerkat_sqlite::domain_version(&conn, "session-store")?,
        None
    );
    assert_eq!(
        meerkat_sqlite::domain_version(&conn, "runtime-store")?,
        Some(4)
    );
    Ok(())
}

#[test]
fn explicit_joint_owner_upgrades_and_current_cotenants_can_reopen() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("paired.sqlite3");
    released_domains(&path, true, true)?;
    SqliteRuntimeStore::new_head_canonical(&path)?;
    SqliteSessionStore::open(&path)?;
    SqliteRuntimeStore::new_head_canonical(&path)?;
    let conn = Connection::open(&path)?;
    assert_eq!(
        meerkat_sqlite::domain_version(&conn, "session-store")?,
        Some(5)
    );
    assert_eq!(
        meerkat_sqlite::domain_version(&conn, "runtime-store")?,
        Some(4)
    );
    Ok(())
}

#[test]
fn current_domain_reopen_waits_for_normal_read_contention() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("runtime.sqlite3");
    SqliteRuntimeStore::new_whole_blob(&path)?;
    let mut blocker = Connection::open(&path)?;
    blocker.pragma_update(None, "journal_mode", "DELETE")?;
    let exclusive = blocker.transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)?;
    let (started_tx, started_rx) = mpsc::channel();
    let (completed_tx, completed_rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let start = started_tx.send(());
        let opened = SqliteRuntimeStore::new_whole_blob(path).map(|_| ());
        let complete = completed_tx.send(opened);
        (start, complete)
    });
    started_rx.recv_timeout(Duration::from_secs(5))?;
    let while_locked = completed_rx.recv_timeout(Duration::from_millis(200));
    exclusive.rollback()?;
    assert!(
        matches!(while_locked, Err(mpsc::RecvTimeoutError::Timeout)),
        "ordinary current-read path failed immediately instead of honoring its busy timeout: {while_locked:?}"
    );
    completed_rx.recv_timeout(Duration::from_secs(5))??;
    let (start, complete) = reader.join().map_err(|_| "reader thread panicked")?;
    start?;
    complete?;
    Ok(())
}
