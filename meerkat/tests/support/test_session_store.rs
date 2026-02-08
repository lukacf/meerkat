use async_trait::async_trait;
use meerkat_core::{Session, SessionId, SessionMeta};
use meerkat_store::{SessionFilter, SessionStore, StoreError};
use std::collections::HashMap;
use tokio::sync::RwLock;

#[derive(Default)]
pub struct TestSessionStore {
    sessions: RwLock<HashMap<SessionId, Session>>,
}

impl TestSessionStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SessionStore for TestSessionStore {
    async fn save(&self, session: &Session) -> Result<(), StoreError> {
        self.sessions
            .write()
            .await
            .insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        Ok(self.sessions.read().await.get(id).cloned())
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError> {
        let mut sessions: Vec<_> = self
            .sessions
            .read()
            .await
            .values()
            .map(SessionMeta::from)
            .collect();

        if let Some(created_after) = filter.created_after {
            sessions.retain(|meta| meta.created_at > created_after);
        }
        if let Some(updated_after) = filter.updated_after {
            sessions.retain(|meta| meta.updated_at > updated_after);
        }

        let offset = filter.offset.unwrap_or(0);
        let limit = filter.limit.unwrap_or(usize::MAX);
        Ok(sessions.into_iter().skip(offset).take(limit).collect())
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        self.sessions.write().await.remove(id);
        Ok(())
    }
}
