use async_trait::async_trait;
use meerkat_core::{Session, SessionId, SessionMeta};
use meerkat_store::{SessionFilter, SessionStore, SessionStoreError};
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
    async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
        self.sessions
            .write()
            .await
            .insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn save_authoritative_projection(
        &self,
        session: &Session,
    ) -> Result<(), SessionStoreError> {
        self.sessions
            .write()
            .await
            .insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn save_authoritative_projection_if_current_revision(
        &self,
        session: &Session,
        expected_current_revision: Option<String>,
    ) -> Result<(), SessionStoreError> {
        let mut sessions = self.sessions.write().await;
        let previous = sessions.get(session.id());
        meerkat_core::session_store::authoritative_projection_current_revision_guard(
            session,
            previous,
            expected_current_revision.as_deref(),
        )?;
        sessions.insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
        Ok(self.sessions.read().await.get(id).cloned())
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
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

    async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
        self.sessions.write().await.remove(id);
        Ok(())
    }

    async fn delete_if_current_revision(
        &self,
        id: &SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, SessionStoreError> {
        let mut sessions = self.sessions.write().await;
        let Some(previous) = sessions.get(id) else {
            return Ok(false);
        };
        let previous_token = meerkat_core::session_store::session_projection_cas_token(previous)?;
        if previous_token != expected_current_revision {
            return Ok(false);
        }
        sessions.remove(id);
        Ok(true)
    }
}
