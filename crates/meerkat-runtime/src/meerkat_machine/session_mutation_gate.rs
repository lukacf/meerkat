//! The per-session mutation gate (M) and the only ways to wait for it.
//!
//! M serializes same-session mutating commands across the full DSL-stage ->
//! driver-mutate -> DSL-sync span. Without it, two concurrent commands on the
//! same session can interleave between the DSL projection sync (which
//! releases the `sessions` lock) and the driver mutation (which takes the
//! `driver` lock on its own). It is not a replacement for either lock: it is
//! an additional serialization point that spans the whole multi-step window.
//!
//! Every copy of M is a [`SessionMutationGate`]: the session entry's, an
//! unregister saga's incarnation witness, a member residency's registration
//! and effect lease, a registration witness's weak copy, and any gate a
//! command captured before it waited. The mutex is private, so whoever holds
//! a copy takes the gate in one of three named ways:
//!
//! - [`SessionMutationGate::lock_live`], for work on a live session. Once the
//!   session degrades while another holder keeps the gate, the wait ends with
//!   the degradation's cause. The holder may be one that keeps the gate until
//!   the session is reloaded, such as the post-stop fence an exited runtime
//!   loop retains after a failed boundary commit.
//! - [`SessionMutationGate::lock_settled`], which waits as long as the holder
//!   keeps the gate. Its caller names why that is right with a
//!   [`SettledGateWait`].
//! - [`SessionMutationGate::try_lock`], which never waits.

use std::sync::{Arc, Weak};

use crate::tokio::sync::{Mutex, OwnedMutexGuard, TryLockError};

use super::durability_health::{DurabilityHealthHandle, DurabilityReloadRequired};

/// One session entry incarnation's mutation gate (M). Clones are copies of
/// the same gate; [`Self::same_gate`] compares incarnations.
#[derive(Clone)]
pub(crate) struct SessionMutationGate {
    mutex: Arc<Mutex<()>>,
    /// The entry's durability health, whose degradation ends a live wait.
    /// `None` for a session without durable state, which never degrades.
    durability_health: Option<DurabilityHealthHandle>,
    /// Durable steers admitted under this gate and acknowledged whose
    /// boundary preparation has not registered with the actor yet. An
    /// admission announces here while it holds the gate, before it answers
    /// its caller; the actor's runner keeps its boundary open meanwhile.
    pending_boundary_admissions: meerkat_core::PendingBoundaryAdmissions,
}

/// A copy of a [`SessionMutationGate`] that does not keep its entry
/// incarnation's gate alive.
#[derive(Clone)]
pub(crate) struct WeakSessionMutationGate {
    mutex: Weak<Mutex<()>>,
    durability_health: Option<DurabilityHealthHandle>,
    pending_boundary_admissions: meerkat_core::PendingBoundaryAdmissions,
}

/// How a helper that serves both kinds of waiter takes the gate. Only the
/// live lifecycle lease's gate helper serves both, so it exists with `live`.
#[cfg(feature = "live")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionMutationGateWait {
    /// [`SessionMutationGate::lock_live`].
    Live,
    /// [`SessionMutationGate::lock_settled`] for the reason given.
    Settled(SettledGateWait),
}

/// Why a waiter may wait for the gate for as long as its holder keeps it,
/// instead of ending when the session degrades.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettledGateWait {
    /// The waiter's own entry is not published yet, or it aborts a
    /// materialization claim, which exists only while no executor of the
    /// entry is attached: no runtime loop or boundary commit of the session
    /// can keep the gate. (An attached actor recovery's claim is the
    /// exception, and its bindings never reach the abort: they own no
    /// rollback.)
    PreInstall,
    /// The waiter runs only after the holders a degradation can strand have
    /// released the gate: the reload-required discard's own stop of the
    /// executor, which runs once the boundary commit settled.
    AfterSettle,
    /// The degraded settlement itself, handed off by a waiter that left on
    /// the degradation: it takes the gate only to detach the degraded entry
    /// and hard-cancel its exact run.
    DegradedSettlement,
    /// An unregister saga whose final `UnregisterSession` is staged or
    /// committed: ending the wait would strand the pending finalization, which
    /// the reload-required discard refuses.
    StagedUnregister,
    /// A predecessor settlement's own internal registration: it never
    /// attaches an executor, so no runtime loop or boundary commit of the
    /// session can keep the gate, and its marker refuses every other taker.
    UnattachedSettlement,
    /// A test fixture that installs test state under the gate.
    #[cfg(any(test, feature = "test-support"))]
    TestFixture,
}

