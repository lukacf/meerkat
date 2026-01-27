//! Adapter from SessionStore to AgentSessionStore.

use async_trait::async_trait;
use meerkat_core::{AgentError, AgentSessionStore, Session, SessionId};
use std::sync::Arc;

use crate::SessionStore;

/// Adapter that wraps a SessionStore to implement AgentSessionStore.
pub struct StoreAdapter<S: SessionStore> {
    store: Arc<S>,
}

impl<S: SessionStore> StoreAdapter<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<S: SessionStore + 'static> AgentSessionStore for StoreAdapter<S> {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        self.store
            .save(session)
            .await
            .map_err(|e| AgentError::StoreError(e.to_string()))
    }

    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        let session_id = SessionId::parse(id)
            .map_err(|e| AgentError::StoreError(format!("Invalid session ID: {}", e)))?;

        self.store
            .load(&session_id)
            .await
            .map_err(|e| AgentError::StoreError(e.to_string()))
    }
}
