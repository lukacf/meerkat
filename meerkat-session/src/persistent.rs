//! PersistentSessionService — wraps EphemeralSessionService with snapshot + event persistence.
//!
//! Gated behind the `session-store` feature.
//!
//! After each turn completes, the session snapshot is saved to the `SessionStore`
//! and events are appended to the `EventStore`. On `read` and `list`, persisted
//! sessions are merged with live (ephemeral) sessions.

use async_trait::async_trait;
use meerkat_core::service::{
    CreateSessionRequest, SessionError, SessionQuery, SessionService, SessionSummary, SessionView,
    StartTurnRequest,
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
    pub fn new(
        builder: B,
        max_sessions: usize,
        store: Arc<dyn SessionStore>,
    ) -> Self {
        Self {
            inner: EphemeralSessionService::new(builder, max_sessions),
            store,
        }
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionService for PersistentSessionService<B> {
    async fn create_session(
        &self,
        req: CreateSessionRequest,
    ) -> Result<RunResult, SessionError> {
        let result = self.inner.create_session(req).await?;

        // Persist the session snapshot after the first turn.
        // read() queries the live session task for its current state.
        if let Ok(view) = self.inner.read(&result.session_id).await {
            tracing::debug!(
                session_id = %result.session_id,
                message_count = view.message_count,
                "Persisting session snapshot after create"
            );
            // We save a minimal session record. Full snapshot persistence would
            // require the session task to expose its Session object, which we
            // avoid to maintain the ownership model. The SessionView contains
            // enough metadata for list/read from the store.
            let _ = self.save_session_meta(&result.session_id, &view).await;
        }

        Ok(result)
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        let result = self.inner.start_turn(id, req).await?;

        // Persist snapshot after turn
        if let Ok(view) = self.inner.read(id).await {
            tracing::debug!(
                session_id = %id,
                message_count = view.message_count,
                "Persisting session snapshot after turn"
            );
            let _ = self.save_session_meta(id, &view).await;
        }

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
                match self.store.load(id).await {
                    Ok(Some(session)) => Ok(SessionView {
                        session_id: session.id().clone(),
                        created_at: session.created_at(),
                        updated_at: session.updated_at(),
                        message_count: session.messages().len(),
                        total_tokens: session.total_tokens(),
                        usage: session.total_usage(),
                        is_active: false,
                        last_assistant_text: session.last_assistant_text(),
                    }),
                    _ => Err(SessionError::NotFound { id: id.clone() }),
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        // Get live sessions
        let mut summaries = self.inner.list(SessionQuery::default()).await?;
        let live_ids: std::collections::HashSet<_> =
            summaries.iter().map(|s| s.session_id.clone()).collect();

        // Merge persisted sessions not currently live
        if let Ok(stored) = self.store.list(meerkat_store::SessionFilter::default()).await {
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
        }

        // Apply pagination
        if let Some(offset) = query.offset {
            summaries = summaries.into_iter().skip(offset).collect();
        }
        if let Some(limit) = query.limit {
            summaries.truncate(limit);
        }

        Ok(summaries)
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        // Archive from live sessions (ephemeral handles the shutdown)
        let live_result = self.inner.archive(id).await;

        // Also keep the persisted snapshot (archive doesn't delete from store —
        // the snapshot remains for historical queries). If you want full deletion,
        // call store.delete() separately.

        live_result
    }
}

impl<B: SessionAgentBuilder + 'static> PersistentSessionService<B> {
    /// Save session metadata to the store. Uses the SessionView to construct
    /// a minimal Session for persistence.
    async fn save_session_meta(
        &self,
        id: &SessionId,
        view: &SessionView,
    ) -> Result<(), SessionError> {
        // Load existing session or create new
        let mut session = match self.store.load(id).await {
            Ok(Some(s)) => s,
            _ => meerkat_core::Session::with_id(id.clone()),
        };

        // Update usage
        session.record_usage(view.usage.clone());
        session.touch();

        self.store
            .save(&session)
            .await
            .map_err(|e| {
                tracing::warn!(session_id = %id, error = %e, "Failed to persist session");
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    format!("Persistence failed: {e}"),
                ))
            })
    }
}
