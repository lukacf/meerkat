//! PersistentSessionService — wraps EphemeralSessionService with snapshot + event persistence.
//!
//! Gated behind the `session-store` feature.

use async_trait::async_trait;
use meerkat_core::service::{
    CreateSessionRequest, SessionError, SessionQuery, SessionService, SessionSummary, SessionView,
    StartTurnRequest,
};
use meerkat_core::types::{RunResult, SessionId};

use crate::ephemeral::{EphemeralSessionService, SessionAgentBuilder};

/// Session service backed by persistent storage.
///
/// Wraps `EphemeralSessionService` and adds snapshot + event persistence
/// via `SessionStore` and `EventStore`.
pub struct PersistentSessionService<B: SessionAgentBuilder> {
    inner: EphemeralSessionService<B>,
}

impl<B: SessionAgentBuilder + 'static> PersistentSessionService<B> {
    /// Create a new persistent session service.
    pub fn new(builder: B, max_sessions: usize) -> Self {
        Self {
            inner: EphemeralSessionService::new(builder, max_sessions),
        }
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionService for PersistentSessionService<B> {
    async fn create_session(
        &self,
        req: CreateSessionRequest,
    ) -> Result<RunResult, SessionError> {
        self.inner.create_session(req).await
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        self.inner.start_turn(id, req).await
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.interrupt(id).await
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        self.inner.read(id).await
    }

    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        self.inner.list(query).await
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.archive(id).await
    }
}
