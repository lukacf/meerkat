use super::*;
use meerkat_core::Session;
use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::service::StartTurnRequest;
use meerkat_core::service::{SessionError, SessionServiceCommsExt, SessionServiceControlExt};
use meerkat_core::{InputId, RunId};
use sha2::{Digest, Sha256};
#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
fn ephemeral_runtime_adapter_cache()
-> &'static Mutex<HashMap<usize, Weak<meerkat_runtime::RuntimeSessionAdapter>>> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Weak<meerkat_runtime::RuntimeSessionAdapter>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(not(target_arch = "wasm32"))]
fn persistent_runtime_adapter_cache()
-> &'static Mutex<HashMap<usize, Weak<meerkat_runtime::RuntimeSessionAdapter>>> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Weak<meerkat_runtime::RuntimeSessionAdapter>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(not(target_arch = "wasm32"))]
fn cached_runtime_adapter(
    cache: &'static Mutex<HashMap<usize, Weak<meerkat_runtime::RuntimeSessionAdapter>>>,
    key: usize,
    init: impl FnOnce() -> Arc<meerkat_runtime::RuntimeSessionAdapter>,
) -> Arc<meerkat_runtime::RuntimeSessionAdapter> {
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
pub trait MobSessionService: SessionServiceCommsExt + SessionServiceControlExt {
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

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::RuntimeSessionAdapter>> {
        None
    }

    /// Whether a listed session belongs to the given mob for reconciliation.
    async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
        false
    }

    /// Load the persisted session snapshot when available.
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

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::RuntimeSessionAdapter>> {
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let key = std::ptr::from_ref(self) as usize;
            Some(cached_runtime_adapter(
                ephemeral_runtime_adapter_cache(),
                key,
                || Arc::new(meerkat_runtime::RuntimeSessionAdapter::ephemeral()),
            ))
        }
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        meerkat_session::EphemeralSessionService::<B>::subscribe_session_events(self, session_id)
            .await
    }

    async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
        false
    }

    async fn load_persisted_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        Ok(None)
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
        Ok(CoreApplyOutput {
            receipt,
            session_snapshot: Some(session_snapshot),
        })
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

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::RuntimeSessionAdapter>> {
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let key = std::ptr::from_ref(self) as usize;
            self.runtime_store().map(|store| {
                cached_runtime_adapter(persistent_runtime_adapter_cache(), key, || {
                    Arc::new(meerkat_runtime::RuntimeSessionAdapter::persistent(store))
                })
            })
        }
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        meerkat_session::PersistentSessionService::<B>::subscribe_session_events(self, session_id)
            .await
    }

    async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
        self.load_persisted_session(_session_id)
            .await
            .ok()
            .flatten()
            .and_then(|session| {
                session
                    .session_metadata()
                    .and_then(|metadata| metadata.comms_name)
            })
            .is_some_and(|name| name.starts_with(&format!("{_mob_id}/")))
    }

    async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        meerkat_session::PersistentSessionService::<B>::load_persisted(self, session_id).await
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

    async fn cancel_all_checkpointers(&self) {
        meerkat_session::PersistentSessionService::<B>::cancel_all_checkpointers(self).await;
    }

    async fn rearm_all_checkpointers(&self) {
        meerkat_session::PersistentSessionService::<B>::rearm_all_checkpointers(self).await;
    }
}
