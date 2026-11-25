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
//! - Owners of sessions no entry names: an owner that does not run in this
//!   process now has no entries (a mob that is not loaded has no machine to
//!   publish them), so its members' sessions would read as nobody's. A host
//!   of such owners (a mob set that loads a mob when something addresses
//!   it) publishes itself with
//!   [`SessionOwnerWakeRegistry::publish_owner_of_unnamed_sessions`]. A
//!   wake asks it only when no entry names the session, or the entry's
//!   owner answers that it no longer owns it; it decides from the session's
//!   own stored record whether one of its owners holds the session, starts
//!   that owner, and hands the wake to the entry the owner then published.
//!   A read ([`SessionOwnerWakeRegistry::owned`]) never asks it, so a read
//!   or a cancel starts no owner.
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
    /// The owners of sessions no entry names, in the order they were
    /// published. See [`Self::publish_owner_of_unnamed_sessions`].
    unnamed: parking_lot::RwLock<Vec<Weak<dyn SessionOwnerWaker>>>,
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

    /// Publish `owner` as an owner of sessions no entry names: a host of
    /// owners that do not run in this process now (a mob set that loads a
    /// mob when something addresses it). [`Self::wake`] asks it only for a
    /// session whose entry is missing, gone, or answers
    /// [`SessionOwnerWakeOutcome::NotOwned`]; [`Self::owned`] never asks
    /// it. Publishing the same owner again changes nothing.
    pub fn publish_owner_of_unnamed_sessions(&self, owner: Weak<dyn SessionOwnerWaker>) {
        let mut unnamed = self.unnamed.write();
        unnamed.retain(|current| current.strong_count() > 0);
        if !unnamed.iter().any(|current| Weak::ptr_eq(current, &owner)) {
            unnamed.push(owner);
        }
    }

    /// Remove `owner` from the owners of sessions no entry names. Returns
    /// whether it was one.
    pub fn retract_owner_of_unnamed_sessions(&self, owner: &Weak<dyn SessionOwnerWaker>) -> bool {
        let mut unnamed = self.unnamed.write();
        let published = unnamed.iter().any(|current| Weak::ptr_eq(current, owner));
        unnamed.retain(|current| current.strong_count() > 0 && !Weak::ptr_eq(current, owner));
        published
    }

    /// Whether a live owner holds `session_id`. A read: it wakes nothing,
    /// and it never asks an owner of sessions no entry names.
    pub fn owned(&self, session_id: &SessionId) -> bool {
        self.owner(session_id).is_some()
    }

    /// Ask the owner `session_id`'s entry names to wake its member; the
    /// caller found no live actor serving the session. `None` when no live
    /// owner's entry names the session. The owners of sessions no entry
    /// names are not asked: one of them calls this once the owner it hosts
    /// runs and has published its entries.
    pub async fn wake_published(
        &self,
        session_id: &SessionId,
    ) -> Option<Result<SessionOwnerWakeOutcome, SessionError>> {
        let owner = self.owner(session_id)?;
        Some(owner.wake_session_owner(session_id).await)
    }

    /// Ask `session_id`'s owner to wake its member; the caller found no
    /// live actor serving the session. `None` when no owner holds the
    /// session: the caller proceeds as it would without this registry.
    ///
    /// The owner the session's entry names is asked first. When no entry
    /// names the session, or its owner answers that it no longer owns it,
    /// the owners of sessions no entry names are asked in the order they
    /// were published, and the first answer other than
    /// [`SessionOwnerWakeOutcome::NotOwned`] is the wake's.
    pub async fn wake(
        &self,
        session_id: &SessionId,
    ) -> Option<Result<SessionOwnerWakeOutcome, SessionError>> {
        let published = self.wake_published(session_id).await;
        if !matches!(
            published,
            None | Some(Ok(SessionOwnerWakeOutcome::NotOwned))
        ) {
            return published;
        }
        let unnamed = self
            .unnamed
            .read()
            .iter()
            .filter_map(Weak::upgrade)
            .collect::<Vec<_>>();
        for owner in unnamed {
            match owner.wake_session_owner(session_id).await {
                Ok(SessionOwnerWakeOutcome::NotOwned) => {}
                answer => return Some(answer),
            }
        }
        published
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

    /// An owner of sessions no entry names, which counts how often it was
    /// asked and answers what it was scripted with.
    struct CountingOwner {
        asked: std::sync::atomic::AtomicUsize,
        answer: Result<SessionOwnerWakeOutcome, ()>,
    }

    impl CountingOwner {
        fn answering(answer: Result<SessionOwnerWakeOutcome, ()>) -> Arc<Self> {
            Arc::new(Self {
                asked: std::sync::atomic::AtomicUsize::new(0),
                answer,
            })
        }

        fn asked(&self) -> usize {
            self.asked.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl SessionOwnerWaker for CountingOwner {
        fn wake_session_owner(&self, session_id: &SessionId) -> SessionOwnerWakeFuture {
            self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let answer = self.answer.map_err(|()| SessionError::Busy {
                id: session_id.clone(),
            });
            Box::pin(async move { answer })
        }
    }

    fn unnamed(owner: &Arc<CountingOwner>) -> Weak<dyn SessionOwnerWaker> {
        let owner: Arc<dyn SessionOwnerWaker> = owner.clone();
        Arc::downgrade(&owner)
    }

    #[tokio::test]
    async fn an_owner_of_unnamed_sessions_is_asked_only_on_a_miss_and_never_by_a_read() {
        let registry = SessionOwnerWakeRegistry::default();
        let session = SessionId::new();
        let host = CountingOwner::answering(Ok(SessionOwnerWakeOutcome::Ready));
        registry.publish_owner_of_unnamed_sessions(unnamed(&host));
        registry.publish_owner_of_unnamed_sessions(unnamed(&host));

        assert!(
            !registry.owned(&session),
            "no entry names the session, and a read asks nobody"
        );
        assert_eq!(host.asked(), 0, "the read did not ask the host");
        assert!(matches!(
            registry.wake(&session).await,
            Some(Ok(SessionOwnerWakeOutcome::Ready))
        ));
        assert_eq!(host.asked(), 1, "published twice, asked once");
        assert!(
            registry.wake_published(&session).await.is_none(),
            "the entries alone still miss"
        );
        assert_eq!(host.asked(), 1);

        // A live entry answers for its session; the host is not asked.
        let owner: Arc<dyn SessionOwnerWaker> =
            Arc::new(ScriptedOwner(SessionOwnerWakeOutcome::Ready));
        registry.publish(session.clone(), weak(&owner));
        assert!(matches!(
            registry.wake(&session).await,
            Some(Ok(SessionOwnerWakeOutcome::Ready))
        ));
        assert_eq!(host.asked(), 1);

        assert!(registry.retract_owner_of_unnamed_sessions(&unnamed(&host)));
        assert!(!registry.retract_owner_of_unnamed_sessions(&unnamed(&host)));
        assert!(registry.retract(&session, &weak(&owner)));
        assert!(registry.wake(&session).await.is_none());
        assert_eq!(host.asked(), 1, "a retracted host is not asked");
    }

    #[tokio::test]
    async fn an_entry_that_no_longer_owns_its_session_hands_the_wake_to_the_hosts() {
        let registry = SessionOwnerWakeRegistry::default();
        let session = SessionId::new();
        let closed: Arc<dyn SessionOwnerWaker> =
            Arc::new(ScriptedOwner(SessionOwnerWakeOutcome::NotOwned));
        registry.publish(session.clone(), weak(&closed));
        assert!(matches!(
            registry.wake(&session).await,
            Some(Ok(SessionOwnerWakeOutcome::NotOwned))
        ));

        // Hosts are asked in the order they were published; one that does
        // not own the session passes it on, and the first other answer is
        // the wake's: a refusal is an answer. A host published twice is
        // one host: it is asked once.
        let stranger = CountingOwner::answering(Ok(SessionOwnerWakeOutcome::NotOwned));
        let refusing = CountingOwner::answering(Err(()));
        let never = CountingOwner::answering(Ok(SessionOwnerWakeOutcome::Ready));
        for host in [&stranger, &stranger, &refusing, &never] {
            registry.publish_owner_of_unnamed_sessions(unnamed(host));
        }
        assert!(matches!(
            registry.wake(&session).await,
            Some(Err(SessionError::Busy { .. }))
        ));
        assert_eq!(
            (stranger.asked(), refusing.asked(), never.asked()),
            (1, 1, 0)
        );

        // With only hosts that do not own it, the entry's answer stands; a
        // host that is gone is not asked.
        assert!(registry.retract_owner_of_unnamed_sessions(&unnamed(&refusing)));
        let gone = unnamed(&never);
        drop(never);
        assert_eq!(gone.strong_count(), 0);
        assert!(matches!(
            registry.wake(&session).await,
            Some(Ok(SessionOwnerWakeOutcome::NotOwned))
        ));
        assert_eq!(stranger.asked(), 2);
    }
}
