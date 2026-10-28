//! The session-owner wake registry: how a session-keyed live operation
//! reaches the owner of a session that is not materialized in this process.
//!
//! A mob member's session can be seated in its mob without a live actor in
//! this process (the member is dormant). A live operation addressed to that
//! session by its id (a turn, a live channel, comms as the member, a
//! scheduled session target, a session-addressed runtime delivery) must not
//! fail for that, and must not materialize the session on its own either:
//! the member's owner decides, through its machine, whether and how the
//! member wakes. This registry is the declared projection that routes such
//! an operation to that owner.
//!
//! - Source: the owner's own machine facts. A mob publishes an entry for
//!   each seated, session-bound local member when its machine publishes the
//!   member's wake route (`PublishWakeRoute`, and `RebuildWakeRoutes` at the
//!   owner's start), keyed by the member's bound session, and retracts it
//!   with the route (`RetractWakeRoute`).
//! - Trigger: a session-keyed live operation that finds no live actor for
//!   its session asks [`SessionOwnerWakeRegistry::wake`] before it fails,
//!   and a cold attach of a session (the session service's own seam every
//!   surface attaches through) asks it before materializing the session
//!   itself.
//!   An operation that only reads, or that cancels (an interrupt, a
//!   cancel), never wakes: [`SessionOwnerWakeRegistry::owned`] answers
//!   whether an owner holds the session, so it can answer "no run open"
//!   without waking.
//! - Staleness: an entry names its owner weakly. An owner that is gone is a
//!   miss (the operation fails as before). When a member's binding moves to
//!   a successor session, its owner retracts the old session's entry and
//!   publishes the successor's in the same step, so an entry never routes a
//!   session its owner no longer binds; an owner that is asked about a
//!   session it no longer owns answers [`SessionOwnerWakeOutcome::NotOwned`]
//!   and wakes nothing.
//! - Boundary: process-local, keyed by session id, one entry per session.
//!   The registry decides nothing: the owner's answer is the operation's.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, Weak};

use crate::service::SessionError;
use crate::types::SessionId;

/// What an owner answered for a wake of one of its sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionOwnerWakeOutcome {
    /// The session's member is live and ready: the operation proceeds.
    Ready,
    /// The owner no longer owns this session (a stale entry): the operation
    /// proceeds as for an unowned session.
    NotOwned,
}

/// The future an owner answers a wake with.
#[cfg(not(target_arch = "wasm32"))]
pub type SessionOwnerWakeFuture =
    futures::future::BoxFuture<'static, Result<SessionOwnerWakeOutcome, SessionError>>;
/// The future an owner answers a wake with (wasm32: single-threaded).
#[cfg(target_arch = "wasm32")]
pub type SessionOwnerWakeFuture =
    futures::future::LocalBoxFuture<'static, Result<SessionOwnerWakeOutcome, SessionError>>;

/// The owner of sessions that may not be materialized in this process.
///
/// Implemented by a mob: it wakes the member bound to the session through
/// its machine and answers once the member is ready, or with the typed
/// refusal (as a [`SessionError`]). It never runs the operation itself.
pub trait SessionOwnerWaker: Send + Sync {
    /// Wake the member that owns `session_id` for a live operation. A wake
    /// is asked only when the caller found no live actor serving the
    /// session, so the owner takes that as its observation: a member it
    /// still holds live has lost its materialization.
    fn wake_session_owner(&self, session_id: &SessionId) -> SessionOwnerWakeFuture;
}

/// The process-wide session-owner wake registry. See the module docs.
#[derive(Default)]
pub struct SessionOwnerWakeRegistry {
    entries: parking_lot::RwLock<HashMap<SessionId, Weak<dyn SessionOwnerWaker>>>,
}

static GLOBAL_SESSION_OWNER_WAKE_REGISTRY: OnceLock<SessionOwnerWakeRegistry> = OnceLock::new();

