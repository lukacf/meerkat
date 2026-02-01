use meerkat_core::{AgentSessionStore, Session, SessionMeta};
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

/// Regression: Session index removes stale updated_at entries during rebuild.
///
/// Previously: insert_many didn't remove old updated_at entries, causing duplicates
/// in the by_updated table if a session's timestamp changed between writes.
/// Fix: Check for existing entry and remove stale updated_at key before inserting.
#[tokio::test]
async fn test_regression_session_index_removes_stale_updated_at()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = JsonlStore::new(dir.path().to_path_buf());
    store.init().await?;

    // Create and save a session
    let mut session = Session::new();
    let session_id = session.id().clone();
    store.save(&session).await?;

    // Get initial updated_at
    let sessions = store.list(meerkat_store::SessionFilter::default()).await?;
    assert_eq!(sessions.len(), 1);
    let first_updated = sessions[0].updated_at;

    // Wait a moment and update session
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    session.touch();
    store.save(&session).await?;

    // Verify updated_at changed and only one entry exists
    let sessions = store.list(meerkat_store::SessionFilter::default()).await?;
    assert_eq!(sessions.len(), 1, "should still have exactly one session");
    assert!(
        sessions[0].updated_at > first_updated,
        "updated_at should have changed"
    );

    // Delete index and re-reconcile (simulating crash recovery)
    let index_path = dir.path().join("session_index.redb");
    if index_path.exists() {
        tokio::fs::remove_file(&index_path).await?;
    }

    let store2 = JsonlStore::new(dir.path().to_path_buf());
    let sessions = store2.list(meerkat_store::SessionFilter::default()).await?;

    // Should have exactly one session, not duplicates
    assert_eq!(
        sessions.len(),
        1,
        "reconciliation should produce exactly one entry, not duplicates"
    );
    assert_eq!(sessions[0].id, session_id);

    Ok(())
}

/// Regression: Session index reconciles even when counts match.
///
/// Previously: Reconciliation only ran when sidecar_count > index_count, so
/// a crash between updating sidecar and index left stale metadata.
/// Fix: Always reconcile when sidecars exist to handle updated timestamps.
#[tokio::test]
async fn test_regression_session_index_reconciles_when_counts_match()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = JsonlStore::new(dir.path().to_path_buf());
    store.init().await?;

    // Create and save a session
    let mut session = Session::new();
    let session_id = session.id().clone();
    store.save(&session).await?;

    // Get initial state
    let sessions = store.list(meerkat_store::SessionFilter::default()).await?;
    assert_eq!(sessions.len(), 1);
    let first_updated = sessions[0].updated_at;

    // Update session metadata (sidecar will be rewritten)
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    session.touch();
    store.save(&session).await?;

    // Verify update
    let sessions = store.list(meerkat_store::SessionFilter::default()).await?;
    let second_updated = sessions[0].updated_at;
    assert!(
        second_updated > first_updated,
        "should have updated timestamp"
    );

    // Now simulate a scenario where sidecar was updated but index wasn't:
    // Rewrite sidecar with newer timestamp but don't touch index
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    session.touch();

    // Write directly to sidecar file (bypassing index update)
    // Meta files are stored as {store_dir}/{session_id}.meta
    let meta_path = dir.path().join(format!("{}.meta", session_id.0));
    let meta = SessionMeta {
        id: session_id.clone(),
        created_at: session.created_at(),
        updated_at: session.updated_at(),
        message_count: session.messages().len(),
        total_tokens: session.total_tokens(),
        metadata: session.metadata().clone(),
    };
    tokio::fs::write(&meta_path, serde_json::to_vec(&meta)?).await?;

    // Create fresh store (simulating restart) - counts match but metadata is stale
    let store2 = JsonlStore::new(dir.path().to_path_buf());
    let sessions = store2.list(meerkat_store::SessionFilter::default()).await?;

    // Should have the newer timestamp from reconciliation
    assert_eq!(sessions.len(), 1);
    assert!(
        sessions[0].updated_at > second_updated,
        "reconciliation should have picked up newer sidecar timestamp"
    );

    Ok(())
}
