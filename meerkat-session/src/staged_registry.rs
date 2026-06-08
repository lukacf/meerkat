//! `StagedSessionRegistry` — the single typed owner of session
//! materialization status and the global active-capacity admission seam.
//!
//! # Why this exists (projection-promotion / dogma)
//!
//! Before this type existed, a session's *materialization status* — whether it
//! is `Staged` (capacity reserved for a deferred first turn), `Promoting`
//! (transitioning a staged reservation into live work), or `Active` (holding
//! live work) — was a **derived projection** spread across two unrelated
//! locks:
//!
//! - the global `Semaphore` (which only answers "is there spare capacity?"),
//!   and
//! - the `sessions` `RwLock` (which only answers "does a handle exist?").
//!
//! No single owner held the canonical answer to "what is this session's
//! materialization status right now?". Reconstructing it required reading both
//! locks and inferring the status — a textbook promotion of a projection into
//! authority.
//!
//! `StagedSessionRegistry` folds both facts behind **one** lock so that:
//!
//! 1. [`MaterializationStatus`] is a singular typed fact owned here, never
//!    re-derived from permit-existence + handle-existence elsewhere; and
//! 2. the capacity permit is reserved/consumed inside the *same* write-lock
//!    that records the status transition, so admission is one atomic seam
//!    rather than a non-atomic two-step across separate locks.
//!
//! The `Semaphore` remains a pure **resource gate** (concurrency limiter): it
//! bounds how many sessions may hold live work at once. It never decides
//! whether a session is a valid/admitted authority — that decision is owned by
//! runtime/machine admission. This registry treats the permit purely as a
//! concurrency token whose acquisition is fused to the typed status flip.

use indexmap::IndexMap;
use meerkat_core::error::AgentError;
use meerkat_core::service::SessionError;
use meerkat_core::types::SessionId;
use std::sync::Mutex;

// Tokio re-exports: on wasm32, use the crate-level alias (tokio_with_wasm).
#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::{OwnedSemaphorePermit, Semaphore};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use std::sync::Arc;

/// Singular typed materialization status for a session.
///
/// This is the canonical answer to "where is this session in its
/// reservation/admission lifecycle". It is never reconstructed from
/// permit-existence or handle-existence; the [`StagedSessionRegistry`] is its
/// only owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializationStatus {
    /// Capacity is reserved for a deferred first turn, but no live work holds
    /// it yet. The reservation can be promoted to `Active` when the deferred
    /// turn actually starts.
    Staged,
    /// A staged reservation is being promoted into live work. This transient
    /// status is held only for the duration of a single promotion seam and
    /// guards against a concurrent second promotion of the same reservation.
    Promoting,
    /// Live work currently holds capacity for this session.
    Active,
}

/// Outcome of a single admission attempt at the registry's write-locked seam.
///
/// The permit (when present) is handed back to the caller so it can be threaded
/// into the per-session lease bookkeeping. A `None` permit means the registry
/// has no global capacity gate configured (unbounded), which is a valid
/// "admitted without a token" outcome — distinct from "rejected".
pub struct AdmissionOutcome {
    /// The status the session now holds in the registry.
    pub status: MaterializationStatus,
    /// The capacity permit consumed for this admission, when a gate is
    /// configured. `None` means the registry is unbounded.
    pub permit: Option<OwnedSemaphorePermit>,
}

/// The single owner of materialization status and the global active-capacity
/// admission seam.
///
/// All status transitions and permit reservations flow through this type's
/// write-lock, so the two facts can never disagree across a window.
pub struct StagedSessionRegistry {
    inner: Mutex<RegistryInner>,
    /// Global concurrency limit on simultaneously-active sessions.
    ///
    /// RESOURCE GATE, not lifecycle authority. `None` means unbounded. See the
    /// module docs for why this is fused into the same lock as the status map.
    capacity: Option<Arc<Semaphore>>,
    /// Configured ceiling, retained only for diagnostic "N/M" error messages.
    max_sessions: Option<usize>,
}

#[derive(Default)]
struct RegistryInner {
    status: IndexMap<SessionId, MaterializationStatus>,
}

impl StagedSessionRegistry {
    /// Create a registry bounded by `max_sessions` simultaneously-active
    /// sessions.
    pub fn bounded(max_sessions: usize) -> Self {
        Self {
            inner: Mutex::new(RegistryInner::default()),
            capacity: Some(Arc::new(Semaphore::new(max_sessions))),
            max_sessions: Some(max_sessions),
        }
    }

