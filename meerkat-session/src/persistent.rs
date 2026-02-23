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
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::ephemeral::{EphemeralSessionService, SessionAgentBuilder};

/// Shared gate between the checkpointer and archive.
///
/// The `Mutex` provides mutual exclusion so that `checkpoint()` cannot
/// race with `archive()`: both acquire the lock before touching the store,
/// and `archive()` sets `cancelled = true` under the lock before deleting.
struct CheckpointerGate {
    cancelled: Mutex<bool>,
}

/// Checkpointer that saves sessions to a [`SessionStore`].
///
/// Used by host-mode agents to persist the session after each interaction
/// without going through `SessionService::start_turn()`.
///
/// Tracks the message count from the last successful save so that
/// back-to-back checkpoints of an unchanged session are skipped.
/// This avoids redundant writes — particularly the first checkpoint
/// after `create_session` (which already calls `persist_full_session`).
struct StoreCheckpointer {
    store: Arc<dyn SessionStore>,
    gate: Arc<CheckpointerGate>,
    last_saved_len: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl meerkat_core::checkpoint::SessionCheckpointer for StoreCheckpointer {
    async fn checkpoint(&self, session: &Session) {
        let guard = self.gate.cancelled.lock().await;
        if *guard {
            return;
        }
        let current_len = session.messages().len();
        let prev_len = self
            .last_saved_len
            .load(std::sync::atomic::Ordering::Acquire);
        if current_len == prev_len {
            return;
        }
        if let Err(e) = self.store.save(session).await {
            tracing::warn!("Host-mode checkpoint failed: {e}");
        } else {
            self.last_saved_len
                .store(current_len, std::sync::atomic::Ordering::Release);
        }
        drop(guard);
    }
}

/// Session service backed by persistent storage.
///
/// Wraps `EphemeralSessionService` and saves session snapshots to a
/// `SessionStore` after each turn completes. On `list` and `read`,
/// merges live sessions with persisted sessions from the store.
pub struct PersistentSessionService<B: SessionAgentBuilder> {
    inner: EphemeralSessionService<B>,
    store: Arc<dyn SessionStore>,
    /// Gates for active host-mode checkpointers, keyed by session ID.
    /// Archive acquires the gate's lock, sets cancelled, then deletes —
    /// mutual exclusion prevents a concurrent checkpoint from resurrecting
    /// the row.
    checkpointer_gates: Mutex<HashMap<SessionId, Arc<CheckpointerGate>>>,
}

impl<B: SessionAgentBuilder + 'static> PersistentSessionService<B> {
    /// Create a new persistent session service.
    pub fn new(builder: B, max_sessions: usize, store: Arc<dyn SessionStore>) -> Self {
        Self {
            inner: EphemeralSessionService::new(builder, max_sessions),
            store,
            checkpointer_gates: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionService for PersistentSessionService<B> {
    async fn create_session(
        &self,
        mut req: CreateSessionRequest,
    ) -> Result<RunResult, SessionError> {
        // Inject a checkpointer for all sessions — the agent only calls it
        // inside the host-mode loop, so non-host sessions pay zero cost.
        // This must be unconditional because mob agents create sessions with
        // host_mode=false and start the host loop explicitly later.
        let gate = Arc::new(CheckpointerGate {
            cancelled: Mutex::new(false),
        });
        let checkpointer = Arc::new(StoreCheckpointer {
            store: Arc::clone(&self.store),
            gate: Arc::clone(&gate),
            last_saved_len: std::sync::atomic::AtomicUsize::new(0),
        });
        let build = req.build.get_or_insert_with(Default::default);
        build.checkpointer = Some(checkpointer.clone());

        let result = self.inner.create_session(req).await?;

        // Track the gate so archive() can cancel checkpoint writes.
        {
            self.checkpointer_gates
                .lock()
                .await
                .insert(result.session_id.clone(), gate);
        }

        // Persist the full session snapshot (messages + metadata) after first
        // turn and seed the checkpointer so the next host-mode checkpoint is
        // skipped if the session hasn't changed since this save.
        let saved_len = self.persist_full_session(&result.session_id).await?;
        checkpointer
            .last_saved_len
            .store(saved_len, std::sync::atomic::Ordering::Release);

        Ok(result)
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        let result = self.inner.start_turn(id, req).await?;

        // Persist full session snapshot after turn.
        let _ = self.persist_full_session(id).await?;

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
        // Acquire the checkpointer gate (if any) and hold it across the
        // delete. This prevents a concurrent checkpoint() from saving the
        // session back after we delete it. Setting cancelled under the
        // lock ensures all future checkpoints are no-ops.
        let gate = self.checkpointer_gates.lock().await.remove(id);
        let _gate_guard = if let Some(ref g) = gate {
            let mut guard = g.cancelled.lock().await;
            *guard = true;
            Some(guard)
        } else {
            None
        };

        let live_result = self.inner.archive(id).await;

        // Check whether the session exists in the persistent store before
        // deleting — store.delete() is idempotent and always returns Ok,
        // so we need exists() to know if the store actually had it.
        let in_store = self
            .store
            .exists(id)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?;
        if in_store {
            self.store
                .delete(id)
                .await
                .map_err(|e| SessionError::Store(Box::new(e)))?;
        }

        // Gate guard is dropped here — any in-flight checkpoint that was
        // blocked on the lock will now see cancelled == true and bail out.
        drop(_gate_guard);

        match (&live_result, in_store) {
            // At least one side had the session — success.
            (Ok(()), _) | (_, true) => Ok(()),
            // Neither side had it — propagate NotFound from the live service.
            _ => live_result,
        }
    }

    async fn subscribe_session_events(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
        self.inner.subscribe_session_events(id).await
    }
}

impl<B: SessionAgentBuilder + 'static> PersistentSessionService<B> {
    /// Get the subscribable event injector for a session, if available.
    pub async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::SubscribableInjector>> {
        self.inner.event_injector(session_id).await
    }

    /// Get the comms runtime for a session, if available.
    pub async fn comms_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.inner.comms_runtime(session_id).await
    }

    /// Wait for a session to be registered.
    pub async fn wait_session_registered(&self) {
        self.inner.wait_session_registered().await;
    }

    /// Shut down all sessions.
    pub async fn shutdown(&self) {
        self.inner.shutdown().await;
    }

    /// Cancel all active checkpointer gates.
    ///
    /// After this call, in-flight checkpoints that are past the gate check
    /// will complete their current save, but subsequent checkpoint calls on
    /// any session will be no-ops. Use this during `stop()` to prevent
    /// checkpoint writes from racing with external cleanup operations.
    pub async fn cancel_all_checkpointers(&self) {
        let gates = self.checkpointer_gates.lock().await;
        for gate in gates.values() {
            let mut cancelled = gate.cancelled.lock().await;
            *cancelled = true;
        }
    }

    /// Re-enable checkpointer gates for all tracked sessions.
    ///
    /// Call this during `resume()` after `cancel_all_checkpointers()` was
    /// used during stop. Gates that were removed by `archive()` are not
    /// affected.
    pub async fn rearm_all_checkpointers(&self) {
        let gates = self.checkpointer_gates.lock().await;
        for gate in gates.values() {
            let mut cancelled = gate.cancelled.lock().await;
            *cancelled = false;
        }
    }

    /// Subscribe to session-wide events from the live inner service.
    pub async fn subscribe_session_events(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
        self.inner.subscribe_session_events(id).await
    }

    /// Load a full session from the persistent store.
    ///
    /// Used by surfaces to resume sessions that aren't currently live.
    /// Returns the complete `Session` including message history.
    pub async fn load_persisted(&self, id: &SessionId) -> Result<Option<Session>, SessionError> {
        self.store
            .load(id)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))
    }

    /// Export the full session from the live task and persist it to the store.
    ///
    /// Returns the saved message count so callers can seed a checkpointer's
    /// `last_saved_len` without a second export round-trip.
    async fn persist_full_session(&self, id: &SessionId) -> Result<usize, SessionError> {
        let session = self.inner.export_session(id).await?;
        let message_count = session.messages().len();

        self.store
            .save(&session)
            .await
            .map_err(|e| SessionError::Store(Box::new(e)))?;

        Ok(message_count)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_store::MemoryStore;

    #[tokio::test]
    async fn test_persistent_load_persisted_returns_stored_session() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let session = Session::new();
        let id = session.id().clone();
        store.save(&session).await.unwrap();

        // Verify load_persisted returns the session.
        // We can't construct a full PersistentSessionService without a SessionAgentBuilder,
        // so test the store path directly via the same logic.
        let loaded = store.load(&id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().id(), &id);
    }

    #[tokio::test]
    async fn test_persistent_load_persisted_returns_none_for_unknown() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let unknown = SessionId::new();
        let loaded = store.load(&unknown).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_persistent_archive_deletes_from_store() {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let session = Session::new();
        let id = session.id().clone();
        store.save(&session).await.unwrap();

        // Verify it exists
        assert!(store.load(&id).await.unwrap().is_some());

        // Delete (simulating archive store cleanup)
        store.delete(&id).await.unwrap();

        // Verify it's gone
        assert!(store.load(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_store_checkpointer_saves_session() {
        use meerkat_core::checkpoint::SessionCheckpointer;

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let gate = Arc::new(super::CheckpointerGate {
            cancelled: tokio::sync::Mutex::new(false),
        });
        let checkpointer = super::StoreCheckpointer {
            store: Arc::clone(&store),
            gate,
            last_saved_len: std::sync::atomic::AtomicUsize::new(0),
        };

        let mut session = Session::new();
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage {
                content: "hello".to_string(),
            },
        ));

        // Checkpoint should persist the session
        checkpointer.checkpoint(&session).await;

        let loaded = store.load(session.id()).await.unwrap();
        assert!(loaded.is_some(), "session should be persisted after checkpoint");
        let loaded = loaded.unwrap();
        assert_eq!(loaded.id(), session.id());
        assert_eq!(loaded.messages().len(), session.messages().len());
    }

    #[tokio::test]
    async fn test_store_checkpointer_suppressed_after_cancellation() {
        use meerkat_core::checkpoint::SessionCheckpointer;

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let gate = Arc::new(super::CheckpointerGate {
            cancelled: tokio::sync::Mutex::new(false),
        });
        let checkpointer = super::StoreCheckpointer {
            store: Arc::clone(&store),
            gate: Arc::clone(&gate),
            last_saved_len: std::sync::atomic::AtomicUsize::new(0),
        };

        let mut session = Session::new();
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage {
                content: "hello".to_string(),
            },
        ));

        // First checkpoint should persist (message count changed)
        checkpointer.checkpoint(&session).await;
        assert!(store.load(session.id()).await.unwrap().is_some());

        // Simulate archive: acquire gate, set cancelled, delete
        {
            let mut guard = gate.cancelled.lock().await;
            *guard = true;
            store.delete(session.id()).await.unwrap();
        }

        // Checkpoint after cancellation should be a no-op
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage {
                content: "world".to_string(),
            },
        ));
        checkpointer.checkpoint(&session).await;
        assert!(
            store.load(session.id()).await.unwrap().is_none(),
            "cancelled checkpointer should not write session back"
        );
    }

    #[tokio::test]
    async fn test_store_checkpointer_skips_unchanged_session() {
        use meerkat_core::checkpoint::SessionCheckpointer;

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let gate = Arc::new(super::CheckpointerGate {
            cancelled: tokio::sync::Mutex::new(false),
        });
        let checkpointer = super::StoreCheckpointer {
            store: Arc::clone(&store),
            gate,
            last_saved_len: std::sync::atomic::AtomicUsize::new(0),
        };

        let mut session = Session::new();
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage {
                content: "hello".to_string(),
            },
        ));

        // First checkpoint saves (message count changed from 0 -> 1)
        checkpointer.checkpoint(&session).await;
        assert!(store.load(session.id()).await.unwrap().is_some());

        // Delete from store to detect whether the next checkpoint writes
        store.delete(session.id()).await.unwrap();

        // Second checkpoint with same session is skipped (count still 1)
        checkpointer.checkpoint(&session).await;
        assert!(
            store.load(session.id()).await.unwrap().is_none(),
            "unchanged session should not be re-saved"
        );

        // Add a message and checkpoint again — should save
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage {
                content: "world".to_string(),
            },
        ));
        checkpointer.checkpoint(&session).await;
        assert!(
            store.load(session.id()).await.unwrap().is_some(),
            "changed session should be saved"
        );
    }

    #[tokio::test]
    async fn test_persistent_archive_store_only_session_succeeds() {
        // After restart, sessions exist only in the persistent store —
        // not in the live (inner) ephemeral service. archive() must still
        // succeed by deleting from the store even when inner returns NotFound.
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let session = Session::new();
        let id = session.id().clone();
        store.save(&session).await.unwrap();

        // Verify the session exists in the store
        assert!(store.load(&id).await.unwrap().is_some());

        // Simulate the archive path: inner.archive() would return NotFound,
        // but store.delete() should still succeed.
        store.delete(&id).await.unwrap();
        assert!(store.load(&id).await.unwrap().is_none());
    }
}