impl SessionOwnerWakeRegistry {
    /// The process-wide registry.
    pub fn global() -> &'static Self {
        GLOBAL_SESSION_OWNER_WAKE_REGISTRY.get_or_init(Self::default)
    }

    /// Route `session_id` to `owner`, replacing any earlier entry.
    pub fn publish(&self, session_id: SessionId, owner: Weak<dyn SessionOwnerWaker>) {
        self.entries.write().insert(session_id, owner);
    }

    /// Remove `session_id`'s entry if `owner` published it (another owner's
    /// later entry for the same session stays). Returns whether it was
    /// removed.
    pub fn retract(&self, session_id: &SessionId, owner: &Weak<dyn SessionOwnerWaker>) -> bool {
        let mut entries = self.entries.write();
        let owned = entries
            .get(session_id)
            .is_some_and(|current| Weak::ptr_eq(current, owner));
        if owned {
            entries.remove(session_id);
        }
        owned
    }

    fn owner(&self, session_id: &SessionId) -> Option<Arc<dyn SessionOwnerWaker>> {
        self.entries.read().get(session_id).and_then(Weak::upgrade)
    }

    /// Whether a live owner holds `session_id`. A read: it wakes nothing.
    pub fn owned(&self, session_id: &SessionId) -> bool {
        self.owner(session_id).is_some()
    }

    /// Ask `session_id`'s owner to wake its member; the caller found no
    /// live actor serving the session. `None` when no live owner holds the
    /// session: the caller proceeds as it would without this registry.
    pub async fn wake(
        &self,
        session_id: &SessionId,
    ) -> Option<Result<SessionOwnerWakeOutcome, SessionError>> {
        let owner = self.owner(session_id)?;
        Some(owner.wake_session_owner(session_id).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScriptedOwner(SessionOwnerWakeOutcome);

    impl SessionOwnerWaker for ScriptedOwner {
        fn wake_session_owner(&self, _session_id: &SessionId) -> SessionOwnerWakeFuture {
            let outcome = self.0;
            Box::pin(async move { Ok(outcome) })
        }
    }

    fn weak(owner: &Arc<dyn SessionOwnerWaker>) -> Weak<dyn SessionOwnerWaker> {
        Arc::downgrade(owner)
    }

    #[tokio::test]
    async fn a_published_session_routes_to_its_owner_until_retracted() {
        let registry = SessionOwnerWakeRegistry::default();
        let session = SessionId::new();
        let owner: Arc<dyn SessionOwnerWaker> =
            Arc::new(ScriptedOwner(SessionOwnerWakeOutcome::Ready));
        assert!(registry.wake(&session).await.is_none());
        registry.publish(session.clone(), weak(&owner));
        assert!(registry.owned(&session));
        assert!(matches!(
            registry.wake(&session).await,
            Some(Ok(SessionOwnerWakeOutcome::Ready))
        ));
        assert!(registry.retract(&session, &weak(&owner)));
        assert!(!registry.owned(&session));
        assert!(registry.wake(&session).await.is_none());
    }

    #[tokio::test]
    async fn retraction_is_owner_exact_and_a_gone_owner_is_a_miss() {
        let registry = SessionOwnerWakeRegistry::default();
        let session = SessionId::new();
        let first: Arc<dyn SessionOwnerWaker> =
            Arc::new(ScriptedOwner(SessionOwnerWakeOutcome::Ready));
        let second: Arc<dyn SessionOwnerWaker> =
            Arc::new(ScriptedOwner(SessionOwnerWakeOutcome::NotOwned));
        registry.publish(session.clone(), weak(&first));
        registry.publish(session.clone(), weak(&second));
        assert!(!registry.retract(&session, &weak(&first)));
        assert!(matches!(
            registry.wake(&session).await,
            Some(Ok(SessionOwnerWakeOutcome::NotOwned))
        ));
        drop(second);
        assert!(!registry.owned(&session));
        assert!(registry.wake(&session).await.is_none());
    }
}
