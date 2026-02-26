use super::*;

// ---------------------------------------------------------------------------
// MobSessionService trait extension
// ---------------------------------------------------------------------------

/// Extension trait for session services that support comms runtime access.
///
/// This bridges the gap between `SessionService` (which doesn't expose comms)
/// and the mob runtime's need to access comms runtimes for wiring.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait MobSessionService: SessionService {
    /// Get the comms runtime for a session.
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>>;

    /// Get the subscribable event injector for a session, if available.
    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::SubscribableInjector>> {
        self.comms_runtime(session_id)
            .await
            .and_then(|runtime| runtime.event_injector())
    }

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

    /// Whether a listed session belongs to the given mob for reconciliation.
    async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
        false
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
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
        meerkat_session::EphemeralSessionService::<B>::comms_runtime(self, session_id).await
    }

    fn supports_persistent_sessions(&self) -> bool {
        false
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::SubscribableInjector>> {
        meerkat_session::EphemeralSessionService::<B>::event_injector(self, session_id).await
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
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl<B> MobSessionService for meerkat_session::PersistentSessionService<B>
where
    B: meerkat_session::SessionAgentBuilder + 'static,
{
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
        meerkat_session::PersistentSessionService::<B>::comms_runtime(self, session_id).await
    }

    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::SubscribableInjector>> {
        meerkat_session::PersistentSessionService::<B>::event_injector(self, session_id).await
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        meerkat_session::PersistentSessionService::<B>::subscribe_session_events(self, session_id)
            .await
    }

    async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
        // No storage-level ownership contract is enforced here.
        // Callers should provide a wrapper service with explicit mob ownership checks.
        false
    }

    async fn cancel_all_checkpointers(&self) {
        meerkat_session::PersistentSessionService::<B>::cancel_all_checkpointers(self).await;
    }

    async fn rearm_all_checkpointers(&self) {
        meerkat_session::PersistentSessionService::<B>::rearm_all_checkpointers(self).await;
    }
}
