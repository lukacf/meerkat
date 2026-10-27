use toolkit_store::{SqliteStore, StoreError};

#[test]
fn existing_open_refuses_missing_or_uninitialized_databases() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.sqlite");
    assert!(SqliteStore::open_existing(&missing).is_err());
    assert!(!missing.exists());

    let empty = directory.path().join("empty.sqlite");
    std::fs::write(&empty, []).unwrap();
    assert!(matches!(
        SqliteStore::open_existing(&empty),
        Err(StoreError::UnsupportedFormat(0))
    ));
    assert!(std::fs::read(&empty).unwrap().is_empty());
    assert!(!empty.with_extension("sqlite-wal").exists());

    let initialized = directory.path().join("initialized.sqlite");
    drop(SqliteStore::open(&initialized).unwrap());
    assert!(SqliteStore::open_existing(&initialized).is_ok());
}

#[test]
fn existing_open_refuses_future_schema_without_changing_journal() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("future.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 999).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        SqliteStore::open_existing(&path),
        Err(StoreError::UnsupportedFormat(999))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!path.with_extension("sqlite-wal").exists());
}