    /// Create an unbounded registry with no global capacity gate.
    pub fn unbounded() -> Self {
        Self {
            inner: Mutex::new(RegistryInner::default()),
            capacity: None,
            max_sessions: None,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Build the typed "max sessions reached" admission error from the current
    /// permit accounting. Fail-closed: callers never silently downgrade a
    /// capacity rejection to success.
    fn capacity_exhausted_error(&self) -> SessionError {
        let max_sessions = self.max_sessions.unwrap_or(0);
        let available = self
            .capacity
            .as_ref()
            .map(|c| c.available_permits())
            .unwrap_or(0);
        let active = max_sessions.saturating_sub(available);
        SessionError::Agent(AgentError::InternalError(format!(
            "Max sessions reached ({active}/{max_sessions})"
        )))
    }

    /// Reserve global capacity inside the registry lock, recording the session
    /// at `status`. The permit (if a gate is configured) is consumed atomically
    /// with the status record and handed back so the caller can thread it into
    /// per-session lease bookkeeping.
    ///
    /// Fail-closed: when the capacity gate is exhausted this returns a typed
    /// error and records no status — there is no partial/laundered admission.
    pub fn reserve(
        &self,
        id: &SessionId,
        status: MaterializationStatus,
    ) -> Result<AdmissionOutcome, SessionError> {
        let mut inner = self.lock();
        let permit = match self.capacity.as_ref() {
            Some(capacity) => match Arc::clone(capacity).try_acquire_owned() {
                Ok(permit) => Some(permit),
                Err(_) => return Err(self.capacity_exhausted_error()),
            },
            None => None,
        };
        inner.status.insert(id.clone(), status);
        Ok(AdmissionOutcome { status, permit })
    }

    /// Reserve a global capacity permit without yet recording a session-keyed
    /// status. Used by the session-create path, where the permit must be
    /// reserved before the agent (and therefore the canonical `SessionId`) is
    /// built. The caller threads the returned permit into the per-session lease
    /// and records the typed status with [`Self::record_status`] once the id is
    /// known.
    ///
    /// Fail-closed: a full gate returns the typed exhaustion error, never a
    /// silent "admitted without a permit".
    pub fn reserve_capacity(&self) -> Result<Option<OwnedSemaphorePermit>, SessionError> {
        // Hold the registry lock across the permit acquisition so capacity
        // accounting and status records cannot interleave with a concurrent
        // promotion/forget on the same gate.
        let _inner = self.lock();
        match self.capacity.as_ref() {
            Some(capacity) => match Arc::clone(capacity).try_acquire_owned() {
                Ok(permit) => Ok(Some(permit)),
                Err(_) => Err(self.capacity_exhausted_error()),
            },
            None => Ok(None),
        }
    }

    /// Record the typed materialization status for a session whose capacity
    /// permit was already reserved via [`Self::reserve_capacity`]. The permit
    /// lives in the caller's per-session lease bookkeeping; this only registers
    /// the singular typed status.
    pub fn record_status(&self, id: &SessionId, status: MaterializationStatus) {
        self.lock().status.insert(id.clone(), status);
    }

    /// Mark a tracked session as holding [`MaterializationStatus::Active`] live
    /// work inside the registry lock.
    ///
    /// The caller owns the actual capacity permit (it was handed back at
    /// `reserve`/`reserve_capacity` time), so this only flips the typed status;
    /// the two never disagree because both live behind this lock. No-op for an
    /// untracked session, so it can never create a phantom `Active` record.
    pub fn mark_active(&self, id: &SessionId) {
        let mut inner = self.lock();
        if inner.status.contains_key(id) {
            inner
                .status
                .insert(id.clone(), MaterializationStatus::Active);
        }
    }

    /// Flip a staged reservation to [`MaterializationStatus::Promoting`] inside
    /// the registry lock so a concurrent caller cannot promote the same
    /// reservation twice. Returns `true` if this caller won the transition (the
    /// session was `Staged`), `false` otherwise.
    pub fn begin_promotion(&self, id: &SessionId) -> bool {
        let mut inner = self.lock();
        match inner.status.get(id).copied() {
            Some(MaterializationStatus::Staged) => {
                inner
                    .status
                    .insert(id.clone(), MaterializationStatus::Promoting);
                true
            }
            _ => false,
        }
    }

    /// Current typed materialization status for a session, if the registry
    /// tracks it.
    pub fn status(&self, id: &SessionId) -> Option<MaterializationStatus> {
        self.lock().status.get(id).copied()
    }

    /// Drop the registry's status record for a session. The owned capacity
    /// permit is released by dropping it on the caller side (it lives in the
    /// session's lease bookkeeping); this only clears the typed status entry.
    pub fn forget(&self, id: &SessionId) {
        self.lock().status.swap_remove(id);
    }

    /// Probe whether spare capacity exists without consuming it. Fail-closed:
    /// returns the typed exhaustion error when no permits are available.
    pub fn ensure_capacity_available(&self) -> Result<(), SessionError> {
        let Some(capacity) = self.capacity.as_ref() else {
            return Ok(());
        };
        if capacity.available_permits() > 0 {
            return Ok(());
        }
        Err(self.capacity_exhausted_error())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn sid() -> SessionId {
        SessionId::new()
    }

    #[test]
    fn reserve_consumes_permit_and_records_typed_status() {
        let registry = StagedSessionRegistry::bounded(1);
        let id = sid();

        let outcome = registry
            .reserve(&id, MaterializationStatus::Staged)
            .expect("first reservation admitted");
        assert_eq!(outcome.status, MaterializationStatus::Staged);
        assert!(
            outcome.permit.is_some(),
            "bounded registry hands back the consumed permit"
        );
        // The typed status is owned by the registry, not re-derived elsewhere.
        assert_eq!(registry.status(&id), Some(MaterializationStatus::Staged));
    }

    #[test]
    fn reserve_fails_closed_when_capacity_exhausted() {
        let registry = StagedSessionRegistry::bounded(1);
        let held = registry
            .reserve(&sid(), MaterializationStatus::Active)
            .expect("first reservation admitted");
        // Holding the permit keeps the gate closed.
        assert!(held.permit.is_some());

        let err = registry
            .reserve(&sid(), MaterializationStatus::Active)
            .err()
            .expect("second reservation must be rejected, not laundered to success");
        let msg = format!("{err}");
        assert!(
            msg.contains("Max sessions reached"),
            "expected typed capacity-exhaustion error, got: {msg}"
        );
    }

    #[test]
    fn permit_returns_to_gate_when_outcome_dropped() {
        let registry = StagedSessionRegistry::bounded(1);
        {
            let outcome = registry
                .reserve(&sid(), MaterializationStatus::Active)
                .expect("first reservation admitted");
            drop(outcome);
        }
        // After the permit drops, the next reservation succeeds again.
        registry
            .reserve(&sid(), MaterializationStatus::Active)
            .expect("capacity freed after permit drop");
    }

    #[test]
    fn begin_promotion_is_single_winner() {
        let registry = StagedSessionRegistry::bounded(2);
        let id = sid();
        registry
            .reserve(&id, MaterializationStatus::Staged)
            .expect("staged reservation admitted");

        assert!(
            registry.begin_promotion(&id),
            "first promotion of a staged reservation wins"
        );
        assert_eq!(registry.status(&id), Some(MaterializationStatus::Promoting));
        assert!(
            !registry.begin_promotion(&id),
            "second concurrent promotion of the same reservation is rejected"
        );
    }

    #[test]
    fn mark_active_only_transitions_tracked_sessions() {
        let registry = StagedSessionRegistry::bounded(2);
        let id = sid();
        // Untracked session: mark_active is a no-op (no phantom Active record).
        registry.mark_active(&id);
        assert_eq!(registry.status(&id), None);

        registry
            .reserve(&id, MaterializationStatus::Staged)
            .expect("staged reservation admitted");
        registry.mark_active(&id);
        assert_eq!(registry.status(&id), Some(MaterializationStatus::Active));
    }

    #[test]
    fn forget_clears_typed_status() {
        let registry = StagedSessionRegistry::bounded(2);
        let id = sid();
        registry
            .reserve(&id, MaterializationStatus::Active)
            .expect("reservation admitted");
        registry.forget(&id);
        assert_eq!(registry.status(&id), None);
    }

    #[test]
    fn ensure_capacity_available_fails_closed() {
        let registry = StagedSessionRegistry::bounded(1);
        registry
            .ensure_capacity_available()
            .expect("spare capacity");
        let _held = registry
            .reserve(&sid(), MaterializationStatus::Active)
            .expect("reservation admitted");
        registry
            .ensure_capacity_available()
            .expect_err("must fail closed once the gate is full");
    }

    #[test]
    fn unbounded_registry_admits_without_permit() {
        let registry = StagedSessionRegistry::unbounded();
        let outcome = registry
            .reserve(&sid(), MaterializationStatus::Active)
            .expect("unbounded registry always admits");
        assert!(
            outcome.permit.is_none(),
            "unbounded registry has no permit to hand back"
        );
        registry
            .ensure_capacity_available()
            .expect("unbounded registry never exhausts");
    }
}
