use meerkat_core::AgentSessionStore;
use meerkat_store::{MemoryStore, StoreAdapter};
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
