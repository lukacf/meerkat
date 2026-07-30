//! Reference `SessionStore` → `SessionStore` delegating wrappers.
//!
//! [`ForwardingSessionStore`] is the documented pattern: a delegating wrapper
//! MUST forward `as_incremental` (and every default-provided method it does
//! not intercept). [`SwallowingSessionStore`] is the bug class the
//! capability-discovery chapter exists to catch: it forwards every async
//! method faithfully but leaves `as_incremental` on the trait default
//! (`None`), silently downgrading `PersistentSessionService` — which probes
//! the capability exactly once at construction — to whole-blob persistence.
//! [`DefaultRangeVerbIncrementalStore`] is the same bug class one level
//! deeper: an `IncrementalSessionStore` delegating wrapper that forwards the
//! eight pre-existing verbs but leaves the range-read capability verbs
//! (`load_canonical_head` / `load_rewrite_commits`) on their trait defaults.
//! That shape is fully CONFORMANT as a store (the conservative defaults are
//! legal — everything degrades to the whole-load path), but as a DELEGATING
//! WRAPPER it silently discards the inner store's capability, which the
//! capability-discovery chapter's forwarding pin makes loud.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::session_store::IncrementalSessionStore;
use meerkat_core::{
    Message, Session, SessionFilter, SessionHead, SessionHeadCas, SessionId, SessionMeta,
    SessionStore, SessionStoreError, TranscriptRewriteCommit, TranscriptRewriteRecord,
    TranscriptStrandId, VerifiedSessionHeadMaterialization,
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

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
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

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
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

/// Delegating incremental wrapper that forwards every pre-existing verb but
/// leaves the range-read capability verbs (`load_canonical_head` /
/// `load_rewrite_commits`) on their trait defaults.
///
/// As a STORE this is conformant: the conservative defaults are legal and
/// keep every reader on the whole-load path. As a DELEGATING WRAPPER it is
/// the reference bug class for the capability-discovery chapter's
/// `range_read_verbs_forwarded` pin: the inner store advertises a canonical
/// head; the wrapper silently answers `None`.
pub struct DefaultRangeVerbIncrementalStore {
    inner_store: Arc<dyn SessionStore>,
    inner_inc: Arc<dyn IncrementalSessionStore>,
}

impl DefaultRangeVerbIncrementalStore {
    /// Wrap an incremental-capable store. Panics-free: returns `None` when
    /// the inner store has no incremental capability to delegate.
    pub fn wrap(inner: Arc<dyn SessionStore>) -> Option<Arc<dyn SessionStore>> {
        let inner_inc = Arc::clone(&inner).as_incremental()?;
        Some(Arc::new(Self {
            inner_store: inner,
            inner_inc,
        }))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionStore for DefaultRangeVerbIncrementalStore {
    async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
        self.inner_store.save(session).await
    }

    async fn save_transcript_rewrite(
        &self,
        session: &Session,
        commit: &TranscriptRewriteCommit,
    ) -> Result<(), SessionStoreError> {
        self.inner_store
            .save_transcript_rewrite(session, commit)
            .await
    }

    async fn save_authoritative_projection(
        &self,
        session: &Session,
    ) -> Result<(), SessionStoreError> {
        self.inner_store
            .save_authoritative_projection(session)
            .await
    }

    async fn save_authoritative_projection_if_current_revision(
        &self,
        session: &Session,
        expected_current_revision: Option<String>,
    ) -> Result<(), SessionStoreError> {
        self.inner_store
            .save_authoritative_projection_if_current_revision(session, expected_current_revision)
            .await
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
        self.inner_store.load(id).await
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
        self.inner_store.list(filter).await
    }

    async fn load_meta(&self, id: &SessionId) -> Result<Option<SessionMeta>, SessionStoreError> {
        self.inner_store.load_meta(id).await
    }

    async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
        self.inner_store.delete(id).await
    }

    async fn delete_if_current_revision(
        &self,
        id: &SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, SessionStoreError> {
        self.inner_store
            .delete_if_current_revision(id, expected_current_revision)
            .await
    }

    async fn exists(&self, id: &SessionId) -> Result<bool, SessionStoreError> {
        self.inner_store.exists(id).await
    }

    fn as_incremental(self: Arc<Self>) -> Option<Arc<dyn IncrementalSessionStore>> {
        // The capability is advertised — but through this wrapper's own
        // incremental impl, whose range-read verbs sit on the defaults.
        Some(self)
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl IncrementalSessionStore for DefaultRangeVerbIncrementalStore {
    async fn append_messages(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        base_seq: u64,
        messages: &[Message],
    ) -> Result<(), SessionStoreError> {
        self.inner_inc
            .append_messages(id, strand, base_seq, messages)
            .await
    }

    async fn commit_rewrite(
        &self,
        id: &SessionId,
        record: &TranscriptRewriteRecord,
        expected: SessionHeadCas,
    ) -> Result<SessionHead, SessionStoreError> {
        self.inner_inc.commit_rewrite(id, record, expected).await
    }

    async fn save_head(
        &self,
        head: &SessionHead,
        expected: SessionHeadCas,
    ) -> Result<(), SessionStoreError> {
        self.inner_inc.save_head(head, expected).await
    }

    async fn load_head(&self, id: &SessionId) -> Result<Option<SessionHead>, SessionStoreError> {
        self.inner_inc.load_head(id).await
    }

    async fn materialize_head(
        &self,
        expected: &SessionHead,
    ) -> Result<VerifiedSessionHeadMaterialization, SessionStoreError> {
        self.inner_inc.materialize_head(expected).await
    }

    async fn load_messages(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        range: std::ops::Range<u64>,
    ) -> Result<Vec<Message>, SessionStoreError> {
        self.inner_inc.load_messages(id, strand, range).await
    }

    async fn load_rewrites(
        &self,
        id: &SessionId,
    ) -> Result<Vec<TranscriptRewriteRecord>, SessionStoreError> {
        self.inner_inc.load_rewrites(id).await
    }

    // NOTE deliberately missing:
    //
    //     async fn load_canonical_head(...) -> Result<Option<SessionHead>, _>
    //     async fn load_rewrite_commits(...) -> Result<Vec<TranscriptRewriteCommit>, _>
    //
    // The trait defaults answer `None` / derive-from-`load_rewrites`, which
    // silently discards the inner store's range-read capability. This is the
    // range-verb swallow.
}
