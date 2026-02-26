//! In-memory session store (for testing)

use crate::{SessionFilter, SessionStore, StoreError};
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
    async fn save(&self, session: &Session) -> Result<(), StoreError> {
        let mut sessions = self.sessions.write().await;
        sessions.insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        let sessions = self.sessions.read().await;
        Ok(sessions.get(id).cloned())
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError> {
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

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(id);
        Ok(())
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
        session.push(Message::User(UserMessage {
            content: "Test".to_string(),
        }));

        let id = session.id().clone();

        store.save(&session).await?;

        let loaded = store.load(&id).await?.ok_or("session not found")?;
        assert_eq!(loaded.id(), &id);
        assert_eq!(loaded.messages().len(), 1);
        Ok(())
    }
}
