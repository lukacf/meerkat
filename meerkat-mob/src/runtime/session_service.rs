use super::*;
use meerkat_core::AgentExecutionSnapshot;
use meerkat_core::CommsCapabilityError;
use meerkat_core::ExternalToolSurfaceSnapshot;
use meerkat_core::PeerIngressRuntimeSnapshot;
use meerkat_core::PendingSystemContextAppend;
use meerkat_core::Session;
use meerkat_core::ToolScopeSnapshot;
use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::service::StartTurnRequest;
use meerkat_core::service::{
    SessionError, SessionServiceCommsExt, SessionServiceControlExt, SessionServiceHistoryExt,
};
use meerkat_core::{InputId, RunId};
use sha2::{Digest, Sha256};
#[cfg(feature = "runtime-adapter")]
use std::collections::HashMap;
#[cfg(feature = "runtime-adapter")]
use std::sync::{Mutex, OnceLock, Weak};

fn build_runtime_receipt(
    run_id: RunId,
    boundary: RunApplyBoundary,
    contributing_input_ids: Vec<InputId>,
    session: &Session,
) -> Result<RunBoundaryReceipt, SessionError> {
    let encoded_messages = serde_json::to_vec(session.messages()).map_err(|err| {
        SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
            "failed to serialize session for runtime receipt digest: {err}"
        )))
    })?;
    Ok(RunBoundaryReceipt {
        run_id,
        boundary,
        contributing_input_ids,
        conversation_digest: Some(format!("{:x}", Sha256::digest(encoded_messages))),
        message_count: session.messages().len(),
        sequence: 0,
    })
}

#[cfg(feature = "runtime-adapter")]
fn ephemeral_runtime_adapter_cache()
-> &'static Mutex<HashMap<usize, Weak<meerkat_runtime::MeerkatMachine>>> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Weak<meerkat_runtime::MeerkatMachine>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(all(not(target_arch = "wasm32"), feature = "runtime-adapter"))]
fn persistent_runtime_adapter_cache()
-> &'static Mutex<HashMap<usize, Weak<meerkat_runtime::MeerkatMachine>>> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Weak<meerkat_runtime::MeerkatMachine>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(feature = "runtime-adapter")]
fn cached_runtime_adapter(
    cache: &'static Mutex<HashMap<usize, Weak<meerkat_runtime::MeerkatMachine>>>,
    key: usize,
    init: impl FnOnce() -> Arc<meerkat_runtime::MeerkatMachine>,
) -> Arc<meerkat_runtime::MeerkatMachine> {
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.retain(|_, adapter| adapter.strong_count() > 0);
    if let Some(existing) = cache.get(&key).and_then(Weak::upgrade) {
        return existing;
    }
    let adapter = init();
    cache.insert(key, Arc::downgrade(&adapter));
    adapter
}

// ---------------------------------------------------------------------------
// MobSessionService trait extension
// ---------------------------------------------------------------------------