impl SessionMutationGate {
    /// A free gate for a new entry incarnation, carrying that entry's
    /// durability health.
    pub(crate) fn new(durability_health: Option<DurabilityHealthHandle>) -> Self {
        Self {
            mutex: Arc::new(Mutex::new(())),
            durability_health,
            pending_boundary_admissions: meerkat_core::PendingBoundaryAdmissions::new(),
        }
    }

    /// Where an admission under this gate announces an acknowledged durable
    /// steer whose boundary preparation is still to register.
    pub(crate) fn pending_boundary_admissions(&self) -> &meerkat_core::PendingBoundaryAdmissions {
        &self.pending_boundary_admissions
    }

    /// Whether `other` is a copy of this gate: the same entry incarnation.
    pub(crate) fn same_gate(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.mutex, &other.mutex)
    }

    /// The gate's address, equal for every copy. Another gate can have it only
    /// once every copy of this one is gone, so a key held with a copy is this
    /// gate's alone.
    pub(crate) fn address(&self) -> usize {
        Arc::as_ptr(&self.mutex).addr()
    }

    /// Whether [`Self::lock_live`] can end before the gate is free: only a
    /// session with durable state degrades.
    pub(crate) fn ends_on_degradation(&self) -> bool {
        self.durability_health.is_some()
    }

    /// How many copies of this gate are alive; tests watch a waiter take one.
    #[cfg(test)]
    pub(crate) fn copy_count(&self) -> usize {
        Arc::strong_count(&self.mutex)
    }

    pub(crate) fn downgrade(&self) -> WeakSessionMutationGate {
        WeakSessionMutationGate {
            mutex: Arc::downgrade(&self.mutex),
            durability_health: self.durability_health.clone(),
            pending_boundary_admissions: self.pending_boundary_admissions.clone(),
        }
    }

    /// Take the gate for work on a live session.
    ///
    /// The gate is polled before the degradation signal, so a free gate is
    /// taken even when the session has already degraded, and the caller sees
    /// the degraded entry under the gate as before. While another holder keeps
    /// the gate, the wait ends with the degradation's retained cause once the
    /// session degrades, including a degradation that came before the call. A
    /// session without durable state never degrades, so its wait lasts as
    /// long as the holder keeps the gate.
    pub(crate) async fn lock_live(&self) -> Result<OwnedMutexGuard<()>, DurabilityReloadRequired> {
        let Some(health) = self.durability_health.as_ref() else {
            return Ok(Arc::clone(&self.mutex).lock_owned().await);
        };
        let degradation = health.subscribe_degradation();
        let mut lock = std::pin::pin!(Arc::clone(&self.mutex).lock_owned());
        let mut fired = std::pin::pin!(degradation.fired());
        crate::tokio::select! {
            biased;
            guard = &mut lock => Ok(guard),
            required = &mut fired => Err(required),
        }
    }

    /// Take the gate however long its holder keeps it. `reason` is the call
    /// site's account of why no holder the session's degradation can strand
    /// stands in the way; it does not change the wait.
    pub(crate) async fn lock_settled(&self, reason: SettledGateWait) -> OwnedMutexGuard<()> {
        let _ = reason;
        Arc::clone(&self.mutex).lock_owned().await
    }

    /// Take the gate only if it is free now.
    pub(crate) fn try_lock(&self) -> Result<OwnedMutexGuard<()>, TryLockError> {
        Arc::clone(&self.mutex).try_lock_owned()
    }
}

impl std::fmt::Debug for SessionMutationGate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionMutationGate")
            .field("gate", &Arc::as_ptr(&self.mutex))
            .field("ends_on_degradation", &self.ends_on_degradation())
            .finish()
    }
}

