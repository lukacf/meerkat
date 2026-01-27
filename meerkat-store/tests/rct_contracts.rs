use async_trait::async_trait;
use meerkat_core::{AgentError, AgentSessionStore, Session, SessionId};
use meerkat_store::{SessionFilter, SessionStore, StoreAdapter, StoreError};
use std::sync::Arc;

struct NoopStore;

#[async_trait]
impl SessionStore for NoopStore {
    async fn save(&self, _session: &Session) -> Result<(), StoreError> {
        Ok(())
    }

    async fn load(&self, _id: &SessionId) -> Result<Option<Session>, StoreError> {
        Ok(None)
    }

    async fn list(
        &self,
        _filter: SessionFilter,
    ) -> Result<Vec<meerkat_core::SessionMeta>, StoreError> {
        Ok(Vec::new())
    }

    async fn delete(&self, _id: &SessionId) -> Result<(), StoreError> {
        Ok(())
    }
}

#[tokio::test]
async fn test_session_store_adapter_error_mapping() {
    let store = Arc::new(NoopStore);
    let adapter = StoreAdapter::new(store);

    let err = adapter.load("not-a-uuid").await.unwrap_err();
    match err {
        AgentError::StoreError(msg) => {
            assert!(msg.contains("Invalid session ID"));
        }
        _ => panic!("unexpected error type"),
    }
}
