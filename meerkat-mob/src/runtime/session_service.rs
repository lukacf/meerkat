use super::*;

// ---------------------------------------------------------------------------
// MobSessionService trait extension
// ---------------------------------------------------------------------------

/// Extension trait for session services that support comms runtime access.
///
/// This bridges the gap between `SessionService` (which doesn't expose comms)
/// and the mob runtime's need to access comms runtimes for wiring.
#[async_trait::async_trait]
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
        let runtime = self
            .comms_runtime(session_id)
            .await
            .ok_or_else(|| StreamError::NotFound(format!("session {}", session_id)))?;
        runtime.stream(StreamScope::Session(session_id.clone()))
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
}

#[async_trait::async_trait]
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
}
