//! Runtime-state observers and cleanup primitives.
//!
//! Populated by W1-E (`PendingSessionEventStreams`,
//! `PendingSessionEventStreamDrop`), W2-E (`SessionInfo`,
//! `SessionState` enums + state observers), and W3-A (skill-identity
//! registry plumbing + `ArchiveRuntimeCleanup`).
//!
//! `ArchiveRuntimeCleanup` lives here in trait-shaped form: the MCP
//! and Mob cleanup hooks are abstracted behind
//! [`ArchiveRuntimeMcpState`] and [`ArchiveRuntimeMobState`] so the
//! struct does not need to depend on `meerkat-mob-mcp` (which the
//! `meerkat` facade does not pull in) or know about the RPC-private
//! `SessionMcpState`. Surfaces wire their own implementations.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::EventEnvelope;
use meerkat_core::event::AgentEvent;
use meerkat_core::skills::{SkillError, SourceIdentityRegistry};
use meerkat_core::types::SessionId;
use tokio::sync::{Mutex, Notify, broadcast};

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

/// Skill-identity registry slot held by the session runtime.
///
/// `generation` is a monotone version stamp that lets the runtime guard
/// against stale writes after a config reload races a still-in-flight
/// builder. Surfaces inject the latest registry into
/// `ContextWindowBuilder` per-session via the runtime accessor.
#[derive(Clone, Default)]
pub struct SkillIdentityRegistryState {
    /// Monotonic generation counter; writes only land if their
    /// generation is `>=` the currently-stored generation.
    pub generation: u64,
    /// The active registry projected onto agents.
    pub registry: SourceIdentityRegistry,
}

/// Build a `SourceIdentityRegistry` from runtime config.
///
/// Surface-agnostic: surfaces (RPC, REST, CLI, embedded examples) call
/// this from their config-load path and feed the result into a
/// runtime accessor. Identical body to the original RPC-side helper.
#[allow(clippy::missing_errors_doc)]
pub fn build_skill_identity_registry(
    config: &meerkat_core::Config,
    context_root: Option<&std::path::Path>,
    user_root: Option<&std::path::Path>,
) -> Result<SourceIdentityRegistry, SkillError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (context_root, user_root);
        config.skills.build_source_identity_registry()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (context_root, user_root);
        config.skills.build_source_identity_registry()
    }
}

/// Per-surface MCP cleanup hook used by [`ArchiveRuntimeCleanup`].
///
/// Surfaces that own MCP session adapters (e.g. `meerkat-rpc`) implement
/// this to shutdown their adapter when a session is archived. The
/// trait is intentionally narrow: `cleanup` is the only side-effect the
/// archive flow needs.
#[async_trait]
pub trait ArchiveRuntimeMcpState: Send + Sync {
    /// Tear down the MCP adapter (and any per-session lifecycle plumbing)
    /// for `session_id`. Implementations are expected to be idempotent —
    /// a missing entry is a no-op.
    async fn cleanup(&self, session_id: &SessionId);
}

/// Per-surface Mob cleanup hook used by [`ArchiveRuntimeCleanup`].
///
/// `meerkat-rpc` (the only surface today that wires mob orchestration)
/// implements this on top of `meerkat_mob_mcp::MobMcpState`. The trait
/// keeps `meerkat` from depending on `meerkat-mob-mcp`.
#[async_trait]
pub trait ArchiveRuntimeMobState: Send + Sync {
    /// Drop session-scoped mob state. The result is propagated by
    /// [`ArchiveRuntimeCleanup::run`].
    async fn cleanup(
        &self,
        session_id: &SessionId,
    ) -> Result<(), meerkat_core::service::SessionError>;

    /// Whether session-scoped mob state still exists for `session_id`
    /// after the service-side archive failed. The archive flow uses
    /// this to distinguish "session never existed" from "session is
    /// retained for follow-up cleanup".
    async fn has_retained_cleanup(&self, session_id: &SessionId) -> bool;
}

/// Surface-agnostic archive cleanup orchestrator.
///
/// Composes the runtime adapter, the per-session pending event-stream
/// map, and the optional MCP / Mob cleanup hooks. Surfaces build one of
/// these per archive call and either invoke `archive_service` followed
/// by `run`, or just `run` when the service step has already happened.
#[derive(Clone)]
pub struct ArchiveRuntimeCleanup {
    /// Runtime adapter; used to unregister the session and abort comms
    /// drain.
    pub runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    /// Per-session pending event streams. `None` for surfaces that
    /// don't track them (e.g. mob session service flows).
    pub pending_session_event_streams:
        Option<Arc<Mutex<HashMap<SessionId, PendingSessionEventStreams>>>>,
    /// Optional MCP cleanup hook; surfaces implement
    /// [`ArchiveRuntimeMcpState`] for their adapter map.
    pub mcp_state: Option<Arc<dyn ArchiveRuntimeMcpState>>,
    /// Optional Mob cleanup hook; surfaces implement
    /// [`ArchiveRuntimeMobState`] for their `MobMcpState` wrapper.
    pub mob_state: Option<Arc<dyn ArchiveRuntimeMobState>>,
}

impl ArchiveRuntimeCleanup {
    /// Whether the mob cleanup hook reports retained per-session state
    /// after the service-side archive returned `NotFound`. Used by the
    /// archive flow to decide whether to surface the not-found error or
    /// treat the archive as a benign no-op.
    pub async fn has_retained_mob_cleanup(&self, session_id: &SessionId) -> bool {
        if let Some(mob_state) = self.mob_state.as_ref()
            && mob_state.has_retained_cleanup(session_id).await
        {
            return true;
        }
        false
    }

    /// Run the durable-store archive step. Surfaces that already
    /// archived through the service skip this and call [`run`]
    /// directly.
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    pub async fn archive_service(
        &self,
        service: &crate::PersistentSessionService<crate::service_factory::FactoryAgentBuilder>,
        session_id: &SessionId,
    ) -> Result<(), meerkat_core::service::SessionError> {
        service
            .archive_with_machine_protocol(
                session_id,
                meerkat_session::MachineSessionArchiveProtocol::from_machine(
                    self.runtime_adapter.as_ref(),
                ),
            )
            .await
    }

    /// Run the per-surface cleanup steps that follow a successful
    /// archive: unregister from the runtime adapter, drop pending event
    /// streams, tear down MCP adapters, destroy mob state, abort comms
    /// drain.
    pub async fn run(
        &self,
        session_id: &SessionId,
    ) -> Result<(), meerkat_core::service::SessionError> {
        self.runtime_adapter.unregister_session(session_id).await;
        if let Some(streams) = self.pending_session_event_streams.as_ref() {
            streams.lock().await.remove(session_id);
        }
        if let Some(mcp_state) = self.mcp_state.as_ref() {
            mcp_state.cleanup(session_id).await;
        }
        if let Some(mob_state) = self.mob_state.as_ref() {
            mob_state.cleanup(session_id).await?;
        }
        #[cfg(feature = "comms")]
        self.runtime_adapter.abort_comms_drain(session_id).await;
        Ok(())
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
