use meerkat_core::{AgentSessionStore, Session};
use meerkat_store::{JsonlStore, MemoryStore, SessionStore, StoreAdapter};
use std::sync::Arc;

#[tokio::test]
async fn test_session_store_adapter_error_mapping() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(MemoryStore::new());
    let adapter = StoreAdapter::new(store);

    // Test successful load of non-existent session (should return None, not error)
    let loaded = adapter.load("550e8400-e29b-41d4-a716-446655440000").await?;
    assert!(loaded.is_none());

    // Test invalid session ID (should return error)
    let result = adapter.load("not-a-uuid").await;
    match result {
        Err(e) => {
            let msg = format!("{e}");
            assert!(msg.contains("Invalid session ID"));
        }
        Ok(_) => return Err("expected error for invalid session id".into()),
    }

    Ok(())
}

/// Regression: Session index reconciliation when sidecar count > index count
///
/// Previously: If crash happened after writing .meta sidecar but before index insert,
/// list() would miss the session because it only queried the index.
/// Fix: Reconcile index from sidecars when sidecar_count > index_count.
#[tokio::test]
async fn test_regression_session_index_reconciles_missing_entries()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = JsonlStore::new(dir.path().to_path_buf());
    store.init().await?;

    // Create and save a session
    let session = Session::new();
    let session_id = session.id().clone();
    store.save(&session).await?;

    // Verify session is listed
    let sessions = store.list(meerkat_store::SessionFilter::default()).await?;
    assert_eq!(sessions.len(), 1, "session should be listed after save");

    // Delete the index file to simulate corruption/partial write
    let index_path = dir.path().join("session_index.redb");
    if index_path.exists() {
        tokio::fs::remove_file(&index_path).await?;
    }

    // Create a fresh store (simulating restart)
    let store2 = JsonlStore::new(dir.path().to_path_buf());

    // List should still find the session (due to reconciliation from sidecars)
    let sessions = store2.list(meerkat_store::SessionFilter::default()).await?;
    assert_eq!(
        sessions.len(),
        1,
        "session should be recovered from sidecar after index deletion"
    );
    assert_eq!(sessions[0].id, session_id);

    Ok(())
}
