//! In-memory session store (for testing)

use crate::{SessionFilter, SessionStore, SessionStoreError};
use async_trait::async_trait;
use meerkat_core::{Session, SessionId, SessionMeta};
use std::collections::HashMap;

// Use crate-level tokio alias for consistency with other crates.
#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use ::tokio::sync::RwLock;

/// In-memory session store
pub struct MemoryStore {
    sessions: RwLock<HashMap<SessionId, Session>>,
}

impl MemoryStore {
    /// Create a new in-memory store
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionStore for MemoryStore {
    async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
        let mut sessions = self.sessions.write().await;
        // F1 closure (wave-c C-H1): same shrink-guard as persistent
        // backends so behaviour is uniform across `SessionStore`
        // implementations.
        let previous = sessions.get(session.id());
        meerkat_core::session_store::append_only_save_guard(session, previous)?;
        sessions.insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn save_transcript_rewrite(
        &self,
        session: &Session,
        commit: &meerkat_core::TranscriptRewriteCommit,
    ) -> Result<(), SessionStoreError> {
        let mut sessions = self.sessions.write().await;
        let previous = sessions.get(session.id());
        meerkat_core::session_store::transcript_rewrite_save_guard(session, previous, commit)?;
        sessions.insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn save_authoritative_projection(
        &self,
        session: &Session,
    ) -> Result<(), SessionStoreError> {
        let mut sessions = self.sessions.write().await;
        sessions.insert(session.id().clone(), session.clone());
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
        let sessions = self.sessions.read().await;
        Ok(sessions.get(id).cloned())
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
        let sessions = self.sessions.read().await;
        let mut metas: Vec<SessionMeta> = sessions
            .values()
            .filter(|s| {
                if let Some(created_after) = filter.created_after
                    && s.created_at() < created_after
                {
                    return false;
                }
                if let Some(updated_after) = filter.updated_after
                    && s.updated_at() < updated_after
                {
                    return false;
                }
                true
            })
            .map(SessionMeta::from)
            .collect();

        // Sort by updated_at descending
        metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        // Apply pagination
        let offset = filter.offset.unwrap_or(0);
        let limit = filter.limit.unwrap_or(usize::MAX);

        Ok(metas.into_iter().skip(offset).take(limit).collect())
    }

    async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(id);
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::{Message, UserMessage};

    #[tokio::test]
    async fn test_memory_store_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let store = MemoryStore::new();

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("Test".to_string())));

        let id = session.id().clone();

        store.save(&session).await?;

        let loaded = store.load(&id).await?.ok_or("session not found")?;
        assert_eq!(loaded.id(), &id);
        assert_eq!(loaded.messages().len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn authoritative_projection_expected_revision_rejects_stale_writer()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = MemoryStore::new();

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("base".to_string())));
        store.save(&session).await?;
        let expected_revision = session.transcript_revision()?;

        let mut newer = session.clone();
        newer.push(Message::User(UserMessage::text("newer".to_string())));
        store.save(&newer).await?;

        let mut stale_projection = session.clone();
        stale_projection.push(Message::User(UserMessage::text("stale".to_string())));
        let err = store
            .save_authoritative_projection_if_current_revision(
                &stale_projection,
                Some(expected_revision),
            )
            .await
            .expect_err("stale authoritative projection should be rejected");
        assert!(matches!(
            err,
            SessionStoreError::TranscriptContinuityViolation { .. }
        ));

        let saved = store
            .load(session.id())
            .await?
            .expect("session should remain saved");
        assert_eq!(saved.messages().len(), newer.messages().len());
        assert_eq!(saved.transcript_revision()?, newer.transcript_revision()?);
        Ok(())
    }

    #[tokio::test]
    async fn delete_if_current_revision_only_deletes_matching_projection()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = MemoryStore::new();

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("base".to_string())));
        store.save(&session).await?;
        let stale_token = meerkat_core::session_store::session_projection_cas_token(&session)?;

        let mut newer = session.clone();
        newer.push(Message::User(UserMessage::text("newer".to_string())));
        store.save(&newer).await?;
        assert!(
            !store
                .delete_if_current_revision(session.id(), &stale_token)
                .await?
        );
        assert!(store.load(session.id()).await?.is_some());

        let current_token = meerkat_core::session_store::session_projection_cas_token(&newer)?;
        assert!(
            store
                .delete_if_current_revision(session.id(), &current_token)
                .await?
        );
        assert!(store.load(session.id()).await?.is_none());
        Ok(())
    }
}
