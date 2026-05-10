//! Capacity admission helpers for the session runtime.
//!
//! Populated by W1-A (`StagedCapacityAdmissions` typedef + helpers)
//! and W1-C (`RuntimePreAdmission` family of RAII guards).

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, Weak};

use meerkat_core::InputId;
use meerkat_core::types::SessionId;
use meerkat_session::RuntimeContextAdmissionGuard;
use tokio::sync::Mutex;

use crate::StagedSessionRegistry;

/// Type alias for a capacity guard issued by the session service for a
/// staged or running session. The actual guard type lives in
/// `meerkat-session`; we re-alias here so the runtime crate can use a
/// stable name without leaking the dependency path through every call
/// site.
pub type ActiveCapacityGuard = RuntimeContextAdmissionGuard;

/// Map of staged sessions to their reserved capacity admissions.
///
/// The shared mutex models the staged-session capacity ledger: while a
/// session is staged in `StagedSessionRegistry`, its admission is held
/// here and restored if promotion fails. The lock is `std::sync::Mutex`
/// because every operation is short and synchronous.
pub type StagedCapacityAdmissions = Arc<StdMutex<HashMap<SessionId, ActiveCapacityGuard>>>;

/// Restore a previously-taken admission back into the staged-capacity
/// ledger. Used by RAII guards (`RuntimePreAdmission`,
/// `PendingPromotionCleanup`, …) on rollback paths.
pub fn restore_staged_capacity_admission(
    admissions: &StagedCapacityAdmissions,
    session_id: SessionId,
    admission: ActiveCapacityGuard,
) {
    let mut admissions = admissions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    admissions.insert(session_id, admission);
}

/// Reason an `insert_staged_capacity_admission` call could not be
/// satisfied. Surface-agnostic: surfaces translate to their own error
/// shape (`RpcError::SESSION_BUSY`, HTTP 409, …) at the call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedCapacityCollision {
    /// The session id whose admission slot was already populated.
    pub session_id: SessionId,
}

/// Insert a fresh admission into the staged-capacity ledger.
///
/// Returns `Err(StagedCapacityCollision)` when the session id already
/// has an admission staged — surfaces map this onto their own
/// session-busy wire error.
pub fn insert_staged_capacity_admission(
    admissions: &StagedCapacityAdmissions,
    session_id: SessionId,
    admission: ActiveCapacityGuard,
) -> Result<(), StagedCapacityCollision> {
    let mut guard = admissions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if guard.contains_key(&session_id) {
        return Err(StagedCapacityCollision { session_id });
    }
    guard.insert(session_id, admission);
    Ok(())
}

/// Remove and return the staged admission for `session_id`, if any.
pub fn take_staged_capacity_admission(
    admissions: &StagedCapacityAdmissions,
    session_id: &SessionId,
) -> Option<ActiveCapacityGuard> {
    admissions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(session_id)
}

/// Whether `session_id` currently holds a staged admission.
pub fn has_staged_capacity_admission(
    admissions: &StagedCapacityAdmissions,
    session_id: &SessionId,
) -> bool {
    admissions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains_key(session_id)
}

/// Drop the staged admission for `session_id` if present. The guard's
/// `Drop` returns capacity to the pool.
pub fn discard_staged_capacity_admission(
    admissions: &StagedCapacityAdmissions,
    session_id: &SessionId,
) {
    drop(take_staged_capacity_admission(admissions, session_id));
}

/// Carries the metadata needed to restore an admission to the staged
/// ledger when a `RuntimePreAdmission` originally taken from the
/// staged-capacity bucket goes out of scope without being consumed.
#[derive(Clone)]
pub struct StagedAdmissionRestore {
    /// The shared staged-capacity ledger to restore the admission to.
    pub admissions: StagedCapacityAdmissions,
    /// The session id whose slot the admission belongs in.
    pub session_id: SessionId,
}