/// Extension trait for session services used by the mob runtime.
///
/// Builds on `SessionServiceCommsExt` from core so mob orchestration can use
/// comms/injector access without per-crate bridge traits.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait MobSessionService:
    SessionServiceCommsExt + SessionServiceControlExt + SessionServiceHistoryExt
{
    /// Subscribe to session-wide events regardless of triggering interaction.
    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        <Self as SessionService>::subscribe_session_events(self, session_id).await
    }

    /// Whether this service satisfies the persistent-session contract required
    /// by REQ-MOB-030.
    fn supports_persistent_sessions(&self) -> bool {
        false
    }

    #[cfg(feature = "runtime-adapter")]
    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        None
    }

    async fn execution_snapshot(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError> {
        Ok(None)
    }

    async fn tool_scope_snapshot(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<ToolScopeSnapshot>, SessionError> {
        Ok(None)
    }

    async fn external_tool_surface_snapshot(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<ExternalToolSurfaceSnapshot>, SessionError> {
        Ok(None)
    }

    async fn peer_ingress_runtime_snapshot(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<PeerIngressRuntimeSnapshot>, SessionError> {
        Ok(None)
    }

    /// Whether a listed session belongs to the given mob for reconciliation.
    ///
    /// Default: `false`. The wave-a demolition removed the comms-name matching
    /// probe; wave-c was intended to land a runtime-aware replacement but did
    /// not, so this remains a no-op default. Persistent services may override
    /// to implement real reconciliation; the ephemeral default treats no
    /// listed session as "belongs to mob".
    async fn session_belongs_to_mob(
        &self,
        _session_id: &SessionId,
        _mob_id: &crate::ids::MobId,
    ) -> bool {
        false
    }

    /// Load the persisted session snapshot when available.
    ///
    /// Default: `Ok(None)`. Matches the pre-demolition behavior where services
    /// without durable persistence returned no snapshot, letting callers treat
    /// the session as missing and fall through to recreate-from-roster paths.
    async fn load_persisted_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        Ok(None)
    }

    async fn apply_runtime_turn(
        &self,
        _session_id: &SessionId,
        _run_id: RunId,
        _req: StartTurnRequest,
        _boundary: RunApplyBoundary,
        _contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        Err(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(
                "runtime-backed apply is unavailable for this session service".into(),
            ),
        ))
    }

    async fn apply_runtime_context_appends(
        &self,
        _session_id: &SessionId,
        _run_id: RunId,
        _appends: Vec<PendingSystemContextAppend>,
        _contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        Err(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(
                "runtime-backed context append apply is unavailable for this session service"
                    .into(),
            ),
        ))
    }

    async fn discard_live_session(&self, _session_id: &SessionId) -> Result<(), SessionError> {
        Ok(())
    }

    /// Cancel all active checkpointer gates.
    ///
    /// After this call in-flight saves complete but subsequent checkpoint
    /// calls on any session are no-ops. Call during `stop()` to prevent
    /// checkpoint writes from racing with external cleanup.
    async fn cancel_all_checkpointers(&self) {}

    /// Re-enable checkpointer gates cancelled by [`cancel_all_checkpointers`].
    ///
    /// Call during `resume()` to restore periodic persistence.
    async fn rearm_all_checkpointers(&self) {}
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl<B> MobSessionService for meerkat_session::EphemeralSessionService<B>
where
    B: meerkat_session::SessionAgentBuilder + 'static,
{
    fn supports_persistent_sessions(&self) -> bool {
        false
    }

    #[cfg(feature = "runtime-adapter")]
    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        let key = std::ptr::from_ref(self) as usize;
        Some(cached_runtime_adapter(
            ephemeral_runtime_adapter_cache(),
            key,
            || Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
        ))
    }

    async fn execution_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::execution_snapshot(self, session_id).await
    }

    async fn tool_scope_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ToolScopeSnapshot>, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::tool_scope_snapshot(self, session_id).await
    }

    async fn external_tool_surface_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ExternalToolSurfaceSnapshot>, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::external_tool_surface_snapshot(
            self, session_id,
        )
        .await
    }

    async fn peer_ingress_runtime_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<PeerIngressRuntimeSnapshot>, SessionError> {
        let Some(runtime) = self.comms_runtime(session_id).await else {
            return Ok(None);
        };

        match runtime.peer_ingress_runtime_snapshot().await {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(CommsCapabilityError::Unsupported(_)) => Ok(None),
        }
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        meerkat_session::EphemeralSessionService::<B>::subscribe_session_events(self, session_id)
            .await
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        meerkat_session::EphemeralSessionService::<B>::discard_live_session(self, session_id).await
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::start_turn(self, session_id, req).await?;
        let session =
            meerkat_session::EphemeralSessionService::<B>::export_session(self, session_id).await?;
        let receipt = build_runtime_receipt(run_id, boundary, contributing_input_ids, &session)?;
        let session_snapshot = serde_json::to_vec(&session).map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to serialize session snapshot for runtime commit: {err}"
            )))
        })?;
        Ok(CoreApplyOutput::without_terminal(
            receipt,
            Some(session_snapshot),
        ))
    }

    async fn apply_runtime_context_appends(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        meerkat_session::EphemeralSessionService::<B>::apply_runtime_context_appends(
            self,
            session_id,
            run_id,
            appends,
            contributing_input_ids,
        )
        .await
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl<B> MobSessionService for meerkat_session::PersistentSessionService<B>
where
    B: meerkat_session::SessionAgentBuilder + 'static,
{
    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    #[cfg(feature = "runtime-adapter")]
    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let key = std::ptr::from_ref(self) as usize;
            self.runtime_store().map(|store| {
                cached_runtime_adapter(persistent_runtime_adapter_cache(), key, || {
                    Arc::new(meerkat_runtime::MeerkatMachine::persistent(
                        store,
                        self.blob_store(),
                    ))
                })
            })
        }
    }

    async fn execution_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError> {
        meerkat_session::PersistentSessionService::<B>::execution_snapshot(self, session_id).await
    }

    async fn tool_scope_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ToolScopeSnapshot>, SessionError> {
        meerkat_session::PersistentSessionService::<B>::tool_scope_snapshot(self, session_id).await
    }

    async fn external_tool_surface_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ExternalToolSurfaceSnapshot>, SessionError> {
        meerkat_session::PersistentSessionService::<B>::external_tool_surface_snapshot(
            self, session_id,
        )
        .await
    }

    async fn peer_ingress_runtime_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<PeerIngressRuntimeSnapshot>, SessionError> {
        let Some(runtime) = self.comms_runtime(session_id).await else {
            return Ok(None);
        };

        match runtime.peer_ingress_runtime_snapshot().await {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(CommsCapabilityError::Unsupported(_)) => Ok(None),
        }
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        meerkat_session::PersistentSessionService::<B>::subscribe_session_events(self, session_id)
            .await
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        meerkat_session::PersistentSessionService::<B>::discard_live_session(self, session_id).await
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        meerkat_session::PersistentSessionService::<B>::apply_runtime_turn(
            self,
            session_id,
            run_id,
            req,
            boundary,
            contributing_input_ids,
        )
        .await
    }

    async fn apply_runtime_context_appends(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        appends: Vec<PendingSystemContextAppend>,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        meerkat_session::PersistentSessionService::<B>::apply_runtime_context_appends(
            self,
            session_id,
            run_id,
            appends,
            contributing_input_ids,
        )
        .await
    }

    async fn cancel_all_checkpointers(&self) {
        meerkat_session::PersistentSessionService::<B>::cancel_all_checkpointers(self).await;
    }

    async fn rearm_all_checkpointers(&self) {
        meerkat_session::PersistentSessionService::<B>::rearm_all_checkpointers(self).await;
    }
}
