//! Reference `SessionStore` → `SessionStore` delegating wrappers.
//!
//! [`ForwardingSessionStore`] is the documented pattern: a delegating wrapper
//! MUST forward `as_incremental` (and every default-provided method it does
//! not intercept). [`SwallowingSessionStore`] is the bug class the
//! capability-discovery chapter exists to catch: it forwards every async
//! method faithfully but leaves `as_incremental` on the trait default
//! (`None`), silently downgrading `PersistentSessionService` — which probes
//! the capability exactly once at construction — to whole-blob persistence.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::session_store::IncrementalSessionStore;
use meerkat_core::{
    Session, SessionFilter, SessionId, SessionMeta, SessionStore, SessionStoreError,
    TranscriptRewriteCommit,
};

/// Correctly forwarding delegating wrapper (the documented pattern).
pub struct ForwardingSessionStore {
    inner: Arc<dyn SessionStore>,
}

impl ForwardingSessionStore {
    pub fn new(inner: Arc<dyn SessionStore>) -> Self {
        Self { inner }
    }

    /// Convenience: wrap into an erased handle.
    pub fn wrap(inner: Arc<dyn SessionStore>) -> Arc<dyn SessionStore> {
        Arc::new(Self::new(inner))
    }
}

#[async_trait]
impl SessionStore for ForwardingSessionStore {
    async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
        self.inner.save(session).await
    }

    async fn save_transcript_rewrite(
        &self,
        session: &Session,
        commit: &TranscriptRewriteCommit,
    ) -> Result<(), SessionStoreError> {
        self.inner.save_transcript_rewrite(session, commit).await
    }

    async fn save_authoritative_projection(
        &self,
        session: &Session,
    ) -> Result<(), SessionStoreError> {
        self.inner.save_authoritative_projection(session).await
    }

    async fn save_authoritative_projection_if_current_revision(
        &self,
        session: &Session,
        expected_current_revision: Option<String>,
    ) -> Result<(), SessionStoreError> {
        self.inner
            .save_authoritative_projection_if_current_revision(session, expected_current_revision)
            .await
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
        self.inner.load(id).await
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
        self.inner.list(filter).await
    }

    async fn load_meta(&self, id: &SessionId) -> Result<Option<SessionMeta>, SessionStoreError> {
        self.inner.load_meta(id).await
    }

    async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
        self.inner.delete(id).await
    }

    async fn delete_if_current_revision(
        &self,
        id: &SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, SessionStoreError> {
        self.inner
            .delete_if_current_revision(id, expected_current_revision)
            .await
    }

    async fn exists(&self, id: &SessionId) -> Result<bool, SessionStoreError> {
        self.inner.exists(id).await
    }

    fn as_incremental(self: Arc<Self>) -> Option<Arc<dyn IncrementalSessionStore>> {
        // The forwarding contract: delegate the typed capability accessor so
        // the runtime keeps the O(delta) incremental path.
        self.inner.clone().as_incremental()
    }
}

/// Deliberately swallowing wrapper: forwards every async method but NOT
/// `as_incremental` (the trait default returns `None`).
///
/// This is a reference implementation of the bug class, kept in-crate to
/// self-test the harness and to document the exact shape reviewers should
/// reject in delegating wrappers.
pub struct SwallowingSessionStore {
    inner: Arc<dyn SessionStore>,
}

impl SwallowingSessionStore {
    pub fn new(inner: Arc<dyn SessionStore>) -> Self {
        Self { inner }
    }

    /// Convenience: wrap into an erased handle.
    pub fn wrap(inner: Arc<dyn SessionStore>) -> Arc<dyn SessionStore> {
        Arc::new(Self::new(inner))
    }
}

#[async_trait]
impl SessionStore for SwallowingSessionStore {
    async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
        self.inner.save(session).await
    }

    async fn save_transcript_rewrite(
        &self,
        session: &Session,
        commit: &TranscriptRewriteCommit,
    ) -> Result<(), SessionStoreError> {
        self.inner.save_transcript_rewrite(session, commit).await
    }

    async fn save_authoritative_projection(
        &self,
        session: &Session,
    ) -> Result<(), SessionStoreError> {
        self.inner.save_authoritative_projection(session).await
    }

    async fn save_authoritative_projection_if_current_revision(
        &self,
        session: &Session,
        expected_current_revision: Option<String>,
    ) -> Result<(), SessionStoreError> {
        self.inner
            .save_authoritative_projection_if_current_revision(session, expected_current_revision)
            .await
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
        self.inner.load(id).await
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
        self.inner.list(filter).await
    }

    async fn load_meta(&self, id: &SessionId) -> Result<Option<SessionMeta>, SessionStoreError> {
        self.inner.load_meta(id).await
    }

    async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
        self.inner.delete(id).await
    }

    async fn delete_if_current_revision(
        &self,
        id: &SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, SessionStoreError> {
        self.inner
            .delete_if_current_revision(id, expected_current_revision)
            .await
    }

    async fn exists(&self, id: &SessionId) -> Result<bool, SessionStoreError> {
        self.inner.exists(id).await
    }

    // NOTE deliberately missing:
    //
    //     fn as_incremental(self: Arc<Self>) -> Option<Arc<dyn IncrementalSessionStore>>
    //
    // The trait default returns `None`, which silently degrades incremental
    // persistence to whole-blob saves. This is the swallow.
}