/// RAII admission carried through the runtime input pipeline.
///
/// `fresh` holds an admission that simply returns capacity to the pool
/// when dropped. `staged` holds an admission whose Drop restores it to
/// `StagedCapacityAdmissions` for later consumption by the staged
/// session's promotion path.
pub struct RuntimePreAdmission {
    pub(crate) admission: Option<ActiveCapacityGuard>,
    pub(crate) staged_restore: Option<StagedAdmissionRestore>,
}

impl RuntimePreAdmission {
    /// Build a fresh admission whose Drop simply releases capacity.
    pub fn fresh(admission: ActiveCapacityGuard) -> Self {
        Self {
            admission: Some(admission),
            staged_restore: None,
        }
    }

    /// Build a staged admission whose Drop restores the guard to the
    /// staged ledger keyed on `session_id`.
    pub fn staged(
        session_id: SessionId,
        admissions: StagedCapacityAdmissions,
        admission: ActiveCapacityGuard,
    ) -> Self {
        Self {
            admission: Some(admission),
            staged_restore: Some(StagedAdmissionRestore {
                admissions,
                session_id,
            }),
        }
    }

    /// Consume `self` and return the wrapped guard, suppressing the
    /// staged-restore Drop semantics.
    #[allow(clippy::expect_used)]
    pub fn into_admission(mut self) -> ActiveCapacityGuard {
        self.staged_restore = None;
        self.admission
            .take()
            .expect("runtime pre-admission should not be consumed twice")
    }
}

impl From<ActiveCapacityGuard> for RuntimePreAdmission {
    fn from(admission: ActiveCapacityGuard) -> Self {
        Self::fresh(admission)
    }
}

impl Drop for RuntimePreAdmission {
    fn drop(&mut self) {
        let Some(admission) = self.admission.take() else {
            return;
        };
        if let Some(restore) = self.staged_restore.take() {
            restore_staged_capacity_admission(&restore.admissions, restore.session_id, admission);
        } else {
            drop(admission);
        }
    }
}

/// One-shot guard around a [`RuntimePreAdmission`] that allows the
/// caller to take ownership at the success path; if dropped without
/// `take()`, the inner Drop semantics fire (releasing or restoring
/// staged capacity).
pub struct RuntimePreAdmissionGuard {
    admission: Option<RuntimePreAdmission>,
}

impl RuntimePreAdmissionGuard {
    /// Wrap a [`RuntimePreAdmission`] (or anything `Into<…>`) in the
    /// guard.
    pub fn new(admission: impl Into<RuntimePreAdmission>) -> Self {
        Self {
            admission: Some(admission.into()),
        }
    }

    /// Take the inner admission, consuming the guard's Drop semantics.
    pub fn take(&mut self) -> Option<RuntimePreAdmission> {
        self.admission.take()
    }
}

/// Pre-admission entry keyed on a runtime input id; tracked while the
/// runtime is still mediating the admission to a session.
pub struct RuntimePreAdmissionEntry {
    /// The input id this admission is reserved for.
    pub input_id: InputId,
    /// The reserved admission.
    pub admission: RuntimePreAdmission,
}

/// RAII lock-lease over the per-session registration mutex.
///
/// The lease holds an `Arc<Mutex<()>>` that callers `lock` to serialize
/// entry into a session's registration critical section. When the
/// lease is the last strong reference, Drop also evicts the entry from
/// the locks map so the next caller can re-create it.
pub struct RuntimeRegistrationLockLease {
    /// The shared map of per-session registration locks.
    pub locks: Arc<StdMutex<HashMap<SessionId, Weak<Mutex<()>>>>>,
    /// The session id this lease is bound to.
    pub session_id: SessionId,
    /// The mutex itself; callers `lock()` it to enter the critical
    /// section.
    pub lock: Arc<Mutex<()>>,
}

impl RuntimeRegistrationLockLease {
    /// Borrow the underlying registration mutex.
    pub fn mutex(&self) -> &Mutex<()> {
        &self.lock
    }
}

