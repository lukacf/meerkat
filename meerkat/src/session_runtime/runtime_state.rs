//! Runtime-state observers and cleanup primitives.
//!
//! Populated by W1-E (`PendingSessionEventStreams`,
//! `PendingSessionEventStreamDrop`), W2-E (`SessionInfo`,
//! `SessionState` enums + state observers), and W3-A (skill-identity
//! registry plumbing).
//!
//! `ArchiveRuntimeCleanup` deliberately stays in `meerkat-rpc` until
//! W3-A: its fields reference `SessionMcpState` (RPC-private) and
//! `meerkat_mob_mcp::MobMcpState` (a crate `meerkat` does not yet
//! depend on). Moving it now would require generics + trait bounds
//! (behaviour-changing) or new `meerkat` deps; either is broader than
//! Wave 1's "pure type move" charter. Tracked in W3-A.

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat_core::EventEnvelope;
use meerkat_core::event::AgentEvent;
use meerkat_core::types::SessionId;
use tokio::sync::{Notify, broadcast};

/// Observable lifecycle state of a session.
///
/// Surfaces project this onto their wire format (`session/list`,
/// `GET /sessions/<id>`, …). The serde encoding mirrors the
/// long-standing JSON-RPC contract — surfaces depending on the wire
/// strings (`"idle"`, `"running"`, `"shutting_down"`) MUST not break.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// The session is idle and ready to accept a new turn.
    Idle,
    /// A turn is currently running.
    Running,
    /// The session is shutting down.
    ShuttingDown,
}

impl SessionState {
    /// Return a stable string representation matching the serde
    /// `rename_all` convention. Used by surfaces that bypass serde
    /// (e.g. tracing tags, telemetry counters) and need the canonical
    /// lowercase slug.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::ShuttingDown => "shutting_down",
        }
    }
}

/// Summary information about a session: identity, lifecycle marker,
/// and durable labels. Surfaces hand this back from their list / read
/// endpoints; the wire encoding mirrors the canonical contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    /// The session id this summary describes.
    pub session_id: SessionId,
    /// Lifecycle marker.
    pub state: SessionState,
    /// Durable labels stored alongside the session.
    pub labels: BTreeMap<String, String>,
}

/// Per-session pending event-stream channel pair.
///
/// While a session is staged or archived but still emitting tail
/// events, the runtime keeps a broadcast sender alive so subscribers
/// drain remaining envelopes; `receiver_dropped` notifies the runtime
/// when the last subscriber drops so the channel can be reaped.
#[derive(Clone)]
pub struct PendingSessionEventStreams {
    /// Broadcast sender that fans out [`AgentEvent`] envelopes to live
    /// subscribers.
    pub events: broadcast::Sender<EventEnvelope<AgentEvent>>,
    /// Notifier that fires when the receiver-side handle is dropped, so
    /// the runtime can prune the entry from its event-stream map.
    pub receiver_dropped: Arc<Notify>,
}

/// RAII drop guard that fires `receiver_dropped` exactly once when the
/// receiver-side handle is dropped. Paired with
/// [`PendingSessionEventStreams::receiver_dropped`].
pub struct PendingSessionEventStreamDrop {
    /// Notifier shared with the corresponding
    /// [`PendingSessionEventStreams`] entry.
    pub receiver_dropped: Arc<Notify>,
}

impl Drop for PendingSessionEventStreamDrop {
    fn drop(&mut self) {
        self.receiver_dropped.notify_one();
    }
}

/// `RuntimeStateOps` orchestrator (gated on `session-store`).
///
/// Owns the surface-agnostic session-state observers ([`discard_live_session`],
/// [`discard_stale_live_session`], [`live_session_is_stale`]). Surfaces
/// build one per call from their own SessionRuntime borrows.
///
/// `archived_persisted_session_without_live` and `try_recover_persisted_session`
/// stay in `meerkat-rpc` until W3-A: the former depends on the RPC-private
/// `ArchiveRuntimeCleanup`; the latter is a `#[cfg(test)]` helper that
/// composes RPC-private `TurnOverrides` and `RpcError`.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
mod ops {
    use std::sync::Arc;

    use meerkat_core::service::SessionError;
    use meerkat_core::types::SessionId;
    use meerkat_runtime::MeerkatMachine;

    use crate::PersistentSessionService;
    use crate::service_factory::FactoryAgentBuilder;
    use crate::session_runtime::admission::{
        StagedCapacityAdmissions, discard_staged_capacity_admission,
    };
    use crate::{StagedSessionRegistry, session_runtime::recovery::RecoveryContext};

    /// Surface-agnostic session-state observers shared across surfaces.
    pub struct RuntimeStateOps<'a> {
        /// Persistent session service.
        pub service: &'a Arc<PersistentSessionService<FactoryAgentBuilder>>,
        /// Staged session registry.
        pub staged_sessions: &'a Arc<StagedSessionRegistry>,
        /// Staged capacity ledger; consumed when discarding a live session
        /// that is not currently staged so capacity returns to the pool.
        pub staged_capacity_admissions: &'a StagedCapacityAdmissions,
        /// Runtime adapter.
        pub runtime_adapter: &'a Arc<MeerkatMachine>,
    }

    impl RuntimeStateOps<'_> {
        /// Discard a live session. If the session is not currently
        /// staged, the staged-capacity admission is released back to
        /// the pool.
        pub async fn discard_live_session(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            let result = self.service.discard_live_session(session_id).await;
            if result.is_ok() && !self.staged_sessions.contains(session_id).await {
                discard_staged_capacity_admission(self.staged_capacity_admissions, session_id);
            }
            result
        }

        /// Discard a stale live session and unregister it from the
        /// runtime adapter.
        pub async fn discard_stale_live_session(&self, session_id: &SessionId) {
            let _ = self.discard_live_session(session_id).await;
            self.runtime_adapter.unregister_session(session_id).await;
        }

        /// Determine whether the live projection for `session_id` has
        /// fallen behind the durable authoritative snapshot. Returns
        /// `Ok(false)` once the synchronization shortcut has fired or
        /// the live snapshot already mirrors the durable record.
        ///
        /// `recovery_ctx` provides the
        /// [`RecoveryContext::load_persisted_session`] flow used to
        /// cross-check the durable snapshot — surfaces pass their own
        /// recovery wiring so this observer does not embed the lookup.
        pub async fn live_session_is_stale(
            &self,
            session_id: &SessionId,
            recovery_ctx: &RecoveryContext<'_>,
        ) -> Result<bool, SessionError> {
            if self
                .service
                .synchronize_live_session_from_durable_authority_if_needed(session_id)
                .await?
            {
                return Ok(false);
            }

            let live = match self.service.export_live_session(session_id).await {
                Ok(session) => session,
                Err(SessionError::NotFound { .. }) => {
                    return Ok(recovery_ctx
                        .load_persisted_session(session_id)
                        .await?
                        .is_some());
                }
                Err(err) => return Err(err),
            };
            let Some(stored) = recovery_ctx.load_persisted_session(session_id).await? else {
                return Ok(false);
            };
            Ok(stored.messages().len() > live.messages().len())
        }
    }
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub use ops::RuntimeStateOps;
