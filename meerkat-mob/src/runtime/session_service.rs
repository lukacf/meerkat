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

    async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
        // No storage-level ownership contract is enforced here.
        // Callers should provide a wrapper service with explicit mob ownership checks.
        false
    }
}