impl Drop for RuntimeRegistrationLockLease {
    fn drop(&mut self) {
        if Arc::strong_count(&self.lock) != 1 {
            return;
        }
        let this_lock = Arc::downgrade(&self.lock);
        let mut locks = self
            .locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if locks
            .get(&self.session_id)
            .is_some_and(|registered: &Weak<Mutex<()>>| registered.ptr_eq(&this_lock))
        {
            locks.remove(&self.session_id);
        }
    }
}

/// RAII rollback guard around the staged session registry's archive
/// path. While armed, Drop schedules a `restore_archive` task on the
/// tokio runtime so a partial archive failure leaves the staged
/// session intact. Calling `disarm` after a successful archive
/// suppresses the restore.
pub struct StagedArchiveRollbackGuard {
    staged_sessions: Arc<StagedSessionRegistry>,
    session_id: SessionId,
    restore_on_drop: bool,
}

impl StagedArchiveRollbackGuard {
    /// Arm a rollback guard for `session_id`.
    pub fn new(staged_sessions: Arc<StagedSessionRegistry>, session_id: &SessionId) -> Self {
        Self {
            staged_sessions,
            session_id: session_id.clone(),
            restore_on_drop: true,
        }
    }

    /// Suppress the Drop-time restore (call after a successful archive).
    pub fn disarm(&mut self) {
        self.restore_on_drop = false;
    }
}

impl Drop for StagedArchiveRollbackGuard {
    fn drop(&mut self) {
        if !self.restore_on_drop {
            return;
        }
        let staged_sessions = Arc::clone(&self.staged_sessions);
        let session_id = self.session_id.clone();
        tokio::spawn(async move {
            let _ = staged_sessions.restore_archive(&session_id).await;
        });
    }
}

/// Per-surface hook that releases or restores the runtime
/// pre-admission tracked under `(session_id, input_id)` when its
/// [`RuntimePreAdmissionRegistration`] guard goes out of scope.
///
/// Surfaces (RPC today; REST/CLI/embedded examples later) implement
/// this on top of their own `runtime_pre_admissions` map. The trait
/// keeps [`RuntimePreAdmissionRegistration`] from depending on a
/// concrete `SessionRuntime` type.
pub trait RuntimePreAdmissionRestore: Send + Sync {
    /// Release the admission entry for `(session_id, input_id)`. Drop
    /// semantics call this when `release_on_drop` was not disarmed.
    /// Implementations must be idempotent — a missing entry is a
    /// no-op.
    fn restore_or_release(&self, session_id: &SessionId, input_id: &InputId);
}

/// RAII guard around a runtime pre-admission registration.
///
/// While alive, it holds onto the `(session_id, input_id)` slot in the
/// surface's pre-admission map; on Drop, it asks the surface (via
/// [`RuntimePreAdmissionRestore`]) to release or restore the slot.
/// `disarm` consumes the guard without firing the Drop release —
/// callers do this once the admission has been promoted into a
/// long-lived owner (e.g. handed to the runtime executor task).
pub struct RuntimePreAdmissionRegistration {
    runtime: Arc<dyn RuntimePreAdmissionRestore>,
    session_id: SessionId,
    input_id: InputId,
    release_on_drop: bool,
}

impl RuntimePreAdmissionRegistration {
    /// Build a new registration. The `runtime` parameter is the
    /// surface's own pre-admission ledger; surfaces wrap themselves in
    /// an `Arc<dyn RuntimePreAdmissionRestore>` and pass it in.
    pub fn new(
        runtime: Arc<dyn RuntimePreAdmissionRestore>,
        session_id: SessionId,
        input_id: InputId,
    ) -> Self {
        Self {
            runtime,
            session_id,
            input_id,
            release_on_drop: true,
        }
    }

    /// Suppress Drop-time release. Call after the admission has been
    /// transferred to its long-lived owner.
    pub fn disarm(mut self) {
        self.release_on_drop = false;
    }
}

impl Drop for RuntimePreAdmissionRegistration {
    fn drop(&mut self) {
        if self.release_on_drop {
            self.runtime
                .restore_or_release(&self.session_id, &self.input_id);
        }
    }
}