impl WeakSessionMutationGate {
    /// Whether `other` is a weak copy of the same gate.
    pub(crate) fn same_gate(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.mutex, &other.mutex)
    }

    /// The gate, while its entry incarnation's gate is still alive.
    pub(crate) fn upgrade(&self) -> Option<SessionMutationGate> {
        Some(SessionMutationGate {
            mutex: self.mutex.upgrade()?,
            durability_health: self.durability_health.clone(),
            pending_boundary_admissions: self.pending_boundary_admissions.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

    use super::*;
    use crate::meerkat_machine::durability_health::begin_registration_cold_install;
    use std::time::Duration;

    const BOUND: Duration = Duration::from_secs(5);

    fn ready_health() -> DurabilityHealthHandle {
        let (health, rehydration) = begin_registration_cold_install();
        rehydration
            .mark_ready()
            .expect("a fresh cold install publishes Ready");
        health
    }

    #[tokio::test]
    async fn a_live_wait_behind_a_holder_ends_with_the_cause_once_the_session_degrades() {
        let health = ready_health();
        let gate = SessionMutationGate::new(Some(health.clone()));
        let held = gate.try_lock().expect("a new gate is free");
        let waiter = tokio::spawn({
            let gate = gate.clone();
            async move { gate.lock_live().await.map(|_| ()) }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "a ready session's live wait waits");
        assert!(health.mark_reload_required("boundary_commit", "outcome unknown"));
        let ended = tokio::time::timeout(BOUND, waiter)
            .await
            .expect("the degradation ends the wait")
            .expect("the waiter completes");
        let required = ended.expect_err("the wait ends without the gate");
        assert_eq!(required.operation(), "boundary_commit");
        drop(held);
    }

    #[tokio::test]
    async fn a_live_wait_takes_a_free_gate_of_an_already_degraded_session() {
        let health = ready_health();
        assert!(health.mark_reload_required("boundary_commit", "outcome unknown"));
        let gate = SessionMutationGate::new(Some(health));
        for _ in 0..64 {
            let guard = gate
                .lock_live()
                .await
                .expect("the gate is polled before the degradation");
            drop(guard);
        }
    }

    #[tokio::test]
    async fn a_live_wait_behind_a_holder_of_an_already_degraded_session_ends_at_once() {
        let health = ready_health();
        assert!(health.mark_reload_required("boundary_commit", "outcome unknown"));
        let gate = SessionMutationGate::new(Some(health));
        let _held = gate.try_lock().expect("a new gate is free");
        let required = tokio::time::timeout(BOUND, gate.lock_live())
            .await
            .expect("an earlier degradation ends the wait")
            .expect_err("the wait ends without the gate");
        assert_eq!(required.operation(), "boundary_commit");
    }

    #[tokio::test]
    async fn a_settled_wait_and_a_session_without_durable_state_wait_for_the_holder() {
        let health = ready_health();
        let durable = SessionMutationGate::new(Some(health.clone()));
        let storeless = SessionMutationGate::new(None);
        let durable_held = durable.try_lock().expect("a new gate is free");
        let storeless_held = storeless.try_lock().expect("a new gate is free");
        let settled = tokio::spawn({
            let durable = durable.clone();
            async move {
                drop(durable.lock_settled(SettledGateWait::TestFixture).await);
            }
        });
        let storeless_live = tokio::spawn({
            let storeless = storeless.clone();
            async move { storeless.lock_live().await.map(drop) }
        });
        assert!(health.mark_reload_required("boundary_commit", "outcome unknown"));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !settled.is_finished(),
            "a settled wait outlasts the degradation"
        );
        assert!(!storeless_live.is_finished(), "a storeless live wait waits");
        drop(durable_held);
        drop(storeless_held);
        tokio::time::timeout(BOUND, settled)
            .await
            .expect("the settled wait takes the released gate")
            .expect("the settled waiter completes");
        tokio::time::timeout(BOUND, storeless_live)
            .await
            .expect("the storeless wait takes the released gate")
            .expect("the storeless waiter completes")
            .expect("a session without durable state never degrades");
    }

    #[test]
    fn copies_share_one_gate_and_a_weak_copy_follows_its_incarnation() {
        let gate = SessionMutationGate::new(None);
        let copy = gate.clone();
        let other = SessionMutationGate::new(None);
        assert!(gate.same_gate(&copy));
        assert!(!gate.same_gate(&other));
        let held = copy.try_lock().expect("a new gate is free");
        assert!(gate.try_lock().is_err(), "a copy holds the same gate");
        drop(held);
        let weak = gate.downgrade();
        assert!(
            weak.upgrade()
                .is_some_and(|upgraded| upgraded.same_gate(&gate))
        );
        drop(gate);
        drop(copy);
        assert!(weak.upgrade().is_none(), "a weak copy keeps no gate alive");
    }
}
