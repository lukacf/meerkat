//! PersistentSessionService — wraps EphemeralSessionService with snapshot + event persistence.
//!
//! Gated behind the `session-store` feature.
//!
//! After each turn completes, the session snapshot is saved to the `SessionStore`
//! and events are appended to the `EventStore`. On `read` and `list`, persisted
//! sessions are merged with live (ephemeral) sessions.

use async_trait::async_trait;
use indexmap::IndexSet;
#[allow(unused_imports)] // Used in read() fallback path
use meerkat_core::Session;
use meerkat_core::service::{
    CreateSessionRequest, SessionError, SessionInfo, SessionQuery, SessionService, SessionSummary,
    SessionUsage, SessionView, StartTurnRequest,
};
use meerkat_core::types::{RunResult, SessionId};
use meerkat_store::SessionStore;
use std::sync::Arc;

use crate::ephemeral::{EphemeralSessionService, SessionAgentBuilder};

/// Session service backed by persistent storage.
///
/// Wraps `EphemeralSessionService` and saves session snapshots to a
/// `SessionStore` after each turn completes. On `list` and `read`,
/// merges live sessions with persisted sessions from the store.
pub struct PersistentSessionService<B: SessionAgentBuilder> {
    inner: EphemeralSessionService<B>,
    store: Arc<dyn SessionStore>,
}

impl<B: SessionAgentBuilder + 'static> PersistentSessionService<B> {
    /// Create a new persistent session service.
    pub fn new(builder: B, max_sessions: usize, store: Arc<dyn SessionStore>) -> Self {
        Self {
            inner: EphemeralSessionService::new(builder, max_sessions),
            store,
        }
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionService for PersistentSessionService<B> {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        let result = self.inner.create_session(req).await?;

        // Persist the full session snapshot (messages + metadata) after first turn.
        self.persist_full_session(&result.session_id).await?;

        Ok(result)
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        let result = self.inner.start_turn(id, req).await?;

        // Persist full session snapshot after turn.
        self.persist_full_session(id).await?;

        Ok(result)
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.interrupt(id).await
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        // Try live session first
        match self.inner.read(id).await {
            Ok(view) => Ok(view),
            Err(SessionError::NotFound { .. }) => {
                // Fall back to persisted session
                let session = self
                    .store
                    .load(id)
                    .await
                    .map_err(|e| SessionError::Store(Box::new(e)))?
                    .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;

                Ok(SessionView {
                    state: SessionInfo {
                        session_id: session.id().clone(),
                        created_at: session.created_at(),
                        updated_at: session.updated_at(),
                        message_count: session.messages().len(),
                        is_active: false,
                        last_assistant_text: session.last_assistant_text(),
                    },
                    billing: SessionUsage {
                        total_tokens: session.total_tokens(),
                        usage: session.total_usage(),
                    },
                })
            }
            Err(e) => Err(e),
        }
    }

    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        // Get live sessions
        let mut summaries = self.inner.list(SessionQuery::default()).await?;
        let live_ids: IndexSet<_> = summaries.iter().map(|s| s.session_id.clone()).collect();

        // Merge persisted sessions not currently live
        let stored = self
            .store
            .list(meerkat_store::SessionFilter::default())
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?;

        for meta in stored {
            if !live_ids.contains(&meta.id) {
                summaries.push(SessionSummary {
                    session_id: meta.id,
                    created_at: meta.created_at,
                    updated_at: meta.updated_at,
                    message_count: meta.message_count,
                    total_tokens: meta.total_tokens,
                    is_active: false,
                });
            }
        }

        // Apply pagination
        if let Some(offset) = query.offset {
            if offset < summaries.len() {
                summaries = summaries.split_off(offset);
            } else {
                summaries.clear();
            }
        }
        if let Some(limit) = query.limit {
            summaries.truncate(limit);
        }

        Ok(summaries)
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.archive(id).await
    }
}

impl<B: SessionAgentBuilder + 'static> PersistentSessionService<B> {
    /// Export the full session from the live task and persist it to the store.
    ///
    /// This saves the complete session including all messages, metadata, and
    /// usage — not just a lightweight summary.
    async fn persist_full_session(&self, id: &SessionId) -> Result<(), SessionError> {
        let session = self.inner.export_session(id).await?;

        self.store
            .save(&session)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))
    }
}
